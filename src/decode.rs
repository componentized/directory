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

/// The shape of each item a world imports or exports, to tell whether one world's items are among
/// another's: an interface by its id, e.g. `wasi:io/streams@0.2.6`, or by the name it's under and
/// its id, e.g. `latch0: componentized:http/latch@0.1.0`; an interface defined inline, a function
/// or a type, by its name and its structure, e.g. `log: func(message: string)`. Items of two worlds
/// are alike only when their shapes are identical.
pub fn world_shapes(resolve: &Resolve, items: &IndexMap<WorldKey, WorldItem>) -> Vec<String> {
    items
        .iter()
        .map(|(key, item)| world_item_shape(resolve, key, item))
        .collect()
}

fn world_item_shape(resolve: &Resolve, key: &WorldKey, item: &WorldItem) -> String {
    match (key, item) {
        (WorldKey::Interface(_), _) => resolve.name_world_key(key),
        (WorldKey::Name(name), WorldItem::Interface { id, .. }) => match resolve.id_of(*id) {
            Some(aliased) => format!("{name}: {aliased}"),
            None => format!(
                "{name}: interface {{ {} }}",
                interface_members(resolve, *id).join("; ")
            ),
        },
        (WorldKey::Name(_), WorldItem::Function(func)) => func_shape(resolve, func),
        (WorldKey::Name(name), WorldItem::Type { id, .. }) => type_def_shape(resolve, name, *id),
    }
}

/// Each item a world imports or exports with anything to call, by its key, with the shapes of
/// what's in it, for a component's compatibility with a world to compare item by item: an
/// interface by its id, e.g. `wasi:cli/run@0.2.6`, or by its name, e.g. `latch0:
/// componentized:http/latch@0.1.0` or `inline: interface`, with each of its types and functions,
/// and a function or a type, e.g. `log: func`, with its own shape. An interface of types alone,
/// with no functions, nor any of a resource, has nothing to call, and is left out.
pub fn world_members(
    resolve: &Resolve,
    items: &IndexMap<WorldKey, WorldItem>,
) -> Vec<(String, Vec<String>)> {
    items
        .iter()
        .filter_map(|(key, item)| match (key, item) {
            (_, WorldItem::Interface { id, .. })
                if resolve.interfaces[*id].functions.is_empty() =>
            {
                None
            }
            (WorldKey::Interface(_), WorldItem::Interface { id, .. }) => {
                Some((resolve.name_world_key(key), interface_members(resolve, *id)))
            }
            (WorldKey::Name(name), WorldItem::Interface { id, .. }) => Some((
                match resolve.id_of(*id) {
                    Some(aliased) => format!("{name}: {aliased}"),
                    None => format!("{name}: interface"),
                },
                interface_members(resolve, *id),
            )),
            (_, WorldItem::Function(func)) => Some((
                format!("{}: func", func.name),
                vec![func_shape(resolve, func)],
            )),
            (WorldKey::Name(name), WorldItem::Type { id, .. }) => Some((
                format!("{name}: type"),
                vec![type_def_shape(resolve, name, *id)],
            )),
            (WorldKey::Interface(_), _) => None,
        })
        .collect()
}

/// The shapes of an interface's types and functions, a resource's among them, sorted.
fn interface_members(resolve: &Resolve, id: wit_parser::InterfaceId) -> Vec<String> {
    let iface = &resolve.interfaces[id];
    let mut members: Vec<String> = iface
        .types
        .iter()
        .map(|(t, ty)| type_def_shape(resolve, t, *ty))
        .chain(iface.functions.values().map(|f| func_shape(resolve, f)))
        .collect();
    members.sort();
    members
}

/// A function's shape: its name, which for a resource's names the resource, e.g.
/// `[method]session.end`, its modifiers, and the shapes of its params and result.
fn func_shape(resolve: &Resolve, func: &wit_parser::Function) -> String {
    let params: Vec<String> = func
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_shape(resolve, &p.ty)))
        .collect();
    let modifiers: String = crate::wit_html::modifiers(&func.kind)
        .iter()
        .map(|m| format!("{m} "))
        .collect();
    let result = func
        .result
        .as_ref()
        .map(|ty| format!(" -> {}", type_shape(resolve, ty)))
        .unwrap_or_default();
    format!(
        "{}: {modifiers}func({}){result}",
        func.name,
        params.join(", ")
    )
}

