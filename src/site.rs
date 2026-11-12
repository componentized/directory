//! Renders the committed index as a static site. The WIT stored in the index is the source of
//! every page.

use crate::data::{self, Kind, Metadata, Origin};
use crate::decode;
use crate::resolve::{self, Loader};
use crate::version;
use crate::wit_html;
use anyhow::{Context, Result, bail};
use minijinja::Environment;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use wit_parser::{
    FunctionKind, InterfaceId, PackageId, Resolve, TypeDefKind, TypeOwner, WorldItem, WorldKey,
};

const MARKER: &str = ".directory-site";

/// Like `minijinja::context!`, serializing each value.
macro_rules! ctx {
    ($($key:ident $(=> $value:expr)?),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut map = BTreeMap::<&str, minijinja::Value>::new();
        $( map.insert(stringify!($key), ctx!(@value $key $(, $value)?)); )*
        minijinja::Value::from(minijinja::value::Serde(&map))
    }};
    (@value $key:ident) => { minijinja::Value::from(minijinja::value::Serde(&$key)) };
    (@value $key:ident, $value:expr) => { minijinja::Value::from(minijinja::value::Serde(&$value)) };
}

struct Entry {
    meta: Metadata,
    /// Whether the stored WIT nests the foreign packages it uses, because one is not indexed.
    nested: bool,
    resolve: Resolve,
    pkg: PackageId,
}

impl Entry {
    fn namespace(&self) -> &str {
        data::split_id(&self.meta.package).map_or("", |(ns, _)| ns)
    }

    fn name(&self) -> &str {
        data::split_id(&self.meta.package).map_or(&self.meta.package, |(_, name)| name)
    }

    fn url(&self) -> String {
        format!(
            "/{}/{}/{}/",
            self.namespace(),
            self.name(),
            self.meta.version
        )
    }

    fn package_url(&self) -> String {
        format!("/{}/{}/", self.namespace(), self.name())
    }

    fn id(&self) -> String {
        format!("{}@{}", self.meta.package, self.meta.version)
    }

    fn stable(&self) -> bool {
        version::stable(&self.meta.version).is_some()
    }

    fn description(&self) -> String {
        self.meta
            .annotation("description")
            .map(ToString::to_string)
            .or_else(|| {
                self.resolve.packages[self.pkg]
                    .docs
                    .contents
                    .as_deref()
                    .map(summary)
            })
            .unwrap_or_default()
    }

    /// The repository the version is built from, e.g. `componentized/logging`, from its
    /// `org.opencontainers.image.source` annotation, with the repository's web url.
    fn repository(&self) -> Option<(String, String)> {
        let source = self.meta.annotation("source")?;
        let web = source.trim_end_matches('/').trim_end_matches(".git");
        let path = web.split_once("://").map_or(web, |(_, rest)| rest);
        let (_, key) = path.split_once('/')?;
        (!key.is_empty()).then(|| (key.to_string(), web.to_string()))
    }

    fn date(&self) -> String {
        self.meta
            .created
            .as_deref()
            .unwrap_or(&self.meta.indexed_at)
            .chars()
            .take(10)
            .collect()
    }

