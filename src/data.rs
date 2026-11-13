//! The committed index: `./{namespace}/{name}/{version}/` with `metadata.json` and the WIT.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A WIT package, published as an encoded `interface.wasm`.
    Interface,
    /// A component.
    Component,
}

impl Kind {
    pub fn wit_file(self) -> &'static str {
        match self {
            Kind::Interface => "package.wit",
            Kind::Component => "world.wit",
        }
    }
}

/// Why a version is in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// A stable release found by scanning the org's packages.
    Release,
    /// A version a release depends on, e.g. a WASI pre-release.
    Dependency,
    /// A version of a package indexed in full, as `.config/directory/tracked.toml` lists, e.g.
    /// each release of `wasi:http`, whether or not a release depends on it.
    Tracked,
    /// A sample made up to show how each WIT feature renders, e.g. the `kitchen-sink`
    /// namespace. It's in `samples/`, never the index, and isn't published to any registry.
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OciRef {
    pub registry: String,
    pub repository: String,
    pub tag: String,
    /// The manifest digest.
    pub digest: String,
    /// The digest of the wasm layer.
    pub layer_digest: String,
    /// The size of the wasm layer in bytes.
    pub size: u64,
}

impl OciRef {
    pub fn reference(&self) -> String {
        format!("{}/{}:{}", self.registry, self.repository, self.tag)
    }

    pub fn pinned(&self) -> String {
        format!("{}/{}@{}", self.registry, self.repository, self.digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Dependency {
    pub package: String,
    pub version: Option<String>,
}

impl Dependency {
    pub fn id(&self) -> String {
        match &self.version {
            Some(version) => format!("{}@{version}", self.package),
            None => self.package.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// The package id, e.g. `componentized:logging` or `componentized:logging-levels`.
    pub package: String,
    pub version: String,
    pub kind: Kind,
    pub origin: Origin,
    pub oci: OciRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<OciRef>,
    /// When the artifact was created, from the OCI config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    /// The interfaces a component imports, e.g. `wasi:logging/logging@0.1.0-draft`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    /// The interfaces a component exports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,
    /// The GitHub release of the version in its source repository, tagged with the version, e.g.
    /// `https://github.com/componentized/logging/releases/tag/v0.3.1`. A scan looks for it until
    /// it's found, as a release can be published after its packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub indexed_at: String,
}

impl Metadata {
    pub fn annotation(&self, key: &str) -> Option<&str> {
        self.annotations
            .get(&format!("org.opencontainers.image.{key}"))
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
}

/// Splits a package id into its namespace and name, e.g. `wasi:logging`.
pub fn split_id(package: &str) -> Option<(&str, &str)> {
    package.split_once(':')
}

pub fn version_dir(root: &Path, package: &str, version: &str) -> PathBuf {
    let (namespace, name) = split_id(package).unwrap_or(("_", package));
    root.join(namespace).join(name).join(version)
}

pub fn exists(root: &Path, package: &str, version: &str) -> bool {
    version_dir(root, package, version)
        .join("metadata.json")
        .is_file()
}

pub fn read(root: &Path, package: &str, version: &str) -> Option<Metadata> {
    let path = version_dir(root, package, version).join("metadata.json");
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn write(root: &Path, metadata: &Metadata, wit: &str) -> Result<PathBuf> {
    let dir = version_dir(root, &metadata.package, &metadata.version);
    std::fs::create_dir_all(&dir)?;
    let mut json = serde_json::to_string_pretty(metadata)?;
    json.push('\n');
    std::fs::write(dir.join("metadata.json"), json)?;
    let mut wit = wit.to_string();
    if !wit.ends_with('\n') {
        wit.push('\n');
    }
    std::fs::write(dir.join(metadata.kind.wit_file()), wit)?;
    Ok(dir)
}

/// An indexed version, as committed.
pub struct Stored {
    pub metadata: Metadata,
    pub wit: String,
    pub dir: PathBuf,
}

/// Reads every `{namespace}/{name}/{version}/metadata.json` under the root.
pub fn load(root: &Path) -> Result<Vec<Stored>> {
    let mut stored = Vec::new();
    // a first scan starts from an empty index
    if !root.exists() {
        return Ok(stored);
    }
    for namespace in subdirs(root)? {
        for name in subdirs(&namespace)? {
            for version in subdirs(&name)? {
                let path = version.join("metadata.json");
                if !path.is_file() {
                    continue;
                }
                let metadata: Metadata = serde_json::from_str(&std::fs::read_to_string(&path)?)
                    .with_context(|| format!("parsing {}", path.display()))?;
                let wit_path = version.join(metadata.kind.wit_file());
                let wit = std::fs::read_to_string(&wit_path)
                    .with_context(|| format!("reading {}", wit_path.display()))?;
                stored.push(Stored {
                    metadata,
                    wit,
                    dir: version,
                });
            }
        }
    }
    stored.sort_by(|a, b| {
        a.metadata
            .package
            .cmp(&b.metadata.package)
            .then_with(|| crate::version::cmp_desc(&a.metadata.version, &b.metadata.version))
    });
    Ok(stored)
}

fn subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}
