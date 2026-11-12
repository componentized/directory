//! Resolves the WIT stored in the index.
//!
//! The index stores each version's main package alone. The foreign packages it uses are resolved
//! from their own indexed versions, which hold the full packages with their docs, not just the
//! parts a component uses. A version with a dependency the index does not hold keeps the foreign
//! packages nested in its WIT, so it still resolves on its own.

use crate::data::Stored;
use anyhow::{Context, Result, anyhow};
use std::collections::{HashMap, HashSet};
use wit_component::WitPrinter;
use wit_parser::{
    AstItem, IndexMap, InterfaceId, PackageId, Resolve, UnresolvedPackageGroup, WorldItem, WorldKey,
};

pub struct Loader<'a> {
    stored: &'a [Stored],
    /// The stored versions by id, e.g. `wasi:logging@0.1.0-draft`.
    by_id: HashMap<String, usize>,
}

impl<'a> Loader<'a> {
    pub fn new(stored: &'a [Stored]) -> Self {
        let by_id = stored.iter().enumerate().map(|(i, s)| (id(s), i)).collect();
        Self { stored, by_id }
    }

    /// Whether every dependency of a stored version is indexed.
    pub fn dependencies_indexed(&self, index: usize) -> bool {
        self.stored[index]
            .metadata
            .dependencies
            .iter()
            .all(|dep| self.by_id.contains_key(&dep.id()))
    }

    /// Resolves a stored version with the packages it depends on.
    pub fn resolve(&self, index: usize) -> Result<(Resolve, PackageId)> {
        let stored = &self.stored[index];
        let path = stored.dir.join(stored.metadata.kind.wit_file());
        let mut resolve = new_resolve();
        let pkg = if has_nested(&stored.wit) {
            resolve
                .push_str(&path, &stored.wit)
                .with_context(|| format!("parsing {}", path.display()))?
        } else {
            let mut pushed = HashSet::new();
            self.push(&mut resolve, &mut pushed, index, &mut Vec::new())
                .with_context(|| format!("resolving {}", path.display()))?
        };
        as_written(&mut resolve, pkg, &stored.wit)
            .with_context(|| format!("reading the worlds of {}", path.display()))?;
        Ok((resolve, pkg))
    }

    /// Pushes a stored version after the versions it depends on.
    fn push(
        &self,
        resolve: &mut Resolve,
        pushed: &mut HashSet<usize>,
        index: usize,
        path: &mut Vec<usize>,
    ) -> Result<PackageId> {
        if path.contains(&index) {
            return Err(anyhow!(
                "a dependency cycle through {}",
                id(&self.stored[index])
            ));
        }
        path.push(index);
        let stored = &self.stored[index];
        for dep in &stored.metadata.dependencies {
            let dep_id = dep.id();
            let dep_index = *self
                .by_id
                .get(&dep_id)
                .ok_or_else(|| anyhow!("the dependency {dep_id} is not indexed"))?;
            if !pushed.contains(&dep_index) {
                self.push(resolve, pushed, dep_index, path)?;
            }
        }
        path.pop();
        let file = stored.dir.join(stored.metadata.kind.wit_file());
        let pkg = resolve
            .push_str(&file, &stored.wit)
            .with_context(|| format!("parsing {}", file.display()))?;
        pushed.insert(index);
        Ok(pkg)
    }
}