/// A named type's shape where it's defined, a world or an inline interface: its name and the
/// structure it names, e.g. `type compass = wasi:x/shapes@0.1.0#direction`.
fn type_def_shape(resolve: &Resolve, name: &str, id: wit_parser::TypeId) -> String {
    match &resolve.types[id].kind {
        // a resource is its name, its functions the scope's
        wit_parser::TypeDefKind::Resource => format!("resource {name}"),
        kind => format!("type {name} = {}", kind_shape(resolve, kind)),
    }
}

/// A type's shape: a type an interface with an id defines is that interface's, e.g.
/// `wasi:io/streams@0.2.6#input-stream`, wherever it's used from; a resource of a world or an
/// inline interface is its name; anything else is its structure.
fn type_shape(resolve: &Resolve, ty: &wit_parser::Type) -> String {
    use wit_parser::Type;
    let id = match ty {
        Type::Id(id) => *id,
        Type::Bool => return "bool".into(),
        Type::U8 => return "u8".into(),
        Type::U16 => return "u16".into(),
        Type::U32 => return "u32".into(),
        Type::U64 => return "u64".into(),
        Type::S8 => return "s8".into(),
        Type::S16 => return "s16".into(),
        Type::S32 => return "s32".into(),
        Type::S64 => return "s64".into(),
        Type::F32 => return "f32".into(),
        Type::F64 => return "f64".into(),
        Type::Char => return "char".into(),
        Type::String => return "string".into(),
        Type::ErrorContext => return "error-context".into(),
    };
    // through aliases and uses, to the type they name
    let id = origin(resolve, id);
    let def = &resolve.types[id];
    if let Some(name) = &def.name {
        if let wit_parser::TypeOwner::Interface(owner) = def.owner
            && let Some(owner) = resolve.id_of(owner)
        {
            return format!("{owner}#{name}");
        }
        if matches!(def.kind, wit_parser::TypeDefKind::Resource) {
            return format!("resource {name}");
        }
    }
    kind_shape(resolve, &def.kind)
}

