//! The wkg config, `.config/wasm-pkg/config.toml`: which registry serves each WIT namespace.
//!
//! A registry is resolved the way wkg resolves it: a package override, then the package's
//! namespace, then the default registry. A mapping is either a registry host, whose
//! `/.well-known/wasm-pkg/registry.json` locates the packages, or a host with the registry
//! metadata inline.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_registry: Option<String>,
    #[serde(default)]
    pub namespace_registries: HashMap<String, Mapping>,
    #[serde(default)]
    pub package_registry_overrides: HashMap<String, Mapping>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Mapping {
    /// A registry host, located through its well-known registry metadata.
    Registry(String),
    /// A registry host with its metadata, e.g.
    /// `{ registry = "example.com", metadata = { oci = { registry = "ghcr.io" } } }`.
    Custom {
        registry: String,
        metadata: serde_json::Value,
    },
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let source =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&source).with_context(|| format!("parsing {}", path.display()))
    }

    /// The registry mapping of a package, e.g. `wasi:logging`.
    pub fn resolve(&self, package: &str) -> Option<Mapping> {
        let namespace = package.split_once(':').map_or(package, |(ns, _)| ns);
        self.package_registry_overrides
            .get(package)
            .or_else(|| self.namespace_registries.get(namespace))
            .cloned()
            .or_else(|| self.default_registry.clone().map(Mapping::Registry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_order() {
        let config: Config = toml::from_str(
            r#"
            default_registry = "default.example"

            [namespace_registries]
            wasi = "wasi.dev"
            custom = { registry = "custom.example", metadata = { oci = { registry = "ghcr.io", namespacePrefix = "custom/" } } }

            [package_registry_overrides]
            "wasi:special" = "special.example"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.resolve("wasi:logging"),
            Some(Mapping::Registry("wasi.dev".to_string()))
        );
        assert_eq!(
            config.resolve("wasi:special"),
            Some(Mapping::Registry("special.example".to_string()))
        );
        assert_eq!(
            config.resolve("other:thing"),
            Some(Mapping::Registry("default.example".to_string()))
        );
        assert!(matches!(
            config.resolve("custom:thing"),
            Some(Mapping::Custom { registry, .. }) if registry == "custom.example"
        ));
        assert_eq!(Config::default().resolve("wasi:logging"), None);
    }

    #[test]
    fn repository_config() {
        let config = Config::load(Path::new(".config/wasm-pkg/config.toml")).unwrap();
        for namespace in ["wasi", "componentized", "wasmcloud"] {
            assert!(
                config.resolve(&format!("{namespace}:x")).is_some(),
                "{namespace}"
            );
        }
    }
}
