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

/// The WIT packages indexed in full, `.config/directory/tracked.toml`: each of their releases,
/// beyond the versions the org's releases depend on, e.g. `wasi:http`.
///
/// ```toml
/// packages = ["wasi:cli", "wasi:http"]
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct Tracked {
    #[serde(default)]
    pub packages: Vec<String>,
}

impl Tracked {
    /// The packages at `path`, none when there's no file.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(source) => {
                toml::from_str(&source).with_context(|| format!("parsing {}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }
}

/// Corrections to what packages' published images say of themselves, `.config/directory/sources.toml`,
/// for an image that names the wrong source repository, e.g. one its publishing workflow filled in:
///
/// ```toml
/// [packages."wasi:config"]
/// source = "https://github.com/webassembly/wasi-config"
/// ```
///
/// The index keeps what the registry says; a correction applies as the index is read.
#[derive(Debug, Default, Deserialize)]
pub struct Sources {
    #[serde(default)]
    pub packages: HashMap<String, PackageSource>,
}

#[derive(Debug, Deserialize)]
pub struct PackageSource {
    /// The repository the package is built from, its `org.opencontainers.image.source`.
    pub source: String,
}

impl Sources {
    /// The corrections at `path`, none when there's no file.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(source) => {
                toml::from_str(&source).with_context(|| format!("parsing {}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Corrects the source repository of the stored versions of each package it names.
    pub fn apply(&self, stored: &mut [crate::data::Stored]) {
        for stored in stored {
            if let Some(correction) = self.packages.get(&stored.metadata.package) {
                stored.metadata.annotations.insert(
                    "org.opencontainers.image.source".to_string(),
                    correction.source.clone(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrects_sources() {
        let sources: Sources = toml::from_str(
            "[packages.\"wasi:config\"]\nsource = \"https://github.com/webassembly/wasi-config\"\n",
        )
        .unwrap();
        assert_eq!(
            sources.packages["wasi:config"].source,
            "https://github.com/webassembly/wasi-config"
        );
        let missing = Sources::load(Path::new("/nonexistent/sources.toml")).unwrap();
        assert!(missing.packages.is_empty());
    }

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
