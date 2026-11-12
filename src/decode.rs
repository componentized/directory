//! Decodes a published wasm binary, a component or an encoded WIT package, into WIT text and the
//! facts the index records about it.

use crate::data::{Dependency, Kind};
use anyhow::{Context, Result};
use wit_component::{DecodedWasm, WitPrinter};
use wit_parser::{IndexMap, PackageId, Resolve, WorldItem, WorldKey};

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
    items
        .iter()
        .filter_map(|(key, item)| match (key, item) {
            (WorldKey::Interface(_), WorldItem::Interface { .. }) => {
                Some(resolve.name_world_key(key))
            }
            (WorldKey::Name(name), WorldItem::Interface { id, .. }) => {
                Some(match resolve.id_of(*id) {
                    Some(aliased) => format!("{name}: {aliased}"),
                    None => format!("{name}: interface"),
                })
            }
            (_, WorldItem::Function(f)) => Some(format!("{}: func", f.name)),
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

#[cfg(test)]
mod tests {
    use super::*;

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