    /// The interfaces defined by the package, with their ids, e.g.
    /// `wasi:logging/logging@0.1.0-draft`.
    fn interfaces(&self) -> Vec<(&str, InterfaceId, String)> {
        self.resolve.packages[self.pkg]
            .interfaces
            .iter()
            .map(|(name, id)| {
                (
                    name.as_str(),
                    *id,
                    self.resolve.id_of(*id).unwrap_or_else(|| name.clone()),
                )
            })
            .collect()
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Link {
    label: String,
    url: Option<String>,
}

struct Site {
    entries: Vec<Entry>,
    /// The entries of each package, newest version first.
    packages: BTreeMap<String, Vec<usize>>,
    /// The page of each interface by id, e.g. `wasi:logging/logging@0.1.0-draft`.
    interface_urls: HashMap<String, String>,
    imported_by: HashMap<String, Vec<Link>>,
    exported_by: HashMap<String, Vec<Link>>,
    /// The versions that depend on a package version, e.g. `wasi:logging@0.1.0-draft`.
    dependents: HashMap<String, Vec<Link>>,
    /// The release that supersedes each dev version, e.g. `0.2.3` for `0.2.3-dev`. A superseded
    /// version keeps its pages, so links to it still work, but is left out of every listing.
    superseded_by: Vec<Option<usize>>,
    /// The repositories the versions are built from, e.g. `componentized/logging`. A repository
    /// releases its interface package and its components together, with the same version.
    repos: BTreeMap<String, Repo>,
}

struct Repo {
    key: String,
    /// The repository's web url, e.g. `https://github.com/componentized/logging`.
    web: String,
    /// The entries of each version, newest version first.
    versions: Vec<(String, Vec<usize>)>,
}

impl Repo {
    fn url(&self) -> String {
        format!("/repos/{}/", self.key)
    }

    fn version_url(&self, version: &str) -> String {
        format!("/repos/{}/{version}/", self.key)
    }

    /// A dev version is superseded once the repository releases its version.
    fn superseded(&self, version: &str) -> bool {
        version::release_of(version)
            .is_some_and(|release| self.versions.iter().any(|(v, _)| *v == release))
    }

    /// The newest stable version, or the newest version when there is no stable one.
    fn latest(&self) -> usize {
        self.versions
            .iter()
            .position(|(v, _)| version::stable(v).is_some())
            .unwrap_or(0)
    }
}

pub fn build(root: &Path, out: &Path, base_url: &str) -> Result<()> {
    let mut entries = Vec::new();
    let stored = data::load(root)?;
    let loader = Loader::new(&stored);
    for (index, stored) in stored.iter().enumerate() {
        let (resolve, pkg) = loader.resolve(index)?;
        entries.push(Entry {
            meta: stored.metadata.clone(),
            nested: resolve::has_nested(&stored.wit),
            resolve,
            pkg,
        });
    }
    let updated = last_indexed(stored.iter().map(|s| s.metadata.indexed_at.as_str()));
    let site = Site::new(entries);

    prepare(out)?;
    let mut env = environment()?;
    env.add_global("base_url", base_url.trim_end_matches('/').to_string());
    if let Some(updated) = updated {
        env.add_global(
            "updated",
            ctx! {
                datetime => updated.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                text => updated.format("%Y-%m-%d %H:%M UTC").to_string(),
                commit => head_commit(root),
            },
        );
    }
    // each asset is referenced with a fingerprint of its contents, so a browser fetches it again
    // once it changes rather than keeping a cached copy
    let fingerprint = |contents: &str| crate::http::sha256(contents.as_bytes())[7..19].to_string();
    env.add_global(
        "assets",
        ctx! {
            style => fingerprint(include_str!("../static/style.css")),
            search => fingerprint(include_str!("../static/search.js")),
            theme => fingerprint(include_str!("../static/theme.js")),
            toc => fingerprint(include_str!("../static/toc.js")),
        },
    );

    write_file(out, MARKER, "")?;
    write_file(out, ".nojekyll", "")?;
    write_file(out, "style.css", include_bytes!("../static/style.css"))?;
    write_file(out, "logo.svg", include_bytes!("../static/logo.svg"))?;
    write_file(out, "favicon.svg", include_bytes!("../static/favicon.svg"))?;
    write_file(
        out,
        "apple-touch-icon.png",
        include_bytes!("../static/apple-touch-icon.png"),
    )?;
    write_file(out, "search.js", include_str!("../static/search.js"))?;
    write_file(out, "theme.js", include_str!("../static/theme.js"))?;
    write_file(out, "toc.js", include_str!("../static/toc.js"))?;
    write_file(out, "search.json", &serde_json::to_string(&site.search())?)?;
    write_file(
        out,
        "index.html",
        &env.get_template("index.html")?.render(site.home())?,
    )?;
    write_file(
        out,
        "all/index.html",
        &env.get_template("all.html")?.render(site.all())?,
    )?;
    write_file(
        out,
        "404.html",
        &env.get_template("404.html")?.render(ctx! {})?,
    )?;
    write_file(
        out,
        "repos/index.html",
        &env.get_template("repos.html")?.render(site.repos_page())?,
    )?;
    for repo in site.repos.values() {
        for (at, (version, _)) in repo.versions.iter().enumerate() {
            let page = env
                .get_template("repo.html")?
                .render(site.repo_page(repo, at))?;
            write_file(
                out,
                &format!("{}index.html", repo.version_url(version)),
                &page,
            )?;
            if at == repo.latest() {
                write_file(out, &format!("{}index.html", repo.url()), &page)?;
            }
        }
    }
    for (namespace, ctx) in site.namespaces() {
        write_file(
            out,
            &format!("{namespace}/index.html"),
            &env.get_template("namespace.html")?.render(ctx)?,
        )?;
    }
    for indexes in site.packages.values() {
        for &index in indexes {
            let entry = &site.entries[index];
            let page = env
                .get_template("version.html")?
                .render(site.version(index)?)?;
            write_file(out, &format!("{}index.html", entry.url()), &page)?;
            if Some(&index) == site.latest(indexes) {
                write_file(out, &format!("{}index.html", entry.package_url()), &page)?;
            }
            for (name, id, _) in entry.interfaces() {
                write_file(
                    out,
                    &format!("{}{name}/index.html", entry.url()),
                    &env.get_template("interface.html")?
                        .render(site.interface(index, name, id)?)?,
                )?;
            }
        }
    }
    eprintln!(
        "rendered {} packages, {} versions to {}",
        site.packages.len(),
        site.entries.len(),
        out.display()
    );
    Ok(())
}

fn write_file(out: &Path, path: &str, contents: impl AsRef<[u8]>) -> Result<()> {
    let path = out.join(path.trim_start_matches('/'));
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Empties the output directory, refusing to empty a directory it did not render.
fn prepare(out: &Path) -> Result<()> {
    if out.exists() {
        if out.join(MARKER).exists() {
            std::fs::remove_dir_all(out)?;
        } else if std::fs::read_dir(out)?.next().is_some() {
            bail!(
                "{} is not empty and was not rendered by `directory build`",
                out.display()
            );
        }
    }
    std::fs::create_dir_all(out)?;
    Ok(())
}

/// When the index last changed: the latest time a version in it was indexed.
fn last_indexed<'a>(
    indexed_at: impl IntoIterator<Item = &'a str>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    indexed_at
        .into_iter()
        .filter_map(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.to_utc())
        .max()
}

/// The commit checked out in the index's git repository, which the site is built from. A build
/// with uncommitted changes still names its last commit, assumed to be pushed.
fn head_commit(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    let commit = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let valid = output.status.success()
        && commit.len() == 40
        && commit.chars().all(|c| c.is_ascii_hexdigit());
    valid.then_some(commit)
}

fn environment() -> Result<Environment<'static>> {
    let mut env = Environment::new();
    for (name, source) in [
        ("base.html", include_str!("../templates/base.html")),
        ("logo.svg", include_str!("../static/logo.svg")),
        ("macros.html", include_str!("../templates/macros.html")),
        ("index.html", include_str!("../templates/index.html")),
        ("all.html", include_str!("../templates/all.html")),
        (
            "namespace.html",
            include_str!("../templates/namespace.html"),
        ),
        ("version.html", include_str!("../templates/version.html")),
        (
            "interface.html",
            include_str!("../templates/interface.html"),
        ),
        ("404.html", include_str!("../templates/404.html")),
        ("repos.html", include_str!("../templates/repos.html")),
        ("repo.html", include_str!("../templates/repo.html")),
    ] {
        env.add_template(name, source)?;
    }
    Ok(env)
}

fn markdown(src: &str) -> String {
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new(src));
    html
}

fn docs(docs: &wit_parser::Docs) -> String {
    docs.contents.as_deref().map(markdown).unwrap_or_default()
}

fn summary(src: &str) -> String {
    src.split("\n\n").next().unwrap_or("").replace('\n', " ")
}

