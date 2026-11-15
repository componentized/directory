//! Decodes a published wasm binary, a component or an encoded WIT package, into WIT text and the
//! facts the index records about it.

use crate::data::{Dependency, Kind};
use anyhow::{Context, Result};
use wit_component::{DecodedWasm, WitPrinter};
use wit_parser::{IndexMap, PackageId, Resolve, Stability, WorldItem, WorldKey};

#[derive(Debug)]
pub struct Decoded {
    pub kind: Kind,
    /// The WIT text: the main package followed by the foreign packages it refers to, nested.
    pub wit: String,
    /// The package id and version of an encoded WIT package, `None` for a component.
    pub package: Option<(String, Option<String>)>,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub dependencies: Vec<Dependency>,
}

pub fn decode(wasm: &[u8]) -> Result<Decoded> {
    match wit_component::decode(wasm).context("decoding wasm")? {
        DecodedWasm::WitPackage(resolve, pkg) => {
            let name = &resolve.packages[pkg].name;
            Ok(Decoded {
                kind: Kind::Interface,
                wit: print(&resolve, pkg)?,
                package: Some((
                    format!("{}:{}", name.namespace, name.name),
                    name.version.as_ref().map(ToString::to_string),
                )),
                imports: Vec::new(),
                exports: Vec::new(),
                dependencies: dependencies(&resolve, pkg),
            })
        }
        DecodedWasm::Component(resolve, world) => {
            let w = &resolve.worlds[world];
            let pkg = w.package.context("component world without a package")?;
            Ok(Decoded {
                kind: Kind::Component,
                wit: print(&resolve, pkg)?,
                package: None,
                imports: world_items(&resolve, &w.imports),
                exports: world_items(&resolve, &w.exports),
                dependencies: dependencies(&resolve, pkg),
            })
        }
    }
}

/// Describes what a world imports or exports:
///
/// - an interface by its id, `wasi:logging/logging@0.1.0-draft`
/// - a named import of an interface by its name and the interface it aliases,
///   `latch0: componentized:http/latch@0.1.0`
/// - an inline interface, `scope: interface`
/// - a function, `run: func`
pub fn world_items(resolve: &Resolve, items: &IndexMap<WorldKey, WorldItem>) -> Vec<String> {
    gated_world_items(resolve, items)
        .into_iter()
        .map(|(item, _)| item)
        .collect()
}

/// The items of [`world_items`], each with its gate, e.g. the `@unstable(feature = ..)` of an
/// import only a host with the feature provides.
pub fn gated_world_items<'a>(
    resolve: &Resolve,
    items: &'a IndexMap<WorldKey, WorldItem>,
) -> Vec<(String, &'a Stability)> {
    items
        .iter()
        .filter_map(|(key, item)| match (key, item) {
            (WorldKey::Interface(_), WorldItem::Interface { stability, .. }) => {
                Some((resolve.name_world_key(key), stability))
            }
            (WorldKey::Name(name), WorldItem::Interface { id, stability, .. }) => Some((
                match resolve.id_of(*id) {
                    Some(aliased) => format!("{name}: {aliased}"),
                    None => format!("{name}: interface"),
                },
                stability,
            )),
            (_, WorldItem::Function(f)) => Some((format!("{}: func", f.name), &f.stability)),
            (_, WorldItem::Type { .. }) => None,
        })
        .collect()
}

/// The interface an item from [`world_items`] imports or exports, e.g.
/// `componentized:http/latch@0.1.0` for `latch0: componentized:http/latch@0.1.0`. `None` for an
/// inline interface or a function.
pub fn interface_of(item: &str) -> Option<&str> {
    match item.split_once(": ") {
        Some((_, "interface" | "func")) => None,
        Some((_, aliased)) => Some(aliased),
        None => Some(item),
    }
}

/// The imports and exports of a component, from its resolved WIT.
pub fn component_items(resolve: &Resolve, pkg: PackageId) -> Result<(Vec<String>, Vec<String>)> {
    let world = resolve
        .select_world(&[pkg], None)
        .context("selecting the component's world")?;
    let world = &resolve.worlds[world];
    Ok((
        world_items(resolve, &world.imports),
        world_items(resolve, &world.exports),
    ))
}

fn others(resolve: &Resolve, pkg: PackageId) -> Vec<PackageId> {
    resolve
        .topological_packages()
        .into_iter()
        .filter(|id| *id != pkg)
        .collect()
}

fn print(resolve: &Resolve, pkg: PackageId) -> Result<String> {
    let mut printer = WitPrinter::default();
    printer.print(resolve, pkg, &others(resolve, pkg))?;
    Ok(printer.output.to_string())
}

fn dependencies(resolve: &Resolve, pkg: PackageId) -> Vec<Dependency> {
    let mut deps: Vec<_> = others(resolve, pkg)
        .into_iter()
        .map(|id| {
            let name = &resolve.packages[id].name;
            Dependency {
                package: format!("{}:{}", name.namespace, name.name),
                version: name.version.as_ref().map(ToString::to_string),
            }
        })
        .collect();
    deps.sort();
    deps.dedup();
    deps
}

