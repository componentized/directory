//! Finds new releases in the registry and commits their WIT and metadata to the index.

use crate::config::{Config, Mapping};
use crate::data::{self, Metadata, OciRef, Origin};
use crate::decode;
use crate::http::{self, GitHub, Oci, OciLocation};
use crate::resolve::{self, Loader};
use crate::version::{self, Tag};
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

pub struct Options {
    /// The root of the index.
    pub root: PathBuf,
    /// The GitHub org, also the WIT namespace and the ghcr.io namespace, e.g. `componentized`.
    pub org: String,
    pub github_token: Option<String>,
    /// Packages to scan in addition to the discovered ones, e.g. `logging/levels`.
    pub packages: Vec<String>,
    /// Only scan packages that start with one of these, e.g. `logging`.
    pub only: Vec<String>,
    /// Only scan this version of the packages, e.g. `0.3.1`.
    pub version: Option<String>,
    /// The registry of each WIT namespace, from the wkg config.
    pub registries: Config,
    /// Corrections to the source repositories packages' images name.
    pub sources: crate::config::Sources,
    /// The WIT packages indexed in full, each of their releases, e.g. `wasi:http`.
    pub tracked: crate::config::Tracked,
}

const REGISTRY: &str = "ghcr.io";

pub fn run(options: &Options) -> Result<()> {
    let client = http::client()?;
    let oci = Oci::new(client.clone());
    let github = GitHub::new(client.clone(), options.github_token.clone());
    let mut scanner = Scanner {
        options,
        oci,
        client,
        locations: HashMap::new(),
        added: Vec::new(),
        failed: Vec::new(),
    };

    let mut packages = discover(&github, &options.org, &options.only)?;
    packages.extend(options.packages.iter().cloned());
    packages.retain(|package| {
        options.only.is_empty()
            || options
                .only
                .iter()
                .any(|only| package == only || package.starts_with(&format!("{only}/")))
    });
    eprintln!(
        "scanning {} packages under {REGISTRY}/{}{}",
        packages.len(),
        options.org,
        match &options.version {
            Some(version) => format!(", only {version}"),
            None => String::new(),
        }
    );
    for package in &packages {
        if let Err(err) = scanner.scan_package(package) {
            scanner.fail(format!("{}/{package}", options.org), err);
        }
    }
    // a partial scan, e.g. of a repository that just published, leaves them to the next full scan
    if options.only.is_empty() {
        for package in &options.tracked.packages {
            if let Err(err) = scanner.track(package) {
                scanner.fail(package.clone(), err);
            }
        }
    }
    scanner.dependencies()?;
    trim_nested(&options.root)?;
    refresh_items(&options.root)?;
    if let Err(err) = refresh_releases(&options.root, &github, &options.sources) {
        scanner.fail("the GitHub releases".to_string(), err);
    }

    eprintln!("indexed {} new versions", scanner.added.len());
    for added in &scanner.added {
        println!("{added}");
    }
    for (what, err) in &scanner.failed {
        println!("::warning::{what}: {err:#}");
    }
    Ok(())
}