/// The versions of one package, e.g. the dependents `componentized:logging-levels@0.3.1` and
/// `componentized:logging-levels@0.3.0`.
#[derive(Serialize, Debug, PartialEq)]
struct VersionGroup {
    package: String,
    /// Each version, labeled with the version alone, newest first.
    versions: Vec<Link>,
}

/// Groups `package@version` links by package, ordered by package.
fn group_versions(links: Vec<Link>) -> Vec<VersionGroup> {
    let mut groups: Vec<VersionGroup> = Vec::new();
    for link in links {
        let (package, version) = match link.label.rsplit_once('@') {
            Some((package, version)) => (package.to_string(), version.to_string()),
            None => (link.label.clone(), String::new()),
        };
        let versioned = Link {
            label: version,
            url: link.url,
        };
        match groups.iter_mut().find(|g| g.package == package) {
            Some(group) => group.versions.push(versioned),
            None => groups.push(VersionGroup {
                package,
                versions: vec![versioned],
            }),
        }
    }
    for group in &mut groups {
        group
            .versions
            .sort_by(|a, b| version::cmp_desc(&a.label, &b.label));
    }
    groups.sort_by(|a, b| a.package.cmp(&b.package));
    groups
}

fn human_size(size: u64) -> String {
    match size {
        s if s >= 1024 * 1024 => format!("{:.1} MiB", s as f64 / (1024.0 * 1024.0)),
        s if s >= 1024 => format!("{:.1} KiB", s as f64 / 1024.0),
        s => format!("{s} B"),
    }
}

#[derive(Serialize)]
struct Card {
    id: String,
    package: String,
    namespace: String,
    /// The package's name within its namespace, e.g. `logging-levels`.
    name: String,
    version: String,
    kind: Kind,
    description: String,
    url: String,
    date: String,
    versions: usize,
    dependency: bool,
}

#[derive(Serialize)]
struct RepoCard {
    key: String,
    url: String,
    version: String,
    description: String,
    /// The interface packages the repository releases, e.g. `componentized:logging`.
    interfaces: Vec<String>,
    components: usize,
    date: String,
    dependency: bool,
}

#[derive(Serialize)]
struct WorldView {
    name: String,
    docs: String,
    imports: Vec<Link>,
    exports: Vec<Link>,
}

#[derive(Serialize)]
struct InterfaceSummary {
    name: String,
    id: String,
    summary: String,
    url: String,
    stability: Option<StabilityView>,
}

#[derive(Serialize)]
struct VersionLink {
    version: String,
    url: String,
    current: bool,
    stable: bool,
    date: String,
}

#[derive(Serialize)]
struct Member {
    name: String,
    ty: String,
    docs: String,
    stability: Option<StabilityView>,
    /// How a resource's function is listed: `constructor`, `static`, or `method`.
    role: Option<&'static str>,
    /// A function's modifiers: `static`, `async`, `get`, `set` or `constructor`.
    modifiers: Vec<&'static str>,
}

/// An item's `@since`, `@unstable` and `@deprecated` gates.
#[derive(Serialize, Debug, PartialEq)]
struct StabilityView {
    since: Option<String>,
    unstable: Option<String>,
    deprecated: Option<String>,
}

fn stability(stability: &wit_parser::Stability) -> Option<StabilityView> {
    match stability {
        wit_parser::Stability::Unknown => None,
        wit_parser::Stability::Stable { since, deprecated } => Some(StabilityView {
            since: Some(since.to_string()),
            unstable: None,
            deprecated: deprecated.as_ref().map(ToString::to_string),
        }),
        wit_parser::Stability::Unstable {
            feature,
            deprecated,
        } => Some(StabilityView {
            since: None,
            unstable: Some(feature.clone()),
            deprecated: deprecated.as_ref().map(ToString::to_string),
        }),
    }
}

#[derive(Serialize)]
struct TypeView {
    name: String,
    kind: &'static str,
    /// The kind of the type a `use` imports, e.g. `resource`.
    used_kind: Option<&'static str>,
    /// Where a used type is defined, e.g. `wasi:io/streams@0.2.6`.
    from: Option<Link>,
    docs: String,
    stability: Option<StabilityView>,
    members: Vec<Member>,
    functions: Vec<Member>,
}

/// The definition a `use` imports: the type it refers to, following any uses that type is in
/// turn.
fn origin_of(resolve: &Resolve, mut id: wit_parser::TypeId) -> wit_parser::TypeId {
    for _ in 0..64 {
        match resolve.types[id].kind {
            TypeDefKind::Type(wit_parser::Type::Id(next)) if resolve.types[next].name.is_some() => {
                id = next
            }
            _ => break,
        }
    }
    id
}

impl Site {
    fn new(entries: Vec<Entry>) -> Self {
        let mut packages: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            packages
                .entry(entry.meta.package.clone())
                .or_default()
                .push(index);
        }
        for indexes in packages.values_mut() {
            indexes.sort_by(|a, b| {
                version::cmp_desc(&entries[*a].meta.version, &entries[*b].meta.version)
            });
        }

        let superseded_by: Vec<Option<usize>> = entries
            .iter()
            .map(|entry| {
                let release = version::release_of(&entry.meta.version)?;
                packages[&entry.meta.package]
                    .iter()
                    .copied()
                    .find(|i| entries[*i].meta.version == release)
            })
            .collect();

        let mut interface_urls = HashMap::new();
        for entry in entries.iter().filter(|e| e.meta.kind == Kind::Interface) {
            for (name, _, id) in entry.interfaces() {
                interface_urls.insert(id, format!("{}{name}/", entry.url()));
            }
        }