/// Removes the interfaces that resolving adds to the worlds of a package, leaving the worlds as
/// written. Resolving a world imports each interface the interfaces it imports or exports use,
/// e.g. a world importing `wasi:clocks/system-clock@0.3.0` also imports `wasi:clocks/types@0.3.0`.
/// Those interfaces come from the full packages in the index, while a component imports only
/// those the parts it uses need.
fn as_written(resolve: &mut Resolve, pkg: PackageId, wit: &str) -> Result<()> {
    let group =
        UnresolvedPackageGroup::parse("written.wit", wit).map_err(|(_, err)| anyhow!("{err}"))?;
    let main = &group.main;
    // the id of each interface the unresolved package names, e.g. `wasi:clocks/types@0.3.0`
    let mut ids: HashMap<InterfaceId, String> = HashMap::new();
    for (id, iface) in main.interfaces.iter() {
        if let Some(name) = &iface.name {
            ids.insert(id, main.name.interface_id(name));
        }
    }
    for (package, items) in &main.foreign_deps {
        for (name, (item, _)) in items {
            if let AstItem::Interface(id) = item {
                ids.insert(*id, package.interface_id(name));
            }
        }
    }
    let written = |items: &IndexMap<WorldKey, WorldItem>| -> HashSet<String> {
        items
            .keys()
            .filter_map(|key| match key {
                WorldKey::Interface(id) => ids.get(id).cloned(),
                WorldKey::Name(_) => None,
            })
            .collect()
    };
    let mut worlds: HashMap<String, (HashSet<String>, HashSet<String>)> = HashMap::new();
    for (_, world) in main.worlds.iter() {
        worlds.insert(
            world.name.clone(),
            (written(&world.imports), written(&world.exports)),
        );
    }

    let world_ids: Vec<_> = resolve.packages[pkg].worlds.values().copied().collect();
    for world_id in world_ids {
        let Some((imports, exports)) = worlds.get(&resolve.worlds[world_id].name) else {
            continue;
        };
        let keep = |resolve: &Resolve, key: &WorldKey, written: &HashSet<String>| match key {
            WorldKey::Interface(id) => resolve.id_of(*id).is_none_or(|id| written.contains(&id)),
            WorldKey::Name(_) => true,
        };
        let world = &resolve.worlds[world_id];
        let import_keys: Vec<_> = world
            .imports
            .keys()
            .filter(|key| !keep(resolve, key, imports))
            .cloned()
            .collect();
        let export_keys: Vec<_> = world
            .exports
            .keys()
            .filter(|key| !keep(resolve, key, exports))
            .cloned()
            .collect();
        let world = &mut resolve.worlds[world_id];
        for key in import_keys {
            world.imports.shift_remove(&key);
        }
        for key in export_keys {
            world.exports.shift_remove(&key);
        }
    }
    Ok(())
}

fn id(stored: &Stored) -> String {
    format!("{}@{}", stored.metadata.package, stored.metadata.version)
}

/// A resolve that keeps the items of every feature gate, `@unstable(feature = ..)`. Items behind a
/// gate that is not enabled are dropped while parsing.
pub fn new_resolve() -> Resolve {
    Resolve {
        all_features: true,
        ..Resolve::default()
    }
}

/// Whether WIT nests the foreign packages it uses after its main package.
pub fn has_nested(wit: &str) -> bool {
    let mut resolve = new_resolve();
    resolve.push_str("check.wit", wit).is_ok() && resolve.packages.len() > 1
}

/// The main package of WIT that nests the foreign packages it uses, without them.
pub fn main_only(wit: &str) -> Result<String> {
    let mut resolve = new_resolve();
    let pkg = resolve.push_str("main.wit", wit)?;
    let mut printer = WitPrinter::default();
    printer.print(&resolve, pkg, &[])?;
    Ok(printer.output.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NESTED: &str = r#"
package root:component;

world root {
  import wasi:logging/logging@0.1.0-draft;
}

package wasi:logging@0.1.0-draft {
  interface logging {
    log: func(message: string);
  }
}
"#;

    #[test]
    fn worlds_as_written() {
        let nested = r#"
package root:component;

world root {
  import wasi:clocks/system-clock@0.3.0;
}

package wasi:clocks@0.3.0 {
  interface types {
    type duration = u64;
  }
  interface system-clock {
    use types.{duration};
    now: func() -> duration;
  }
}
"#;
        let mut resolve = Resolve::default();
        let pkg = resolve.push_str("world.wit", nested).unwrap();
        let world = resolve.select_world(&[pkg], None).unwrap();
        // resolving imports the interface system-clock uses
        assert_eq!(resolve.worlds[world].imports.len(), 2);
        as_written(&mut resolve, pkg, nested).unwrap();
        let imports: Vec<_> = resolve.worlds[world]
            .imports
            .keys()
            .map(|key| resolve.name_world_key(key))
            .collect();
        assert_eq!(imports, ["wasi:clocks/system-clock@0.3.0"]);
    }

    #[test]
    fn keeps_feature_gated_items() {
        let wit = "package a:b@1.0.0;\n\ninterface i {\n  @since(version = 1.0.0)\n  f: func();\n\n  @unstable(feature = x)\n  g: func();\n}\n";
        let main = main_only(wit).unwrap();
        assert!(main.contains("@unstable(feature = x)"), "{main}");
        assert!(main.contains("g: func();"), "{main}");
    }

    #[test]
    fn strips_nested_packages() {
        assert!(has_nested(NESTED));
        let main = main_only(NESTED).unwrap();
        assert!(!has_nested(&main));
        assert!(main.contains("import wasi:logging/logging@0.1.0-draft;"));
        assert!(!main.contains("log: func"));
    }
}