/// Links each stable version to the GitHub release of its source repository tagged with the
/// version, `v0.3.1` or else `0.3.1`. A repository's releases are listed once, and only while one
/// of its stable versions has no release recorded: a release can be published after its packages
/// are indexed. Dev versions have no releases. A repository that isn't on GitHub, or whose releases
/// can't be listed, e.g. without a token past the rate limit, is skipped.
///
/// A version that records the commit it was built from is linked only when the release's tag is
/// that commit: a repository can tag versions of something else, e.g. `wasmCloud/wasmCloud` tags
/// its host's versions, not those of the `wasmcloud:keyvalue` package it publishes.
fn refresh_releases(
    root: &std::path::Path,
    github: &GitHub,
    sources: &crate::config::Sources,
) -> Result<()> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        draft: bool,
    }
    #[derive(Deserialize)]
    struct Commit {
        sha: String,
    }

    // the source a correction names is where the releases are
    let mut stored = data::load(root)?;
    sources.apply(&mut stored);
    // the versions still without a release, by the repository they're built from
    let mut pending: std::collections::BTreeMap<String, Vec<&data::Stored>> = Default::default();
    for stored in &stored {
        let meta = &stored.metadata;
        if meta.release.is_some() || version::stable(&meta.version).is_none() {
            continue;
        }
        let Some(repo) = meta.annotation("source").and_then(github_repo) else {
            continue;
        };
        pending.entry(repo).or_default().push(stored);
    }
    let mut linked = 0;
    for (repo, versions) in pending {
        let Some(releases) = github.list::<Release>(&format!("/repos/{repo}/releases"))? else {
            continue;
        };
        let tags: HashMap<&str, &str> = releases
            .iter()
            .filter(|release| !release.draft)
            .map(|release| (release.tag_name.as_str(), release.html_url.as_str()))
            .collect();
        for stored in versions {
            let version = &stored.metadata.version;
            let tagged = format!("v{version}");
            let Some((tag, url)) = [tagged.as_str(), version.as_str()]
                .into_iter()
                .find_map(|tag| tags.get(tag).map(|url| (tag, *url)))
            else {
                continue;
            };
            if let Some(revision) = stored.metadata.annotation("revision") {
                let revision = revision.trim_end_matches("+dirty");
                let commit = github.get::<Commit>(&format!("/repos/{repo}/commits/{tag}"))?;
                if !commit.is_some_and(|commit| commit.sha.starts_with(revision)) {
                    continue;
                }
            }
            let mut metadata = stored.metadata.clone();
            metadata.release = Some(url.to_string());
            data::write(root, &metadata, &stored.wit)?;
            linked += 1;
        }
    }
    if linked > 0 {
        eprintln!("linked {linked} indexed versions to their GitHub releases");
    }
    Ok(())
}

/// The `owner/repo` of a source repository on GitHub, e.g. `componentized/logging` for
/// `https://github.com/componentized/logging.git`.
fn github_repo(source: &str) -> Option<String> {
    let path = source
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://github.com/")?;
    let (owner, repo) = path.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .then(|| format!("{owner}/{repo}"))
}

/// Re-derives the imports and exports of the indexed components from their stored WIT, updating
/// metadata written by an older scan, e.g. before named imports recorded the interface they alias.
fn refresh_items(root: &std::path::Path) -> Result<()> {
    let stored = data::load(root)?;
    let loader = Loader::new(&stored);
    let mut refreshed = 0;
    for (index, stored) in stored.iter().enumerate() {
        if stored.metadata.kind != data::Kind::Component {
            continue;
        }
        let (resolve, pkg) = loader.resolve(index)?;
        let (imports, exports) = decode::component_items(&resolve, pkg)
            .with_context(|| format!("reading {}", stored.dir.display()))?;
        if imports != stored.metadata.imports || exports != stored.metadata.exports {
            let mut metadata = stored.metadata.clone();
            metadata.imports = imports;
            metadata.exports = exports;
            data::write(root, &metadata, &stored.wit)?;
            refreshed += 1;
        }
    }
    if refreshed > 0 {
        eprintln!("refreshed the imports and exports of {refreshed} indexed versions");
    }
    Ok(())
}

/// Reduces the stored WIT of each version to its main package. A decoded component or package
/// nests the parts of the foreign packages it uses, the index holds those packages in full, with
/// their docs. A version keeps its nested packages while a package it depends on is not indexed,
/// so it still resolves on its own.
fn trim_nested(root: &std::path::Path) -> Result<()> {
    let stored = data::load(root)?;
    let loader = Loader::new(&stored);
    let mut trimmed = Vec::new();
    for (index, stored) in stored.iter().enumerate() {
        if !resolve::has_nested(&stored.wit) {
            continue;
        }
        if !loader.dependencies_indexed(index) {
            eprintln!(
                "note: {} keeps its nested packages, a dependency is not indexed",
                stored
                    .dir
                    .strip_prefix(root)
                    .unwrap_or(&stored.dir)
                    .display()
            );
            continue;
        }
        let main = resolve::main_only(&stored.wit)
            .with_context(|| format!("trimming {}", stored.dir.display()))?;
        data::write(root, &stored.metadata, &main)?;
        trimmed.push((
            stored.metadata.package.clone(),
            stored.metadata.version.clone(),
            stored.wit.clone(),
        ));
    }
    // restore the versions that do not resolve from the index, e.g. a dependency itself depends
    // on a package that is not indexed
    loop {
        let stored = data::load(root)?;
        let loader = Loader::new(&stored);
        let mut restored = 0;
        trimmed.retain(|(package, version, wit)| {
            let Some(index) = stored
                .iter()
                .position(|s| s.metadata.package == *package && s.metadata.version == *version)
            else {
                return false;
            };
            match loader.resolve(index) {
                Ok(_) => true,
                Err(err) => {
                    eprintln!("note: {package}@{version} keeps its nested packages: {err:#}");
                    let _ = data::write(root, &stored[index].metadata, wit);
                    restored += 1;
                    false
                }
            }
        });
        if restored == 0 {
            break;
        }
    }
    if !trimmed.is_empty() {
        eprintln!(
            "reduced the WIT of {} indexed versions to their main package",
            trimmed.len()
        );
    }
    Ok(())
}