        let mut imported_by: HashMap<String, Vec<Link>> = HashMap::new();
        let mut exported_by: HashMap<String, Vec<Link>> = HashMap::new();
        let mut dependents: HashMap<String, Vec<Link>> = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            if superseded_by[index].is_some() {
                continue;
            }
            let link = Link {
                label: entry.id(),
                url: Some(entry.url()),
            };
            match entry.meta.kind {
                Kind::Component => {
                    for (items, index) in [
                        (&entry.meta.imports, &mut imported_by),
                        (&entry.meta.exports, &mut exported_by),
                    ] {
                        for interface in items.iter().filter_map(|i| decode::interface_of(i)) {
                            index
                                .entry(interface.to_string())
                                .or_default()
                                .push(link.clone());
                        }
                    }
                }
                Kind::Interface => {
                    // the worlds of a WIT package import and export interfaces too
                    for (world_name, world) in &entry.resolve.packages[entry.pkg].worlds {
                        let world = &entry.resolve.worlds[*world];
                        let link = Link {
                            label: format!(
                                "{}/{world_name}@{}",
                                entry.meta.package, entry.meta.version
                            ),
                            url: Some(format!("{}#world-{world_name}", entry.url())),
                        };
                        for (items, index) in [
                            (&world.imports, &mut imported_by),
                            (&world.exports, &mut exported_by),
                        ] {
                            for item in decode::world_items(&entry.resolve, items) {
                                if let Some(interface) = decode::interface_of(&item) {
                                    index
                                        .entry(interface.to_string())
                                        .or_default()
                                        .push(link.clone());
                                }
                            }
                        }
                    }
                }
            }
            for dep in &entry.meta.dependencies {
                dependents.entry(dep.id()).or_default().push(link.clone());
            }
        }
        for links in imported_by
            .values_mut()
            .chain(exported_by.values_mut())
            .chain(dependents.values_mut())
        {
            links.sort();
            links.dedup();
        }

        let mut repos: BTreeMap<String, Repo> = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            let Some((key, web)) = entry.repository() else {
                continue;
            };
            let repo = repos.entry(key.clone()).or_insert_with(|| Repo {
                key,
                web,
                versions: Vec::new(),
            });
            match repo
                .versions
                .iter_mut()
                .find(|(v, _)| *v == entry.meta.version)
            {
                Some((_, members)) => members.push(index),
                None => repo
                    .versions
                    .push((entry.meta.version.clone(), vec![index])),
            }
        }
        for repo in repos.values_mut() {
            repo.versions
                .sort_by(|(a, _), (b, _)| version::cmp_desc(a, b));
            for (_, members) in &mut repo.versions {
                // the interface packages, then the components, by name
                members.sort_by(|a, b| {
                    let (a, b) = (&entries[*a].meta, &entries[*b].meta);
                    (a.kind != Kind::Interface, &a.package)
                        .cmp(&(b.kind != Kind::Interface, &b.package))
                });
            }
        }

        Self {
            entries,
            packages,
            interface_urls,
            imported_by,
            exported_by,
            dependents,
            superseded_by,
            repos,
        }
    }

    fn repo_of(&self, index: usize) -> Option<&Repo> {
        let (key, _) = self.entries[index].repository()?;
        self.repos.get(&key)
    }

    fn repo_card(&self, repo: &Repo) -> RepoCard {
        let (version, members) = &repo.versions[repo.latest()];
        let lead = &self.entries[members[0]];
        RepoCard {
            key: repo.key.clone(),
            url: repo.url(),
            version: version.clone(),
            description: lead.description(),
            interfaces: members
                .iter()
                .filter(|i| self.entries[**i].meta.kind == Kind::Interface)
                .map(|i| self.entries[*i].meta.package.clone())
                .collect(),
            components: members
                .iter()
                .filter(|i| self.entries[**i].meta.kind == Kind::Component)
                .count(),
            date: members
                .iter()
                .map(|i| self.entries[*i].date())
                .min()
                .unwrap_or_default(),
            dependency: members
                .iter()
                .all(|i| self.entries[*i].meta.origin == Origin::Dependency),
        }
    }

    fn repo_cards(&self) -> Vec<RepoCard> {
        self.repos
            .values()
            .map(|repo| self.repo_card(repo))
            .collect()
    }

    fn repos_page(&self) -> minijinja::Value {
        ctx! { repos => self.repo_cards() }
    }

    fn repo_page(&self, repo: &Repo, at: usize) -> minijinja::Value {
        let (version, members) = &repo.versions[at];
        let cards = |kind: Kind| -> Vec<Card> {
            members
                .iter()
                .filter(|i| self.entries[**i].meta.kind == kind)
                .map(|i| self.card(*i))
                .collect()
        };
        let versions: Vec<_> = repo
            .versions
            .iter()
            .enumerate()
            // a superseded dev version is listed only on its own page
            .filter(|(i, (v, _))| *i == at || !repo.superseded(v))
            .map(|(i, (v, members))| VersionLink {
                version: v.clone(),
                url: repo.version_url(v),
                current: i == at,
                stable: version::stable(v).is_some(),
                date: members
                    .iter()
                    .map(|m| self.entries[*m].date())
                    .min()
                    .unwrap_or_default(),
            })
            .collect();
        let lead = &self.entries[members[0]].meta;
        let revision = lead
            .annotation("revision")
            .map(|r| r.trim_end_matches("+dirty").to_string());
        let release = version::release_of(version)
            .filter(|release| repo.versions.iter().any(|(v, _)| v == release))
            .map(|release| ctx! { version => release, url => repo.version_url(&release) });
        ctx! {
            title => format!("{}@{version}", repo.key),
            canonical => repo.version_url(version),
            noindex => release.is_some(),
            repo => ctx! {
                key => repo.key,
                url => repo.url(),
                web => repo.web,
                version,
                stable => version::stable(version).is_some(),
                dev => version::dev(version).is_some(),
                superseded_by => release,
                date => versions.iter().find(|v| v.current).map(|v| v.date.clone()),
                revision => revision.as_ref().map(|r| r.chars().take(12).collect::<String>()),
                tree => revision.as_ref().map(|r| format!("{}/tree/{r}", repo.web)),
                versions,
            },
            interfaces => cards(Kind::Interface),
            components => cards(Kind::Component),
        }
    }

    fn listed(&self, index: usize) -> bool {
        self.superseded_by[index].is_none()
    }

    /// The newest stable version of a package, or its newest version when it has no stable one.
    fn latest<'a>(&self, indexes: &'a [usize]) -> Option<&'a usize> {
        indexes
            .iter()
            .find(|i| self.entries[**i].stable())
            .or_else(|| indexes.first())
    }

    /// Links an imported or exported item to the interface it is, or aliases, e.g.
    /// `latch0: componentized:http/latch@0.1.0`.
    fn interface_link(&self, item: &str) -> Link {
        Link {
            label: item.to_string(),
            url: decode::interface_of(item).and_then(|id| self.interface_urls.get(id).cloned()),
        }
    }

    fn package_link(&self, package: &str, version: Option<&str>) -> Link {
        let label = match version {
            Some(version) => format!("{package}@{version}"),
            None => package.to_string(),
        };
        let url = self.packages.get(package).and_then(|indexes| {
            let index = match version {
                Some(version) => indexes
                    .iter()
                    .find(|i| self.entries[**i].meta.version == version)?,
                None => self.latest(indexes)?,
            };
            Some(self.entries[*index].url())
        });
        Link { label, url }
    }

    fn card(&self, index: usize) -> Card {
        let entry = &self.entries[index];
        Card {
            id: entry.id(),
            package: entry.meta.package.clone(),
            namespace: entry.namespace().to_string(),
            name: entry.name().to_string(),
            version: entry.meta.version.clone(),
            kind: entry.meta.kind,
            description: entry.description(),
            url: entry.url(),
            date: entry.date(),
            versions: self.packages[&entry.meta.package]
                .iter()
                .filter(|i| self.listed(**i))
                .count(),
            dependency: entry.meta.origin == Origin::Dependency,
        }
    }

    /// A card for the latest version of every package.
    fn cards(&self) -> Vec<Card> {
        self.packages
            .values()
            .filter_map(|indexes| self.latest(indexes))
            .map(|index| self.card(*index))
            .collect()
    }

    fn search(&self) -> Vec<Card> {
        self.cards()
    }

    fn home(&self) -> minijinja::Value {
        let cards = self.cards();
        let mut recent: Vec<usize> = (0..self.entries.len())
            // dev builds are not releases
            .filter(|i| {
                self.entries[*i].meta.origin == Origin::Release && self.entries[*i].stable()
            })
            .collect();
        recent.sort_by(|a, b| {
            let (a, b) = (&self.entries[*a], &self.entries[*b]);
            b.meta
                .created
                .cmp(&a.meta.created)
                .then_with(|| a.meta.package.cmp(&b.meta.package))
        });
        let recent: Vec<_> = recent.into_iter().take(10).map(|i| self.card(i)).collect();
        let components: Vec<_> = cards.iter().filter(|c| c.kind == Kind::Component).collect();
        let interfaces: Vec<_> = cards.iter().filter(|c| c.kind == Kind::Interface).collect();
        ctx! {
            recent,
            components,
            interfaces,
            namespaces => self.namespace_counts(),
            repos => self.repo_cards(),
            stats => ctx! {
                repos => self.repos.len(),
                packages => self.packages.len(),
                components => components.len(),
                interfaces => interfaces.len(),
                versions => (0..self.entries.len()).filter(|i| self.listed(*i)).count(),
            },
        }
    }

    fn namespace_counts(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for package in self.packages.keys() {
            if let Some((ns, _)) = data::split_id(package) {
                *counts.entry(ns.to_string()).or_default() += 1;
            }
        }
        counts.into_iter().collect()
    }

    fn all(&self) -> minijinja::Value {
        ctx! { cards => self.cards(), namespaces => self.namespace_counts() }
    }

    /// The latest version of each package in each namespace, grouped by the repository it is
    /// released from.
    fn namespaces(&self) -> Vec<(String, minijinja::Value)> {
        self.namespace_counts()
            .into_iter()
            .map(|(namespace, count)| {
                let mut groups: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
                for indexes in self.packages.values() {
                    let Some(&index) = self.latest(indexes) else {
                        continue;
                    };
                    if self.entries[index].namespace() != namespace {
                        continue;
                    }
                    let repo = self.repo_of(index).map(|repo| repo.key.clone());
                    groups.entry(repo).or_default().push(index);
                }
                // the packages without a known repository last
                let mut groups: Vec<_> = groups.into_iter().collect();
                groups.sort_by_key(|(key, _)| (key.is_none(), key.clone()));
                let groups: Vec<_> = groups
                    .into_iter()
                    .map(|(key, mut members)| {
                        // the interface packages, then the components, by name
                        members.sort_by(|a, b| {
                            let (a, b) = (&self.entries[*a].meta, &self.entries[*b].meta);
                            (a.kind != Kind::Interface, &a.package)
                                .cmp(&(b.kind != Kind::Interface, &b.package))
                        });
                        let repo = key.as_ref().and_then(|key| self.repos.get(key));
                        // the repository's name, without the namespace when the namespace owns it,
                        // e.g. `cli` for `componentized/cli` in `componentized`
                        let name = key.as_ref().map(|key| {
                            key.strip_prefix(&format!("{namespace}/"))
                                .unwrap_or(key)
                                .to_string()
                        });
                        ctx! {
                            key,
                            name,
                            url => repo.map(Repo::url),
                            version => repo.map(|repo| repo.versions[repo.latest()].0.clone()),
                            interfaces => members.iter().filter(|i| self.entries[**i].meta.kind == Kind::Interface).count(),
                            components => members.iter().filter(|i| self.entries[**i].meta.kind == Kind::Component).count(),
                            cards => members.into_iter().map(|i| self.card(i)).collect::<Vec<_>>(),
                        }
                    })
                    .collect();
                (namespace.clone(), ctx! { namespace, count, groups })
            })
            .collect()
    }

    fn versions(&self, index: usize) -> Vec<VersionLink> {
        let entry = &self.entries[index];
        self.packages[&entry.meta.package]
            .iter()
            // a superseded dev version is listed only on its own page
            .filter(|i| self.listed(**i) || **i == index)
            .map(|i| {
                let other = &self.entries[*i];
                VersionLink {
                    version: other.meta.version.clone(),
                    url: other.url(),
                    current: *i == index,
                    stable: other.stable(),
                    date: other.date(),
                }
            })
            .collect()
    }

    fn header(&self, index: usize) -> minijinja::Value {
        let entry = &self.entries[index];
        let meta = &entry.meta;
        let source = meta
            .annotation("source")
            .map(|s| s.trim_end_matches(".git").to_string());
        let revision = meta
            .annotation("revision")
            .map(|r| r.chars().take(12).collect::<String>());
        ctx! {
            id => entry.id(),
            package => meta.package,
            namespace => entry.namespace(),
            name => entry.name(),
            version => meta.version,
            kind => meta.kind,
            url => entry.url(),
            package_url => entry.package_url(),
            description => entry.description(),
            dependency => meta.origin == Origin::Dependency,
            stable => entry.stable(),
            dev => version::dev(&meta.version).is_some(),
            superseded_by => self.superseded_by[index].map(|i| ctx! {
                version => self.entries[i].meta.version,
                url => self.entries[i].url(),
            }),
            date => entry.date(),
            license => meta.annotation("licenses"),
            source,
            tree => meta.annotation("url"),
            repository => self.repo_of(index).map(|repo| ctx! {
                key => repo.key,
                url => repo.version_url(&meta.version),
            }),
            revision,
            versions => self.versions(index),
        }
    }

    fn version(&self, index: usize) -> Result<minijinja::Value> {
        let entry = &self.entries[index];
        let meta = &entry.meta;
        let resolve = &entry.resolve;
        let package = &resolve.packages[entry.pkg];

        let world_items = |items: &wit_parser::IndexMap<WorldKey, WorldItem>| -> Result<Vec<Link>> {
            Ok(decode::world_items(resolve, items)
                .iter()
                .map(|item| self.interface_link(item))
                .collect())
        };
        let mut worlds = Vec::new();
        for (name, id) in &package.worlds {
            let world = &resolve.worlds[*id];
            worlds.push(WorldView {
                name: name.clone(),
                docs: docs(&world.docs),
                imports: world_items(&world.imports)?,
                exports: world_items(&world.exports)?,
            });
        }
        let interfaces: Vec<_> = entry
            .interfaces()
            .into_iter()
            .map(|(name, id, iid)| InterfaceSummary {
                name: name.to_string(),
                summary: resolve.interfaces[id]
                    .docs
                    .contents
                    .as_deref()
                    .map(summary)
                    .unwrap_or_default(),
                url: format!("{}{name}/", entry.url()),
                id: iid,
                stability: stability(&resolve.interfaces[id].stability),
            })
            .collect();
        let imports: Vec<_> = meta
            .imports
            .iter()
            .map(|i| self.interface_link(i))
            .collect();
        let exports: Vec<_> = meta
            .exports
            .iter()
            .map(|i| self.interface_link(i))
            .collect();
        let dependencies: Vec<_> = meta
            .dependencies
            .iter()
            .map(|d| self.package_link(&d.package, d.version.as_deref()))
            .collect();
        let dependents = self
            .dependents
            .get(&entry.id())
            .cloned()
            .unwrap_or_default();
        let debug = meta.debug.as_ref().map(|debug| {
            ctx! {
                reference => debug.reference(),
                pinned => debug.pinned(),
                size => human_size(debug.size),
            }
        });

        Ok(ctx! {
            title => entry.id(),
            canonical => entry.url(),
            pkg => self.header(index),
            docs => docs(&package.docs),
            oci => ctx! {
                reference => meta.oci.reference(),
                pinned => meta.oci.pinned(),
                digest => meta.oci.digest,
                size => human_size(meta.oci.size),
            },
            debug,
            worlds,
            interfaces,
            imports,
            exports,
            dependencies => group_versions(dependencies),
            dependents => group_versions(dependents),
            wit => wit_html::package(resolve, entry.pkg, entry.nested)?,
        })
    }

    fn interface(&self, index: usize, name: &str, id: InterfaceId) -> Result<minijinja::Value> {
        let entry = &self.entries[index];
        let resolve = &entry.resolve;
        let iface = &resolve.interfaces[id];
        let iid = resolve.id_of(id).unwrap_or_else(|| name.to_string());

        // a named type links to its definition: on this page for this interface's types, on the
        // page of the interface that defines it for a type used from another interface
        let link = |type_id: wit_parser::TypeId| -> Option<String> {
            let def = &resolve.types[type_id];
            let name = def.name.as_ref()?;
            let TypeOwner::Interface(owner) = def.owner else {
                return None;
            };
            if owner == id {
                return Some(format!("#type-{name}"));
            }
            let url = self.interface_urls.get(&resolve.id_of(owner)?)?;
            Some(format!("{url}#type-{name}"))
        };

        // a function, a freestanding one or one of a resource
        let func_member = |func: &wit_parser::Function| {
            let role = match func.kind {
                FunctionKind::Constructor(_) => Some("constructor"),
                FunctionKind::Static(_) | FunctionKind::AsyncStatic(_) => Some("static"),
                // methods, and the getters and setters of a resource
                _ => func.kind.resource().map(|_| "method"),
            };
            Member {
                // a constructor shares its resource's name
                name: match role {
                    Some("constructor") => "constructor".to_string(),
                    _ => func.item_name().to_string(),
                },
                ty: wit_html::signature(resolve, func, &link),
                docs: docs(&func.docs),
                stability: stability(&func.stability),
                role,
                modifiers: wit_html::modifiers(&func.kind),
            }
        };
        // the functions of a resource, from the interface that defines it
        let resource_functions = |resource: wit_parser::TypeId| -> Vec<Member> {
            let TypeOwner::Interface(owner) = resolve.types[resource].owner else {
                return Vec::new();
            };
            resolve.interfaces[owner]
                .functions
                .values()
                .filter(|func| func.kind.resource() == Some(resource))
                .map(func_member)
                .collect()
        };
        let functions: Vec<Member> = iface
            .functions
            .values()
            .filter(|func| func.kind.resource().is_none())
            .map(func_member)
            .collect();

        // the kind of a type and its members: fields, cases, flags, or the type it aliases
        let details = |type_id: wit_parser::TypeId| -> (&'static str, Vec<Member>) {
            let member = |name: &str, ty: Option<&wit_parser::Type>, d: &wit_parser::Docs| Member {
                name: name.to_string(),
                ty: ty
                    .map(|ty| wit_html::type_ref(resolve, ty, &link))
                    .unwrap_or_default(),
                docs: docs(d),
                stability: None,
                role: None,
                modifiers: Vec::new(),
            };
            match &resolve.types[type_id].kind {
                TypeDefKind::Record(r) => (
                    "record",
                    r.fields
                        .iter()
                        .map(|f| member(&f.name, Some(&f.ty), &f.docs))
                        .collect(),
                ),
                TypeDefKind::Variant(v) => (
                    "variant",
                    v.cases
                        .iter()
                        .map(|c| member(&c.name, c.ty.as_ref(), &c.docs))
                        .collect(),
                ),
                TypeDefKind::Enum(e) => (
                    "enum",
                    e.cases
                        .iter()
                        .map(|c| member(&c.name, None, &c.docs))
                        .collect(),
                ),
                TypeDefKind::Flags(f) => (
                    "flags",
                    f.flags
                        .iter()
                        .map(|f| member(&f.name, None, &f.docs))
                        .collect(),
                ),
                TypeDefKind::Resource => ("resource", Vec::new()),
                TypeDefKind::Type(ty) => ("type", vec![member("", Some(ty), &Default::default())]),
                // a named list, option, result, tuple, handle, ... shows what it aliases
                kind => (
                    "type",
                    vec![Member {
                        name: String::new(),
                        ty: wit_html::structure(resolve, kind, &link),
                        docs: String::new(),
                        stability: None,
                        role: None,
                        modifiers: Vec::new(),
                    }],
                ),
            }
        };

        let mut types = Vec::new();
        for (type_name, type_id) in &iface.types {
            let def = &resolve.types[*type_id];
            // a type used from another interface, `use wasi:io/streams.{input-stream}`, shows the
            // details of the type it uses, from the interface that defines it
            let used = match def.kind {
                TypeDefKind::Type(wit_parser::Type::Id(other))
                    if resolve.types[other].owner != TypeOwner::Interface(id) =>
                {
                    Some(origin_of(resolve, other))
                }
                _ => None,
            };
            let view = match used {
                Some(origin) => {
                    let origin_def = &resolve.types[origin];
                    let (used_kind, members) = details(origin);
                    let from = match origin_def.owner {
                        TypeOwner::Interface(owner) => resolve.id_of(owner).map(|owner_id| {
                            let url = self.interface_urls.get(&owner_id).map(|url| {
                                format!(
                                    "{url}#type-{}",
                                    origin_def.name.as_deref().unwrap_or(type_name)
                                )
                            });
                            Link {
                                label: owner_id,
                                url,
                            }
                        }),
                        _ => None,
                    };
                    TypeView {
                        name: type_name.clone(),
                        kind: "use",
                        used_kind: Some(used_kind),
                        // the docs of the use, or else those of the type it uses
                        docs: match docs(&def.docs) {
                            local if !local.is_empty() => local,
                            _ => docs(&origin_def.docs),
                        },
                        stability: stability(&def.stability),
                        members,
                        functions: if used_kind == "resource" {
                            resource_functions(origin)
                        } else {
                            Vec::new()
                        },
                        from,
                    }
                }
                None => {
                    let (kind, members) = details(*type_id);
                    TypeView {
                        name: type_name.clone(),
                        kind,
                        used_kind: None,
                        docs: docs(&def.docs),
                        stability: stability(&def.stability),
                        members,
                        functions: resource_functions(*type_id),
                        from: None,
                    }
                }
            };
            types.push(view);
        }

        Ok(ctx! {
            title => iid,
            canonical => format!("{}{name}/", entry.url()),
            pkg => self.header(index),
            iface => ctx! {
                name,
                id => iid,
                docs => docs(&iface.docs),
                stability => stability(&iface.stability),
                types,
                functions,
                wit => wit_html::interface(
                    resolve,
                    id,
                    name,
                    iface
                        .types
                        .keys()
                        .map(|name| (name.clone(), format!("#type-{name}")))
                        .collect(),
                )?,
            },
            imported_by => group_versions(self.imported_by.get(&iid).cloned().unwrap_or_default()),
            exported_by => group_versions(self.exported_by.get(&iid).cloned().unwrap_or_default()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Dependency, OciRef};

    fn metadata(package: &str, version: &str, kind: Kind, origin: Origin) -> Metadata {
        Metadata {
            package: package.to_string(),
            version: version.to_string(),
            kind,
            origin,
            oci: OciRef {
                registry: "ghcr.io".to_string(),
                repository: package.replace(':', "/"),
                tag: version.to_string(),
                digest: format!("sha256:{}", "0".repeat(64)),
                layer_digest: format!("sha256:{}", "1".repeat(64)),
                size: 1234,
            },
            debug: None,
            created: Some("2026-10-15T13:03:05Z".to_string()),
            annotations: BTreeMap::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            dependencies: Vec::new(),
            indexed_at: "2026-11-11T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn renders_cross_links() {
        let root = std::env::temp_dir().join(format!("directory-site-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let logging = "package wasi:logging@0.1.0-draft;\n\ninterface logging {\n  /// A level.\n  enum level { info, warn }\n  @since(version = 0.0.1)\n  @deprecated(version = 0.0.2)\n  log: func(level: level, message: string);\n  @unstable(feature = tracing)\n  trace: func(message: string);\n}\n\ninterface consumer {\n  use logging.{level};\n}\n";
        data::write(
            &root,
            &metadata(
                "wasi:logging",
                "0.1.0-draft",
                Kind::Interface,
                Origin::Dependency,
            ),
            logging,
        )
        .unwrap();
        let mut component = metadata(
            "example:to-stdout",
            "0.1.0",
            Kind::Component,
            Origin::Release,
        );
        component.annotations.insert(
            "org.opencontainers.image.source".to_string(),
            "https://github.com/example/to.git".to_string(),
        );
        component.imports = vec!["wasi:logging/logging@0.1.0-draft".to_string()];
        component.exports = vec!["wasi:logging/logging@0.1.0-draft".to_string()];
        component.dependencies = vec![Dependency {
            package: "wasi:logging".to_string(),
            version: Some("0.1.0-draft".to_string()),
        }];
        // the main package alone, wasi:logging resolves from its indexed version
        let world = "package root:component;\n\nworld root {\n  import wasi:logging/logging@0.1.0-draft;\n}\n";
        data::write(&root, &component, world).unwrap();

        // a dev build superseded by the 0.1.0 release, and one leading up to the next release
        for version in ["0.1.0-dev", "0.2.0-dev"] {
            let mut dev = component.clone();
            dev.version = version.to_string();
            data::write(&root, &dev, world).unwrap();
        }

        let out = root.join("_site");
        build(&root, &out, "https://example.com").unwrap();
        let read = |path: &str| std::fs::read_to_string(out.join(path)).unwrap();

        let iface = read("wasi/logging/0.1.0-draft/logging/index.html");
        assert!(
            iface.contains("<summary><code>example:to-stdout</code>"),
            "imported by the component"
        );
        assert!(iface.contains("A level."));
        assert!(iface.contains(">since 0.0.1<"));
        assert!(iface.contains(">deprecated 0.0.2<"));
        assert!(
            iface.contains(">unstable: tracing<"),
            "feature gated items are kept"
        );

        // a used type shows the docs and details of the type it uses, and where it's from
        let consumer = read("wasi/logging/0.1.0-draft/consumer/index.html");
        assert!(consumer.contains("A level."), "the docs of the used type");
        assert!(
            consumer.contains(r#"<code class="c">warn</code>"#),
            "the cases of the used enum"
        );
        assert!(consumer.contains(r#"<p class="from">from"#));
        // a type in a signature links to its definition on the page
        assert_eq!(
            iface
                .matches(r##"<a class="t" href="#type-level">level</a>"##)
                .count(),
            2,
            "the signature and the WIT link the type"
        );
        // the component's page shows its main package, not the packages it uses
        assert!(!read("example/to-stdout/0.1.0/index.html").contains("enum</span>"));
        let version = read("example/to-stdout/0.1.0/index.html");
        assert!(
            version.contains("wasi/logging/0.1.0-draft/logging/")
                || version.contains("wasi&#x2f;logging&#x2f;0.1.0-draft&#x2f;logging&#x2f;")
        );
        assert_eq!(
            read("example/to-stdout/index.html"),
            version,
            "latest version"
        );
        assert!(read("search.json").contains("example:to-stdout"));

        // the superseded dev build keeps its page, but is listed nowhere else
        let superseded = read("example/to-stdout/0.1.0-dev/index.html");
        assert!(superseded.contains("noindex"));
        assert!(superseded.contains("superseded by the release"));
        for page in [
            "example/to-stdout/0.1.0/index.html",
            "example/to-stdout/0.2.0-dev/index.html",
            "wasi/logging/0.1.0-draft/logging/index.html",
            "wasi/logging/0.1.0-draft/index.html",
        ] {
            let html = read(page);
            assert!(!html.contains("0.1.0-dev"), "{page} lists 0.1.0-dev");
        }
        assert!(
            version.contains("0.2.0-dev"),
            "the upcoming dev build is listed"
        );
        assert!(read("example/to-stdout/0.2.0-dev/index.html").contains(">dev<"));

        // the repository groups what it releases at each version, and links back
        let repo = read("repos/example/to/0.1.0/index.html");
        assert!(repo.contains("example:to-stdout"));
        assert_eq!(read("repos/example/to/index.html"), repo, "latest version");
        assert!(
            !repo.contains(">0.1.0-dev<"),
            "superseded dev build not listed"
        );
        assert!(repo.contains(">0.2.0-dev<"));
        assert!(version.contains(r#"<dt>Repository</dt>"#));
        assert!(read("repos/index.html").contains("example&#x2f;to"));

        // a second build replaces the output it rendered
        build(&root, &out, "https://example.com").unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn groups_versions_by_package() {
        let link = |label: &str| Link {
            label: label.to_string(),
            url: Some(format!("/{label}/")),
        };
        let groups = group_versions(vec![
            link("a:x@0.2.0"),
            link("a:y@0.1.0"),
            link("a:x@0.10.0"),
            link("a:x@0.3.0-dev"),
        ]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].package, "a:x");
        let versions: Vec<_> = groups[0]
            .versions
            .iter()
            .map(|l| l.label.as_str())
            .collect();
        assert_eq!(versions, ["0.10.0", "0.3.0-dev", "0.2.0"]);
        assert_eq!(groups[1].package, "a:y");
        assert_eq!(
            groups[1].versions,
            [link("a:y@0.1.0")].map(|l| Link {
                label: "0.1.0".to_string(),
                ..l
            })
        );
    }

    #[test]
    fn finds_when_the_index_last_changed() {
        let updated = last_indexed([
            "2026-11-11T00:00:00Z",
            "2026-11-12T09:30:00+02:00",
            "not a time",
            "2026-11-12T00:15:14Z",
        ])
        .unwrap();
        assert_eq!(updated.to_rfc3339(), "2026-11-12T07:30:00+00:00");
        assert_eq!(last_indexed([]), None);
    }

    #[test]
    fn finds_the_commit_built_from() {
        let outside = std::env::temp_dir().join(format!("directory-no-git-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        let commit = head_commit(&outside);
        std::fs::remove_dir_all(&outside).unwrap();
        // the temp dir is only inside a repository on an unusual machine
        if let Some(commit) = commit {
            assert_eq!(commit.len(), 40);
        }
        assert_eq!(head_commit(Path::new("/nonexistent/index")), None);
    }

    #[test]
    fn refuses_foreign_output() {
        let out =
            std::env::temp_dir().join(format!("directory-site-foreign-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("keep.txt"), "").unwrap();
        assert!(prepare(&out).is_err());
        std::fs::remove_dir_all(&out).unwrap();
    }
}