/// The components nested whole in a component, those it may have been composed from, each by the
/// digest of its bytes. A nested component's own nested components are its parts, not these.
pub fn parts(wasm: &[u8]) -> Result<Vec<crate::data::Part>> {
    use wasmparser::{Parser, Payload};
    let mut parts: Vec<crate::data::Part> = Vec::new();
    // how deep the parser is: 1 within the component, 2 within a part, and so on
    let mut depth = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.context("reading the component's nested components")? {
            Payload::Version { .. } => depth += 1,
            Payload::End(_) => depth -= 1,
            Payload::ComponentSection {
                unchecked_range, ..
            } if depth == 1 => {
                let range =
                    usize::try_from(unchecked_range.start)?..usize::try_from(unchecked_range.end)?;
                let bytes = wasm.get(range).context("a nested component out of range")?;
                parts.push(crate::data::Part {
                    digest: crate::http::sha256(bytes),
                    size: bytes.len() as u64,
                    code: false,
                });
            }
            // a core module within a part, at any depth, is code of its own
            Payload::ModuleSection { .. } if depth >= 2 => {
                if let Some(part) = parts.last_mut() {
                    part.code = true;
                }
            }
            _ => {}
        }
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_parts_of_a_composed_component() {
        use wasm_encoder::{Component, Module, ModuleSection, NestedComponentSection};
        // a part with code, a core module, and a wrapper of types alone, with none
        let mut code = Component::new();
        code.section(&ModuleSection(&Module::new()));
        let wrapper = Component::new();
        let mut composed = Component::new();
        composed.section(&NestedComponentSection(&code));
        composed.section(&NestedComponentSection(&wrapper));
        let (code, wrapper, composed) = (code.finish(), wrapper.finish(), composed.finish());

        let parts = parts(&composed).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].digest, crate::http::sha256(&code));
        assert_eq!(parts[0].size, code.len() as u64);
        assert!(parts[0].code);
        assert_eq!(parts[1].digest, crate::http::sha256(&wrapper));
        assert!(!parts[1].code);
        // a component nests no others
        assert!(super::parts(&wrapper).unwrap().is_empty());
    }

    const WIT: &str = r#"
package componentized:example@0.1.0;

world imports {
  import wasi:logging/logging@0.1.0-draft;
}

package wasi:logging@0.1.0-draft {
  interface logging {
    enum level { trace, debug, info, warn, error, critical }
    log: func(level: level, context: string, message: string);
  }
}
"#;

    #[test]
    fn wit_package_round_trip() {
        let mut resolve = Resolve::default();
        let pkg = resolve.push_str("example.wit", WIT).unwrap();
        let wasm = wit_component::encode(&resolve, pkg, false).unwrap();

        let decoded = decode(&wasm).unwrap();
        assert_eq!(decoded.kind, Kind::Interface);
        assert_eq!(
            decoded.package,
            Some((
                "componentized:example".to_string(),
                Some("0.1.0".to_string())
            ))
        );
        assert_eq!(
            decoded.dependencies,
            [Dependency {
                package: "wasi:logging".to_string(),
                version: Some("0.1.0-draft".to_string())
            }]
        );

        // the stored WIT parses back on its own
        let mut again = Resolve::default();
        let pkg = again.push_str("package.wit", &decoded.wit).unwrap();
        assert_eq!(again.packages[pkg].name.name, "example");
        assert!(decoded.wit.contains("log: func(level: level"));
    }

    #[test]
    fn named_and_inline_items() {
        let wit = r#"
package root:component;

world root {
  import latch0: componentized:http/latch@0.1.0;
  import scope: interface {
    get: func() -> string;
  }
  export componentized:http/latch@0.1.0;
  export run: func();
}

package componentized:http@0.1.0 {
  interface latch {
    allow: func() -> bool;
  }
}
"#;
        let mut resolve = Resolve::default();
        let pkg = resolve.push_str("world.wit", wit).unwrap();
        let (imports, exports) = component_items(&resolve, pkg).unwrap();
        assert_eq!(
            imports,
            ["latch0: componentized:http/latch@0.1.0", "scope: interface"]
        );
        // wit-parser lists a world's functions ahead of its interfaces
        assert_eq!(exports, ["run: func", "componentized:http/latch@0.1.0"]);
        assert_eq!(
            imports.iter().map(|i| interface_of(i)).collect::<Vec<_>>(),
            [Some("componentized:http/latch@0.1.0"), None]
        );
        assert_eq!(
            exports.iter().map(|i| interface_of(i)).collect::<Vec<_>>(),
            [None, Some("componentized:http/latch@0.1.0")]
        );
    }

    #[test]
    fn component_imports_and_exports() {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_str(
                "component.wit",
                &WIT.replace(
                    "world imports {\n  import wasi:logging/logging@0.1.0-draft;\n}",
                    "world root {\n  import wasi:logging/logging@0.1.0-draft;\n  export wasi:logging/logging@0.1.0-draft;\n  export run: func();\n}",
                ),
            )
            .unwrap();
        let world = resolve.select_world(&[pkg], Some("root")).unwrap();
        let mut module =
            wit_component::dummy_module(&resolve, world, wit_parser::ManglingAndAbi::Standard32);
        wit_component::embed_component_metadata(
            &mut module,
            &resolve,
            world,
            wit_component::StringEncoding::UTF8,
            false,
        )
        .unwrap();
        let wasm = wit_component::ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap();

        let decoded = decode(&wasm).unwrap();
        assert_eq!(decoded.kind, Kind::Component);
        assert_eq!(decoded.imports, ["wasi:logging/logging@0.1.0-draft"]);
        assert_eq!(
            decoded.exports,
            ["run: func", "wasi:logging/logging@0.1.0-draft"]
        );
        let mut again = Resolve::default();
        again.push_str("world.wit", &decoded.wit).unwrap();
    }
}