/// Lists the container packages of the org: a package for each of its public repositories, and one
/// for each of their `components/*` directories. The GitHub Packages API would list them directly,
/// but it only accepts a personal access token (classic), not a workflow's or an app's token.
/// Given `only`, just those repositories are looked up, rather than listing every repository of the
/// org, e.g. for a scan a repository requests once it publishes.
fn discover(github: &GitHub, org: &str, only: &[String]) -> Result<BTreeSet<String>> {
    #[derive(Deserialize)]
    struct Repo {
        name: String,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        fork: bool,
        #[serde(default)]
        private: bool,
    }
    #[derive(Deserialize)]
    struct Content {
        name: String,
        #[serde(rename = "type")]
        kind: String,
    }

    let mut found = BTreeSet::new();
    let repos = if only.is_empty() {
        github
            .list::<Repo>(&format!("/orgs/{org}/repos?type=public"))?
            .ok_or_else(|| anyhow!("listing the repositories of {org}"))?
    } else {
        let mut repos = Vec::new();
        for name in only {
            // `--only` names a repository, its packages are those it publishes, e.g. `logging`
            let name = name.split('/').next().unwrap_or(name);
            match github.get::<Repo>(&format!("/repos/{org}/{name}"))? {
                Some(repo) => repos.push(repo),
                None => eprintln!("warning: {org}/{name} is not a repository"),
            }
        }
        repos
    };
    for repo in repos
        .iter()
        .filter(|r| !r.archived && !r.fork && !r.private)
    {
        found.insert(repo.name.clone());
        let contents =
            github.list::<Content>(&format!("/repos/{org}/{}/contents/components", repo.name))?;
        for content in contents.unwrap_or_default() {
            let internal = ["dep-", "internal-", "test-"]
                .iter()
                .any(|prefix| content.name.starts_with(prefix));
            if content.kind == "dir" && !internal {
                found.insert(format!("{}/{}", repo.name, content.name));
            }
        }
    }
    Ok(found)
}

struct Scanner<'a> {
    options: &'a Options,
    oci: Oci,
    client: reqwest::blocking::Client,
    locations: HashMap<String, std::result::Result<OciLocation, String>>,
    added: Vec<String>,
    failed: Vec<(String, anyhow::Error)>,
}