fn kind_shape(resolve: &Resolve, kind: &wit_parser::TypeDefKind) -> String {
    use wit_parser::{Handle, TypeDefKind};
    let ty = |ty: &wit_parser::Type| type_shape(resolve, ty);
    let opt = |ty: &Option<wit_parser::Type>| {
        ty.as_ref()
            .map(|t| type_shape(resolve, t))
            .unwrap_or_else(|| "_".into())
    };
    match kind {
        TypeDefKind::Record(r) => format!(
            "record {{ {} }}",
            r.fields
                .iter()
                .map(|f| format!("{}: {}", f.name, ty(&f.ty)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDefKind::Resource => "resource".into(),
        TypeDefKind::Handle(Handle::Own(id)) => format!("own<{}>", ty(&wit_parser::Type::Id(*id))),
        TypeDefKind::Handle(Handle::Borrow(id)) => {
            format!("borrow<{}>", ty(&wit_parser::Type::Id(*id)))
        }
        TypeDefKind::Flags(f) => format!(
            "flags {{ {} }}",
            f.flags
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDefKind::Tuple(t) => format!(
            "tuple<{}>",
            t.types.iter().map(ty).collect::<Vec<_>>().join(", ")
        ),
        TypeDefKind::Variant(v) => format!(
            "variant {{ {} }}",
            v.cases
                .iter()
                .map(|c| match &c.ty {
                    Some(t) => format!("{}({})", c.name, ty(t)),
                    None => c.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDefKind::Enum(e) => format!(
            "enum {{ {} }}",
            e.cases
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDefKind::Option(t) => format!("option<{}>", ty(t)),
        TypeDefKind::Result(r) => format!("result<{}, {}>", opt(&r.ok), opt(&r.err)),
        TypeDefKind::List(t) => format!("list<{}>", ty(t)),
        TypeDefKind::Map(k, v) => format!("map<{}, {}>", ty(k), ty(v)),
        TypeDefKind::FixedLengthList(t, n) => format!("list<{}, {n}>", ty(t)),
        TypeDefKind::Future(t) => format!("future<{}>", opt(t)),
        TypeDefKind::Stream(t) => format!("stream<{}>", opt(t)),
        TypeDefKind::Type(t) => ty(t),
        TypeDefKind::Unknown => "unknown".into(),
    }
}

/// The type an alias or a use names, through any others, e.g. the `point` of `wasi:x/shapes` for
/// `use shapes.{point}`.
fn origin(resolve: &Resolve, mut id: wit_parser::TypeId) -> wit_parser::TypeId {
    for _ in 0..64 {
        match resolve.types[id].kind {
            wit_parser::TypeDefKind::Type(wit_parser::Type::Id(next))
                if resolve.types[next].name.is_some() =>
            {
                id = next
            }
            _ => break,
        }
    }
    id
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

/// A component's parts, and for a composition, one with parts that have code of their own, how
/// they're wired together.
pub fn composition(wasm: &[u8]) -> Result<(Vec<crate::data::Part>, Option<crate::data::Wiring>)> {
    let parts = parts(wasm)?;
    let wiring = match parts.iter().any(|p| p.code) {
        true => Some(wiring(wasm)?),
        false => None,
    };
    Ok((parts, wiring))
}

/// How a composition's parts, those of [`parts`], are wired together: what each instance of a part
/// is made with, and what the composition exports, followed through the aliases between them.
pub fn wiring(wasm: &[u8]) -> Result<crate::data::Wiring> {
    use crate::data::{Instance, Owner, Plug, Source, Wiring};
    use wasmparser::{
        CanonicalFunction, ComponentAlias, ComponentExternalKind, ComponentInstance,
        ComponentOuterAliasKind, ComponentTypeRef, Parser, Payload,
    };
    let mut wiring = Wiring::default();
    // the composition's index spaces that wiring goes through: each component, by its part, each
    // instance and each function, by the item it is. `None` for anything else, e.g. a function the
    // composition lifts itself.
    let mut components: Vec<Option<usize>> = Vec::new();
    let mut instances: Vec<Option<Source>> = Vec::new();
    let mut funcs: Vec<Option<Source>> = Vec::new();
    let mut parts = 0;
    // an export of an item, which is only followed one level: an export of an export is `None`
    let export_of = |item: Option<&Option<Source>>, name: &str| match item {
        Some(Some(Source {
            owner,
            export: None,
        })) => Some(Source {
            owner: owner.clone(),
            export: Some(name.to_string()),
        }),
        _ => None,
    };
    let mut depth = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.context("reading how the component's parts are wired")?;
        match payload {
            Payload::Version { .. } => {
                depth += 1;
                continue;
            }
            Payload::End(_) => {
                depth -= 1;
                continue;
            }
            _ if depth != 1 => continue,
            _ => {}
        }
        match payload {
            Payload::ComponentSection { .. } => {
                components.push(Some(parts));
                parts += 1;
            }
            Payload::ComponentImportSection(reader) => {
                for import in reader {
                    let import = import?;
                    let item = Some(Source {
                        owner: Owner::Import(import.name.full_name().into_owned()),
                        export: None,
                    });
                    match import.ty {
                        ComponentTypeRef::Instance(_) => instances.push(item),
                        ComponentTypeRef::Func(_) => funcs.push(item),
                        ComponentTypeRef::Component(_) => components.push(None),
                        _ => {}
                    }
                }
            }
            Payload::ComponentAliasSection(reader) => {
                for alias in reader {
                    match alias? {
                        ComponentAlias::InstanceExport {
                            kind,
                            instance_index,
                            name,
                        } => {
                            let item = export_of(instances.get(instance_index as usize), name);
                            match kind {
                                ComponentExternalKind::Instance => instances.push(item),
                                ComponentExternalKind::Func => funcs.push(item),
                                ComponentExternalKind::Component => components.push(None),
                                _ => {}
                            }
                        }
                        ComponentAlias::Outer {
                            kind: ComponentOuterAliasKind::Component,
                            ..
                        } => components.push(None),
                        _ => {}
                    }
                }
            }
            Payload::ComponentCanonicalSection(reader) => {
                for canonical in reader {
                    if let CanonicalFunction::Lift { .. } = canonical? {
                        funcs.push(None);
                    }
                }
            }
            Payload::ComponentInstanceSection(reader) => {
                for instance in reader {
                    let item = match instance? {
                        ComponentInstance::Instantiate {
                            component_index,
                            args,
                        } => match components.get(component_index as usize) {
                            Some(Some(part)) => {
                                let args = args
                                    .iter()
                                    .filter_map(|arg| {
                                        let item = match arg.kind {
                                            ComponentExternalKind::Instance => {
                                                instances.get(arg.index as usize)
                                            }
                                            ComponentExternalKind::Func => {
                                                funcs.get(arg.index as usize)
                                            }
                                            _ => None,
                                        };
                                        Some(Plug {
                                            name: arg.name.to_string(),
                                            source: item?.clone()?,
                                        })
                                    })
                                    .collect();
                                wiring.instances.push(Instance { part: *part, args });
                                Some(Source {
                                    owner: Owner::Instance(wiring.instances.len() - 1),
                                    export: None,
                                })
                            }
                            _ => None,
                        },
                        ComponentInstance::FromExports(_) => None,
                    };
                    instances.push(item);
                }
            }
            // an export is an item of its own too, the item it exports
            Payload::ComponentExportSection(reader) => {
                for export in reader {
                    let export = export?;
                    let index = export.index as usize;
                    let item = match export.kind {
                        ComponentExternalKind::Instance => {
                            let item = instances.get(index).cloned().flatten();
                            instances.push(item.clone());
                            item
                        }
                        ComponentExternalKind::Func => {
                            let item = funcs.get(index).cloned().flatten();
                            funcs.push(item.clone());
                            item
                        }
                        ComponentExternalKind::Component => {
                            components.push(components.get(index).copied().flatten());
                            None
                        }
                        _ => None,
                    };
                    if let Some(source) = item {
                        wiring.exports.push(Plug {
                            name: export.name.full_name().into_owned(),
                            source,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    Ok(wiring)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_world_items_by_their_structure() {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_str(
                "shapes.wit",
                "package example:shapes@0.1.0;\n\ninterface types {\n  record point { x: f64 }\n}\n\nworld all {\n  import types;\n  import named: types;\n  import inline: interface {\n    use types.{point};\n    f: func(at: point) -> u32;\n  }\n  import log: func(message: string);\n  record pair { a: u8, b: u8 }\n  resource session {\n    end: func();\n  }\n  export run: func(s: borrow<session>) -> result<_, string>;\n}\n\nworld same {\n  import inline: interface {\n    use types.{point};\n    f: func(at: point) -> u32;\n  }\n  import log: func(message: string);\n  record pair { a: u8, b: u8 }\n}\n\nworld renamed {\n  import log: func(msg: string);\n}\n\nworld retyped {\n  import inline: interface {\n    f: func(at: u32) -> u32;\n  }\n  record pair { a: u8, b: u16 }\n}\n",
            )
            .unwrap();
        let shapes = |name: &str| {
            let world = &resolve.worlds[resolve.packages[pkg].worlds[name]];
            let imports: std::collections::BTreeSet<_> =
                world_shapes(&resolve, &world.imports).into_iter().collect();
            let exports: std::collections::BTreeSet<_> =
                world_shapes(&resolve, &world.exports).into_iter().collect();
            (imports, exports)
        };
        let (all, all_exports) = shapes("all");
        // an interface by its id, or by its name and its id, apart
        assert!(all.contains("example:shapes/types@0.1.0"));
        assert!(all.contains("named: example:shapes/types@0.1.0"));
        // a type an interface defines is that interface's, wherever it's used from, an inline
        // interface's members sorted, the types it uses among them
        assert!(all.contains(
            "inline: interface { f: func(at: example:shapes/types@0.1.0#point) -> u32; type point = example:shapes/types@0.1.0#point }"
        ));
        assert!(all.contains("type pair = record { a: u8, b: u8 }"));
        // a world's resource by its name, its functions the world's
        assert!(all.contains("resource session"));
        assert!(all.contains("[method]session.end: func(self: borrow<resource session>)"));
        assert!(
            all_exports.contains("run: func(s: borrow<resource session>) -> result<_, string>")
        );
        // an interface of types alone has nothing to call, and is left out of the members; the
        // other items are in, each with what's in it
        let world = &resolve.worlds[resolve.packages[pkg].worlds["all"]];
        let members: std::collections::BTreeMap<_, _> = world_members(&resolve, &world.imports)
            .into_iter()
            .collect();
        assert_eq!(
            members.keys().collect::<Vec<_>>(),
            [
                "[method]session.end: func",
                "inline: interface",
                "log: func",
                "pair: type",
                "session: type"
            ]
        );
        assert_eq!(members["log: func"], ["log: func(message: string)"]);
        // items structurally identical are alike, whichever world defines them
        assert!(shapes("same").0.is_subset(&all));
        // a param renamed, or a type changed, isn't
        assert!(!shapes("renamed").0.is_subset(&all));
        for shape in shapes("retyped").0 {
            assert!(!all.contains(&shape), "{shape}");
        }
    }

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

    #[test]
    fn follows_the_wiring_of_a_composition() {
        use crate::data::{Instance, Owner, Plug, Source, Wiring};
        use wasm_encoder::{
            Alias, Component, ComponentAliasSection, ComponentExportKind, ComponentExportSection,
            ComponentImportSection, ComponentInstanceSection, ComponentTypeRef,
            ComponentTypeSection, InstanceType, Module, ModuleSection, NestedComponentSection,
        };
        let mut part = Component::new();
        part.section(&ModuleSection(&Module::new()));
        let mut composed = Component::new();
        let mut types = ComponentTypeSection::new();
        types.instance(&InstanceType::new());
        composed.section(&types);
        // instance 0, the host's
        let mut imports = ComponentImportSection::new();
        imports.import("host:a/b", ComponentTypeRef::Instance(0));
        composed.section(&imports);
        composed.section(&NestedComponentSection(&part));
        composed.section(&NestedComponentSection(&part));
        // instance 1, of the first part, and 2, its export `x`
        let mut instances = ComponentInstanceSection::new();
        instances.instantiate(0, Vec::<(&str, ComponentExportKind, u32)>::new());
        composed.section(&instances);
        let mut aliases = ComponentAliasSection::new();
        aliases.alias(Alias::InstanceExport {
            instance: 1,
            kind: ComponentExportKind::Instance,
            name: "x",
        });
        composed.section(&aliases);
        // instance 3, of the second part, with `x` and the host's, and 4, its export `y`
        let mut instances = ComponentInstanceSection::new();
        instances.instantiate(
            1,
            [
                ("x", ComponentExportKind::Instance, 2),
                ("host:a/b", ComponentExportKind::Instance, 0),
            ],
        );
        composed.section(&instances);
        let mut aliases = ComponentAliasSection::new();
        aliases.alias(Alias::InstanceExport {
            instance: 3,
            kind: ComponentExportKind::Instance,
            name: "y",
        });
        composed.section(&aliases);
        let mut exports = ComponentExportSection::new();
        exports.export("y", ComponentExportKind::Instance, 4, None);
        composed.section(&exports);

        let export = |instance, name: &str| Source {
            owner: Owner::Instance(instance),
            export: Some(name.to_string()),
        };
        let plug = |name: &str, source| Plug {
            name: name.to_string(),
            source,
        };
        let (parts, wiring) = composition(&composed.finish()).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(
            wiring.unwrap(),
            Wiring {
                instances: vec![
                    Instance {
                        part: 0,
                        args: vec![]
                    },
                    Instance {
                        part: 1,
                        args: vec![
                            plug("x", export(0, "x")),
                            plug(
                                "host:a/b",
                                Source {
                                    owner: Owner::Import("host:a/b".to_string()),
                                    export: None,
                                }
                            ),
                        ],
                    },
                ],
                exports: vec![plug("y", export(1, "y"))],
            }
        );
        // a component of types alone has no wiring to follow
        assert_eq!(composition(&Component::new().finish()).unwrap().1, None);
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