impl Scanner<'_> {
    fn fail(&mut self, what: String, err: anyhow::Error) {
        eprintln!("warning: {what}: {err:#}");
        self.failed.push((what, err));
    }

    /// Indexes the new stable releases of a package of the org, e.g. `logging` or `logging/levels`.
    fn scan_package(&mut self, package: &str) -> Result<()> {
        let org = &self.options.org;
        let repository = format!("{org}/{package}");
        let Some(tags) = self.oci.tags(REGISTRY, &repository)? else {
            return Ok(());
        };
        let debug: BTreeSet<_> = tags
            .iter()
            .filter_map(|tag| match version::classify(tag) {
                Tag::Debug(version) => Some(version.to_string()),
                _ => None,
            })
            .collect();
        // a component of a repository is a package of its own, e.g. `componentized:logging-levels`
        let package_id = format!("{org}:{}", package.replace('/', "-"));
        let releases: BTreeSet<_> = tags
            .iter()
            .filter(|tag| matches!(version::classify(tag), Tag::Release(_)))
            .collect();
        for tag in &tags {
            let version = match version::classify(tag) {
                Tag::Release(version) => version.to_string(),
                Tag::Dev(version) => version.to_string(),
                _ => continue,
            };
            if self
                .options
                .version
                .as_ref()
                .is_some_and(|only| *only != version)
            {
                continue;
            }
            let mut refresh = false;
            match version::release_of(&version) {
                // a release is immutable, index it once
                None => {
                    if data::exists(&self.options.root, &package_id, &version) {
                        continue;
                    }
                }
                // a dev tag moves with each build, until its release is published
                Some(release) => {
                    if let Some(stored) = data::read(&self.options.root, &package_id, &version) {
                        if releases.contains(&release) {
                            continue;
                        }
                        let (digest, _) = match self.oci.manifest(REGISTRY, &repository, tag) {
                            Ok(manifest) => manifest,
                            Err(err) => {
                                self.fail(format!("{REGISTRY}/{repository}:{tag}"), err);
                                continue;
                            }
                        };
                        if digest == stored.oci.digest {
                            continue;
                        }
                        refresh = true;
                    }
                }
            }
            let debug_tag = debug.contains(&version).then(|| format!("{version}_debug"));
            let location = OciLocation {
                registry: REGISTRY.to_string(),
                namespace_prefix: String::new(),
            };
            if let Err(err) = self.index(
                &location,
                &repository,
                tag,
                &package_id,
                debug_tag.as_deref(),
                Origin::Release,
                refresh,
            ) {
                self.fail(format!("{REGISTRY}/{repository}:{tag}"), err);
            }
        }
        Ok(())
    }

    /// Indexes the versions the index depends on that are not indexed yet, until every
    /// dependency is indexed or failed. These are often pre-releases, e.g. `wasi:logging@0.1.0-draft`.
    fn dependencies(&mut self) -> Result<()> {
        let mut attempted = BTreeSet::new();
        loop {
            let missing: BTreeSet<_> = data::load(&self.options.root)?
                .into_iter()
                .flat_map(|stored| stored.metadata.dependencies)
                .filter_map(|dep| Some((dep.package.clone(), dep.version.clone()?)))
                .filter(|(package, version)| !data::exists(&self.options.root, package, version))
                .filter(|dep| !attempted.contains(dep))
                .collect();
            if missing.is_empty() {
                return Ok(());
            }
            for (package, version) in missing {
                attempted.insert((package.clone(), version.clone()));
                if let Err(err) = self.dependency(&package, &version) {
                    self.fail(format!("{package}@{version}"), err);
                }
            }
        }
    }

    fn dependency(&mut self, package: &str, version: &str) -> Result<()> {
        let (namespace, name) =
            data::split_id(package).ok_or_else(|| anyhow!("invalid package id"))?;
        let location = self.location(package)?;
        let repository = location.repository(namespace, name);
        self.index(
            &location,
            &repository,
            version,
            package,
            None,
            Origin::Dependency,
            false,
        )
    }

    /// Indexes each release of a tracked package that isn't indexed yet, e.g. every `wasi:http`
    /// 0.2.x and 0.3.x, and its pre-releases of the versions it never released, e.g.
    /// `wasi:config@0.2.0-draft`, but not the release candidates of a version since released.
    fn track(&mut self, package: &str) -> Result<()> {
        let (namespace, name) =
            data::split_id(package).ok_or_else(|| anyhow!("invalid package id {package}"))?;
        let location = self.location(package)?;
        let repository = location.repository(namespace, name);
        let Some(tags) = self.oci.tags(&location.registry, &repository)? else {
            return Ok(());
        };
        let released: BTreeSet<String> = tags
            .iter()
            .filter_map(|tag| version::stable(tag))
            .map(|v| v.to_string())
            .collect();
        for tag in &tags {
            let wanted = version::stable(tag).is_some()
                || version::prerelease(tag).is_some_and(|v| {
                    !released.contains(&format!("{}.{}.{}", v.major, v.minor, v.patch))
                });
            if !wanted || data::exists(&self.options.root, package, tag) {
                continue;
            }
            if let Err(err) = self.index(
                &location,
                &repository,
                tag,
                package,
                None,
                Origin::Tracked,
                false,
            ) {
                self.fail(format!("{package}@{tag}"), err);
            }
        }
        Ok(())
    }

    /// Where a package is published, from the registry the wkg config maps it to.
    fn location(&mut self, package: &str) -> Result<OciLocation> {
        let mapping = self
            .options
            .registries
            .resolve(package)
            .ok_or_else(|| anyhow!("no registry for {package} in the wkg config"))?;
        match mapping {
            Mapping::Custom { registry, metadata } => http::oci_location(metadata)
                .with_context(|| format!("reading the metadata of {registry} in the wkg config")),
            Mapping::Registry(host) => {
                if !self.locations.contains_key(&host) {
                    let location =
                        http::well_known(&self.client, &host).map_err(|err| format!("{err:#}"));
                    self.locations.insert(host.clone(), location);
                }
                self.locations[&host].clone().map_err(|err| anyhow!(err))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn index(
        &mut self,
        location: &OciLocation,
        repository: &str,
        tag: &str,
        package_id: &str,
        debug_tag: Option<&str>,
        origin: Origin,
        // replace an indexed version, e.g. a dev build that was rebuilt
        refresh: bool,
    ) -> Result<()> {
        let registry = &location.registry;
        let (oci, manifest) = self.fetch(registry, repository, tag)?;
        let wasm = self
            .oci
            .blob(registry, repository, &oci.layer_digest)
            .context("fetching the wasm layer")?;
        let decoded = decode::decode(&wasm)?;

        let mut package = package_id.to_string();
        let mut version = tag.to_string();
        if let Some((name, decoded_version)) = &decoded.package {
            if *name != package {
                eprintln!("note: {registry}/{repository}:{tag} is the WIT package {name}");
                package = name.clone();
            }
            if let Some(decoded_version) = decoded_version
                && *decoded_version != version
            {
                eprintln!(
                    "note: {registry}/{repository}:{tag} is version {decoded_version} of {name}"
                );
                version = decoded_version.clone();
            }
        }
        if !refresh && data::exists(&self.options.root, &package, &version) {
            return Ok(());
        }

        let created = match &manifest.config {
            Some(config) if config.size < 64 * 1024 => self
                .oci
                .blob(registry, repository, &config.digest)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<http::WasmConfig>(&bytes).ok())
                .and_then(|config| config.created),
            _ => None,
        };
        let debug = match debug_tag {
            Some(debug_tag) => match self.fetch(registry, repository, debug_tag) {
                Ok((debug, _)) => Some(debug),
                Err(err) => {
                    eprintln!("warning: {registry}/{repository}:{debug_tag}: {err:#}");
                    None
                }
            },
            None => None,
        };

        let metadata = Metadata {
            package,
            version,
            kind: decoded.kind,
            origin,
            oci,
            debug,
            created,
            annotations: manifest.annotations,
            imports: decoded.imports,
            exports: decoded.exports,
            dependencies: decoded.dependencies,
            release: None,
            indexed_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        let dir = data::write(&self.options.root, &metadata, &decoded.wit)?;
        let dir = dir.strip_prefix(&self.options.root).unwrap_or(&dir);
        eprintln!("indexed {registry}/{repository}:{tag} -> {}", dir.display());
        self.added
            .push(format!("{}@{}", metadata.package, metadata.version));
        Ok(())
    }

    fn fetch(
        &self,
        registry: &str,
        repository: &str,
        tag: &str,
    ) -> Result<(OciRef, http::Manifest)> {
        let (digest, manifest) = self.oci.manifest(registry, repository, tag)?;
        if manifest
            .media_type
            .as_deref()
            .is_some_and(|media_type| media_type.contains("index"))
        {
            anyhow::bail!("an image index, not a wasm artifact");
        }
        let layer = manifest
            .layers
            .iter()
            .find(|layer| layer.media_type == "application/wasm")
            .or_else(|| manifest.layers.first())
            .ok_or_else(|| anyhow!("the manifest has no layers"))?
            .clone();
        Ok((
            OciRef {
                registry: registry.to_string(),
                repository: repository.to_string(),
                tag: tag.to_string(),
                digest,
                layer_digest: layer.digest,
                size: layer.size,
            },
            manifest,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_github_repository_of_a_source() {
        for source in [
            "https://github.com/componentized/logging",
            "https://github.com/componentized/logging.git",
            "https://github.com/componentized/logging/",
        ] {
            assert_eq!(
                github_repo(source).as_deref(),
                Some("componentized/logging"),
                "{source}"
            );
        }
        assert_eq!(
            github_repo("https://gitlab.com/componentized/logging"),
            None
        );
        assert_eq!(github_repo("https://github.com/componentized"), None);
        assert_eq!(
            github_repo("https://github.com/componentized/logging/tree/main"),
            None
        );
    }
}
