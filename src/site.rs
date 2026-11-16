//! Renders the committed index as a static site. The WIT stored in the index is the source of
//! every page.

use crate::api;
use crate::data::{self, Kind, Metadata, Origin};
use crate::decode;
use crate::resolve::{self, Loader};
use crate::version;
use crate::wit_html;
use anyhow::{Context, Result, bail};
use minijinja::Environment;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

    /// Whether it's a composition: a component that nests components with code of their own, those
    /// it was composed from.
    fn composed(&self) -> bool {
        self.components() > 0
    }

    /// How many components it nests with code of their own, those it was composed from: its
    /// wrappers of types alone aren't any.
    fn components(&self) -> usize {
        self.meta.parts.iter().flatten().filter(|p| p.code).count()
    }

    /// The kind its icon shows: `interface`, `component`, or `composition`, a component's icon
    /// badged.
    fn icon_kind(&self) -> &'static str {
        match self.meta.kind {
            Kind::Interface => "interface",
            Kind::Component if self.composed() => "composition",
            Kind::Component => "component",
        }
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
        // GitHub's owners and repositories are case-insensitive, and images spell them either way,
        // e.g. `WebAssembly/wasi-otel` beside `webassembly/wasi`, so the key that groups, sorts and
        // names them is lowercase, while the link keeps the spelling given
        (!key.is_empty()).then(|| (key.to_lowercase(), web.to_string()))
    }

    /// Whether two entries are of one build: built from the same commit, or, when either doesn't
    /// record its commit, on the same day.
    fn same_build(&self, other: &Entry) -> bool {
        let revision = |e: &Entry| {
            e.meta
                .annotation("revision")
                .map(|r| r.trim_end_matches("+dirty").to_string())
        };
        match (revision(self), revision(other)) {
            (Some(a), Some(b)) => a == b,
            _ => self.date() == other.date(),
        }
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

/// Whether a name is an interface's id, e.g. `wasi:config/store@0.2.0-rc.1`, rather than a plain
/// name an import or export is given, e.g. `latch0`.
fn is_interface_id(name: &str) -> bool {
    name.contains(':') && name.contains('/')
}

/// The label of each interface id, as brief as tells it apart from the others: its name, e.g.
/// `store`, then with its package, `config/store`, then with its namespace, `wasi:config/store`,
/// then in full, with its version, `wasi:config/store@0.2.0-rc.1`.
fn interface_labels<'a>(ids: impl IntoIterator<Item = &'a str>) -> HashMap<String, String> {
    let ids: BTreeSet<&str> = ids.into_iter().collect();
    let label = |id: &str, level: usize| -> String {
        let (path, _) = id.split_once('@').unwrap_or((id, ""));
        let (_, package_path) = path.split_once(':').unwrap_or(("", path));
        let (_, name) = package_path.split_once('/').unwrap_or(("", package_path));
        [name, package_path, path, id][level].to_string()
    };
    ids.iter()
        .map(|id| {
            let level = (0..3)
                .find(|level| {
                    let own = label(id, *level);
                    ids.iter()
                        .all(|other| other == id || label(other, *level) != own)
                })
                .unwrap_or(3);
            (id.to_string(), label(id, level))
        })
        .collect()
}

/// What plugs into a part's imports, in the order its world declares them, e.g. `latch0` to
/// `latch4`, rather than the order the composition happens to pass them in, which is kept for one
/// with no page to say, or an import its world doesn't name.
fn in_import_order<'a>(part: Option<&Entry>, args: &'a [data::Plug]) -> Vec<&'a data::Plug> {
    let declared: Vec<&str> = part
        .map(|e| {
            e.meta
                .imports
                .iter()
                .map(|i| i.split_once(": ").map_or(i.as_str(), |(name, _)| name))
                .collect()
        })
        .unwrap_or_default();
    let mut args: Vec<&data::Plug> = args.iter().collect();
    args.sort_by_key(|plug| {
        declared
            .iter()
            .position(|name| *name == plug.name)
            .unwrap_or(declared.len())
    });
    args
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
    /// The components each composed component nests whole, those of its `parts`, each as the
    /// version with its bytes, if one has them: one generated as it was composed has none. A part
    /// without code of its own, the wrapper a component's bindings nest, is `None` too.
    composed_of: Vec<Vec<Option<usize>>>,
    /// The composed components that nest each version, the reverse of `composed_of`.
    used_in: HashMap<usize, Vec<Link>>,
}

struct Repo {
    key: String,
    /// The repository's web url, e.g. `https://github.com/componentized/logging`.
    web: String,
    /// The entries of each version, newest version first.
    versions: Vec<(String, Vec<usize>)>,
    /// The entries left over from an earlier build of a dev version, e.g. a component the
    /// repository has since dropped, which kept its last `0.3.0-dev` build when the others were
    /// built again. They're left out of the version, as the repository's other packages.
    left_over: Vec<usize>,
}

impl Repo {
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

pub fn build(
    root: &Path,
    samples: &Path,
    out: &Path,
    base_url: &str,
    sources: &crate::config::Sources,
) -> Result<()> {
    let mut stored = data::load(root)?;
    sources.apply(&mut stored);
    if let Some(sample) = stored
        .iter()
        .find(|s| s.metadata.origin == Origin::Synthetic)
    {
        bail!(
            "{} is synthetic, it belongs in the samples rather than the index",
            sample.dir.display()
        );
    }
    let updated = last_indexed(stored.iter().map(|s| s.metadata.indexed_at.as_str()));
    let site = Site::new(entries(&stored)?);
    let samples = load_samples(samples, &site)?;

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
            pivot => fingerprint(include_str!("../static/pivot.js")),
            head => fingerprint(include_str!("../static/head.js")),
            copy => fingerprint(include_str!("../static/copy.js")),
            tip => fingerprint(include_str!("../static/tip.js")),
            compare => fingerprint(include_str!("../static/compare.js")),
            wiring => fingerprint(include_str!("../static/wiring.js")),
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
    write_file(out, "pivot.js", include_str!("../static/pivot.js"))?;
    write_file(out, "head.js", include_str!("../static/head.js"))?;
    write_file(out, "copy.js", include_str!("../static/copy.js"))?;
    write_file(out, "tip.js", include_str!("../static/tip.js"))?;
    write_file(out, "compare.js", include_str!("../static/compare.js"))?;
    write_file(out, "wiring.js", include_str!("../static/wiring.js"))?;
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
    render_packages(&site, &env, out)?;
    eprintln!(
        "rendered {} packages, {} versions to {}",
        site.packages.len(),
        site.entries.len(),
        out.display()
    );
    if let Some(samples) = samples {
        // the samples are a site of their own, so the index's pages never list, count or link
        // them, while their pages are published like any other
        let mut env = env.clone();
        env.add_global("synthetic", true);
        render_packages(&samples, &env, out)?;
        eprintln!(
            "rendered {} synthetic sample packages, {} versions",
            samples.packages.len(),
            samples.entries.len()
        );
    }
    Ok(())
}

/// Resolves each stored version for rendering.
fn entries(stored: &[data::Stored]) -> Result<Vec<Entry>> {
    let loader = Loader::new(stored);
    let mut entries = Vec::new();
    for (index, stored) in stored.iter().enumerate() {
        let (resolve, pkg) = loader.resolve(index)?;
        entries.push(Entry {
            meta: stored.metadata.clone(),
            nested: resolve::has_nested(&stored.wit),
            resolve,
            pkg,
        });
    }
    Ok(entries)
}

/// Loads the synthetic samples as a site of their own, `None` when there are none.
fn load_samples(dir: &Path, site: &Site) -> Result<Option<Site>> {
    let stored = data::load(dir)?;
    if stored.is_empty() {
        return Ok(None);
    }
    for sample in &stored {
        if sample.metadata.origin != Origin::Synthetic {
            bail!(
                "{} is in the samples, but its origin isn't synthetic",
                sample.dir.display()
            );
        }
        let namespace = data::split_id(&sample.metadata.package).map_or("", |(ns, _)| ns);
        if site.entries.iter().any(|e| e.namespace() == namespace) {
            bail!(
                "{} is in the {namespace} namespace, which the index holds as well",
                sample.dir.display()
            );
        }
    }
    let mut entries = entries(&stored)?;
    // a component's imports and exports come from its WIT, as a scan derives them for the index
    for entry in &mut entries {
        if entry.meta.kind == Kind::Component {
            let (imports, exports) = decode::component_items(&entry.resolve, entry.pkg)?;
            entry.meta.imports = imports;
            entry.meta.exports = exports;
        }
    }
    Ok(Some(Site::new(entries)))
}

/// Renders the page of each namespace, package version and interface of a site.
fn render_packages(site: &Site, env: &Environment, out: &Path) -> Result<()> {
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
            write_file(
                out,
                &format!("{}api.json", entry.url()),
                serde_json::to_string(&site.api(index))?,
            )?;
        }
        // a package's versions are compared on one page, in the browser, from their APIs
        if let Some(ctx) = site.compare(indexes) {
            let latest = &site.entries[*site.latest(indexes).unwrap_or(&indexes[0])];
            write_file(
                out,
                &format!("{}compare/index.html", latest.package_url()),
                &env.get_template("compare.html")?.render(ctx)?,
            )?;
        }
    }
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
        ("compare.html", include_str!("../templates/compare.html")),
    ] {
        env.add_template(name, source)?;
    }
    Ok(env)
}

/// Renders markdown docs that sit under a heading of the page at level `under`, e.g. 3 for the docs
/// of a type under its `<h3>`. Their headings are moved to fit under it: the docs' top level
/// becomes `under + 1`, the levels below it follow, and none goes deeper than `<h6>`. The WASI
/// docs, e.g., start a function's "Typical errors" with an `<h1>`.
///
/// Relative links and images are stripped, leaving their text: they're relative to wherever the
/// docs were written, e.g. a repository's README, which isn't here.
fn markdown(src: &str, under: usize) -> String {
    use pulldown_cmark::{Event, HeadingLevel, Tag, TagEnd};
    let events: Vec<_> = pulldown_cmark::Parser::new(src).collect();
    let top = events
        .iter()
        .filter_map(|event| match event {
            Event::Start(Tag::Heading { level, .. }) => Some(*level as usize),
            _ => None,
        })
        .min();
    let fit = |level: HeadingLevel| -> HeadingLevel {
        let top = top.unwrap_or(level as usize);
        HeadingLevel::try_from((level as usize - top + under + 1).min(6)).unwrap_or(level)
    };
    // whether each link or image open is stripped, to strip its end too
    let mut stripped = Vec::new();
    let events = events.into_iter().filter_map(|event| match event {
        Event::Start(Tag::Heading {
            level,
            id,
            classes,
            attrs,
        }) => Some(Event::Start(Tag::Heading {
            level: fit(level),
            id,
            classes,
            attrs,
        })),
        Event::End(TagEnd::Heading(level)) => Some(Event::End(TagEnd::Heading(fit(level)))),
        Event::Start(Tag::Link { ref dest_url, .. } | Tag::Image { ref dest_url, .. }) => {
            let strip = !absolute_url(dest_url);
            stripped.push(strip);
            (!strip).then_some(event)
        }
        Event::End(TagEnd::Link | TagEnd::Image) => {
            (!stripped.pop().unwrap_or(false)).then_some(event)
        }
        event => Some(event),
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    html
}

/// Whether a link goes somewhere on its own, with a scheme, e.g. `https:` or `mailto:`, or from
/// the root of a host, e.g. `//example.com`, rather than relative to where it was written.
fn absolute_url(url: &str) -> bool {
    url.starts_with("//")
        || url.split_once(':').is_some_and(|(scheme, _)| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        })
}

/// Renders docs that sit under a heading of the page at level `under`, see [`markdown`].
fn docs(docs: &wit_parser::Docs, under: usize) -> String {
    docs.contents
        .as_deref()
        .map(|src| markdown(src, under))
        .unwrap_or_default()
}

fn summary(src: &str) -> String {
    src.split("\n\n").next().unwrap_or("").replace('\n', " ")
}

/// Versions, newest first, each with `version` and `stable`, by release line, see
/// [`release_line`], for a menu: each line leads with its latest version, its newest stable one,
/// after any newer pre-release, e.g. the next dev build, and folds its older versions, which are
/// seldom chosen.
fn release_lines(versions: &[minijinja::Value]) -> Vec<minijinja::Value> {
    let mut lines: Vec<(String, Vec<minijinja::Value>)> = Vec::new();
    for version in versions {
        let v = version
            .get_attr("version")
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let line = release_line(&v);
        match lines.last_mut() {
            Some((label, members)) if *label == line => members.push(version.clone()),
            _ => lines.push((line, vec![version.clone()])),
        }
    }
    lines
        .into_iter()
        .map(|(label, mut members)| {
            let lead = members
                .iter()
                .position(|v| v.get_attr("stable").is_ok_and(|s| s.is_true()))
                .unwrap_or(0);
            let older = members.split_off(lead + 1);
            ctx! { label, shown => members, older }
        })
        .collect()
}

/// The release line of a version, the versions it's compatible with by semver: `0.2.x` for a
/// 0.x version, whose minor versions each break, else its major, e.g. `1.x`. A version that
/// isn't semver is a line of its own.
fn release_line(version: &str) -> String {
    match semver::Version::parse(version) {
        Ok(v) if v.major == 0 => format!("0.{}.x", v.minor),
        Ok(v) => format!("{}.x", v.major),
        Err(_) => version.to_string(),
    }
}

/// The page of a commit of a repository on GitHub, e.g.
/// `https://github.com/componentized/cli/commit/465de9402d6d…`. `None` for another host, whose
/// commit pages differ.
fn commit_url(web: &str, revision: &str) -> Option<String> {
    web.starts_with("https://github.com/")
        .then(|| format!("{web}/commit/{revision}"))
}

/// A link to a GitHub release, labeled with its tag, e.g. `v0.3.1` for
/// `https://github.com/componentized/logging/releases/tag/v0.3.1`.
fn release_link(url: &str) -> minijinja::Value {
    let tag = url.rsplit_once("/tag/").map_or(url, |(_, tag)| tag);
    ctx! { url, tag }
}

/// The versions of one package, e.g. the dependents `componentized:logging-levels@0.3.1` and
/// `componentized:logging-levels@0.3.0`.
#[derive(Serialize, Debug, PartialEq)]
struct VersionGroup {
    package: String,
    /// The kind its icon shows, `interface`, `component` or `world`, set by [`Site::grouped`].
    kind: &'static str,
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
                kind: "",
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
    /// The kind its icon shows, e.g. `composition` for a component composed of others.
    icon: &'static str,
    description: String,
    url: String,
    date: String,
    versions: usize,
    /// Why the version is indexed: the org's release, a tracked package's, or a dependency.
    origin: Origin,
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
    /// Why its latest version's packages are indexed: the org's releases, else a tracked package's,
    /// else what a release depends on.
    origin: Origin,
}

#[derive(Serialize)]
struct WorldView {
    name: String,
    docs: String,
    imports: Vec<WorldItemLink>,
    exports: Vec<WorldItemLink>,
}

/// An item a world imports or exports, with the kind its icon shows: `interface` or `func`.
#[derive(Serialize)]
struct WorldItemLink {
    #[serde(flatten)]
    link: Link,
    kind: &'static str,
}

#[derive(Serialize)]
struct InterfaceSummary {
    name: String,
    id: String,
    summary: String,
    url: String,
    stability: Option<StabilityView>,
}

/// A choice in a breadcrumb's menu, e.g. another package of the namespace.
#[derive(Serialize)]
struct Pivot {
    label: String,
    url: String,
    current: bool,
    /// What the menu notes beside the label, e.g. a namespace's package count.
    note: Option<String>,
    /// The kind of the choice the menu shows by its icon, e.g. `namespace`, `interface` or
    /// `component`.
    kind: Option<&'static str>,
}

#[derive(Serialize)]
struct VersionLink {
    version: String,
    url: String,
    current: bool,
    stable: bool,
    date: String,
    /// The comparison of the version with the one on view, the older of the two first. `None`
    /// for the version on view, or when either isn't listed to compare.
    #[serde(skip_serializing_if = "Option::is_none")]
    compare: Option<String>,
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
    /// For an alias of a named type, `type a = b`, the type it refers to, following any aliases
    /// and uses that type is in turn.
    aliased: Option<Aliased>,
    /// The docs of the type a `use` or an alias refers to, as it's defined where it comes from.
    origin_docs: String,
}

/// The type an alias refers to, as it's defined where it comes from.
#[derive(Serialize)]
struct Aliased {
    name: String,
    kind: &'static str,
    members: Vec<Member>,
    /// The interface that defines it, e.g. `wasi:clocks/types@0.3.1`.
    from: Option<Link>,
}

/// An image's signature as its page shows it: for a GitHub Actions workflow, the workflow's file
/// at the ref it ran on, and the signature's entry in the transparency log.
fn signature_view(signature: &data::Signature) -> minijinja::Value {
    match signature {
        data::Signature::Unsigned => ctx! { status => "unsigned" },
        data::Signature::Failed { reason } => ctx! { status => "failed", reason },
        data::Signature::Verified(signer) => {
            // `https://github.com/{repository}/{path}@{ref}`, e.g. `.github/workflows/ci.yaml`
            let workflow = signer
                .identity
                .strip_prefix("https://github.com/")
                .and_then(|rest| rest.split_once('@'))
                .and_then(|(path, git_ref)| {
                    let mut parts = path.splitn(3, '/');
                    let (owner, repo, file) = (parts.next()?, parts.next()?, parts.next()?);
                    let at = git_ref
                        .strip_prefix("refs/heads/")
                        .or_else(|| git_ref.strip_prefix("refs/tags/"))
                        .unwrap_or(git_ref);
                    Some(ctx! {
                        name => file.rsplit('/').next().unwrap_or(file),
                        url => format!("https://github.com/{owner}/{repo}/blob/{at}/{file}"),
                    })
                });
            ctx! {
                status => "verified",
                identity => signer.identity,
                issuer => signer.issuer,
                repository => signer.repository,
                git_ref => signer.git_ref.as_ref().map(|r| {
                    r.strip_prefix("refs/heads/").or_else(|| r.strip_prefix("refs/tags/")).unwrap_or(r).to_string()
                }),
                workflow,
                log_index => signer.log_index,
                log_url => signer.log_index.map(|i| format!("https://search.sigstore.dev/?logIndex={i}")),
            }
        }
    }
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
                left_over: Vec::new(),
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
            // a dev version is built again and again, and its build is its newest: the entries
            // built from the same commit, or, for entries that don't record theirs, on the same
            // day. The others are left over from an earlier build.
            for (version, members) in &mut repo.versions {
                if version::release_of(version).is_none() {
                    continue;
                }
                let created = |i: usize| {
                    let meta = &entries[i].meta;
                    meta.created
                        .clone()
                        .unwrap_or_else(|| meta.indexed_at.clone())
                };
                let Some(newest) = members.iter().copied().max_by_key(|i| created(*i)) else {
                    continue;
                };
                let (built, left_over): (Vec<usize>, Vec<usize>) = members
                    .iter()
                    .partition(|i| entries[**i].same_build(&entries[newest]));
                *members = built;
                repo.left_over.extend(left_over);
            }
            for (_, members) in &mut repo.versions {
                // the interface packages, then the components, by name
                members.sort_by(|a, b| {
                    let (a, b) = (&entries[*a].meta, &entries[*b].meta);
                    (a.kind != Kind::Interface, &a.package)
                        .cmp(&(b.kind != Kind::Interface, &b.package))
                });
            }
        }

        // a component nested whole in another is the version whose wasm layer has its bytes, or
        // whose debug build's does. Versions can share their bytes, e.g. a release and the dev
        // build it was cut from: a listed release is preferred, then the highest version.
        let mut by_layer: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            let meta = &entry.meta;
            for oci in std::iter::once(&meta.oci).chain(&meta.debug) {
                by_layer.entry(&oci.layer_digest).or_default().push(index);
            }
        }
        let part_of = |digest: &str| -> Option<usize> {
            by_layer.get(digest)?.iter().copied().min_by(|a, b| {
                let rank = |i: usize| (superseded_by[i].is_some(), !entries[i].stable());
                rank(*a).cmp(&rank(*b)).then_with(|| {
                    version::cmp_desc(&entries[*a].meta.version, &entries[*b].meta.version)
                })
            })
        };
        // only the parts with code of their own: a component's bindings nest a wrapper of types
        let composed_of: Vec<Vec<Option<usize>>> = entries
            .iter()
            .map(|entry| {
                entry
                    .meta
                    .parts
                    .iter()
                    .flatten()
                    .map(|p| p.code.then(|| part_of(&p.digest)).flatten())
                    .collect()
            })
            .collect();
        let mut used_in: HashMap<usize, Vec<Link>> = HashMap::new();
        for (index, found) in composed_of.iter().enumerate() {
            if superseded_by[index].is_some() {
                continue;
            }
            for part in found.iter().flatten() {
                used_in.entry(*part).or_default().push(Link {
                    label: entries[index].id(),
                    url: Some(entries[index].url()),
                });
            }
        }
        for links in used_in.values_mut() {
            links.sort();
            links.dedup();
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
            composed_of,
            used_in,
        }
    }

    fn repo_of(&self, index: usize) -> Option<&Repo> {
        let (key, _) = self.entries[index].repository()?;
        self.repos.get(&key)
    }

    /// Where a repository is listed: its section of its namespace's page, e.g.
    /// `/componentized/#componentized/cli`. A repository's packages share a namespace.
    fn repo_url(&self, repo: &Repo) -> String {
        let namespace = repo
            .versions
            .iter()
            .flat_map(|(_, members)| members)
            .map(|i| self.entries[*i].namespace())
            .next()
            .unwrap_or_default();
        format!("/{namespace}/#{}", repo.key)
    }

    fn repo_card(&self, repo: &Repo) -> RepoCard {
        let (version, members) = &repo.versions[repo.latest()];
        let lead = &self.entries[members[0]];
        RepoCard {
            key: repo.key.clone(),
            url: self.repo_url(repo),
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
            origin: {
                let origins: Vec<Origin> = members
                    .iter()
                    .map(|i| self.entries[*i].meta.origin)
                    .collect();
                if origins.contains(&Origin::Release) {
                    Origin::Release
                } else if origins.contains(&Origin::Tracked) {
                    Origin::Tracked
                } else {
                    origins.first().copied().unwrap_or(Origin::Dependency)
                }
            },
        }
    }

    fn repo_cards(&self) -> Vec<RepoCard> {
        self.repos
            .values()
            .map(|repo| self.repo_card(repo))
            .collect()
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

    fn world_item_link(&self, item: &str) -> WorldItemLink {
        WorldItemLink {
            link: self.interface_link(item),
            kind: if item.ends_with(": func") {
                "func"
            } else {
                "interface"
            },
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
            icon: entry.icon_kind(),
            description: entry.description(),
            url: entry.url(),
            date: entry.date(),
            versions: self.packages[&entry.meta.package]
                .iter()
                .filter(|i| self.listed(**i))
                .count(),
            origin: entry.meta.origin,
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
        let recent: Vec<_> = recent.into_iter().take(6).map(|i| self.card(i)).collect();
        ctx! {
            recent,
            capabilities => self.capabilities(),
            namespaces => self.namespace_counts().into_iter().map(|(ns, _)| ns).collect::<Vec<_>>(),
            repos => self.repo_cards(),
        }
    }

    /// The WASI packages that components implement, each with the components that export one of
    /// its interfaces, at their latest versions: what can stand in for `wasi:http`, say, in a
    /// composition. The packages most implemented first.
    fn capabilities(&self) -> Vec<minijinja::Value> {
        let mut providers: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for indexes in self.packages.values() {
            let Some(&index) = self.latest(indexes) else {
                continue;
            };
            let entry = &self.entries[index];
            if entry.meta.kind != Kind::Component {
                continue;
            }
            let exported: std::collections::BTreeSet<&str> = entry
                .meta
                .exports
                .iter()
                .filter_map(|item| decode::interface_of(item)?.split_once('/'))
                .map(|(package, _)| package)
                .filter(|package| data::split_id(package).is_some_and(|(ns, _)| ns == "wasi"))
                .collect();
            for package in exported {
                providers.entry(package).or_default().push(index);
            }
        }
        let mut capabilities: Vec<_> = providers
            .into_iter()
            .filter_map(|(package, members)| {
                let &latest = self.latest(self.packages.get(package)?)?;
                Some((package, latest, members))
            })
            .collect();
        capabilities.sort_by_key(|(package, _, members)| (std::cmp::Reverse(members.len()), *package));
        capabilities
            .into_iter()
            .map(|(package, latest, members)| {
                let interface = &self.entries[latest];
                ctx! {
                    package,
                    url => interface.url(),
                    description => interface.description(),
                    providers => members.iter().map(|i| self.card(*i)).collect::<Vec<_>>(),
                }
            })
            .collect()
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
                // the interface packages, then the components, by name
                let ordered = |mut members: Vec<usize>| -> Vec<usize> {
                    members.sort_by(|a, b| {
                        let (a, b) = (&self.entries[*a].meta, &self.entries[*b].meta);
                        (a.kind != Kind::Interface, &a.package)
                            .cmp(&(b.kind != Kind::Interface, &b.package))
                    });
                    members
                };
                // the packages a group lists, as cards with their counts by kind
                let listing = |members: &[usize]| {
                    let count = |kind| {
                        members
                            .iter()
                            .filter(|i| self.entries[**i].meta.kind == kind)
                            .count()
                    };
                    ctx! {
                        interfaces => count(Kind::Interface),
                        components => count(Kind::Component),
                        cards => members.iter().map(|i| self.card(*i)).collect::<Vec<_>>(),
                    }
                };
                // the one owner of the namespace's repositories, when they share one, e.g.
                // `webassembly` for `webassembly/wasi` and `webassembly/wasi-config` in `wasi`
                let owners: std::collections::BTreeSet<String> = groups
                    .iter()
                    .filter_map(|(key, _)| key.as_ref()?.split_once('/'))
                    .map(|(owner, _)| owner.to_lowercase())
                    .collect();
                let one_owner = owners.len() == 1;
                let groups: Vec<_> = groups
                    .into_iter()
                    .map(|(key, members)| {
                        let repo = key.as_ref().and_then(|key| self.repos.get(key));
                        // the repository's name, without its owner when the namespace's repositories
                        // share it, e.g. `cli` for `componentized/cli` in `componentized`, or
                        // `wasi-config` for `webassembly/wasi-config` in `wasi`; with it otherwise,
                        // to tell repositories of different owners apart
                        let name = key.as_ref().map(|key| match key.split_once('/') {
                            Some((_, repo)) if one_owner => repo.to_string(),
                            _ => key.clone(),
                        });
                        // a repository lists the packages it released together at one version,
                        // the latest at first, switching to each other version it released
                        // packages of this namespace at. A superseded dev version is left out, as
                        // it is on the repository's page.
                        let versions: Vec<_> = repo
                            .map(|repo| {
                                let latest = repo.latest();
                                // the repository's packages of this namespace, at any version
                                let packages: std::collections::BTreeSet<&str> = repo
                                    .versions
                                    .iter()
                                    .flat_map(|(_, members)| members)
                                    .chain(&repo.left_over)
                                    .filter(|m| self.entries[**m].namespace() == namespace)
                                    .map(|m| self.entries[*m].meta.package.as_str())
                                    .collect();
                                repo.versions
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, (v, _))| *i == latest || !repo.superseded(v))
                                    .filter_map(|(i, (version, members))| {
                                        let members: Vec<usize> = members
                                            .iter()
                                            .copied()
                                            .filter(|m| self.entries[*m].namespace() == namespace)
                                            .collect();
                                        if members.is_empty() {
                                            return None;
                                        }
                                        let date = members
                                            .iter()
                                            .map(|m| self.entries[*m].date())
                                            .min()
                                            .unwrap_or_default();
                                        // the commit the version was built from
                                        let revision = self.entries[members[0]]
                                            .meta
                                            .annotation("revision")
                                            .map(|r| r.trim_end_matches("+dirty").to_string());
                                        // the repository's packages it didn't release at this
                                        // version, e.g. one released at a version of its own, at
                                        // their latest, so they're found from any version
                                        let released: std::collections::BTreeSet<&str> = members
                                            .iter()
                                            .map(|m| self.entries[*m].meta.package.as_str())
                                            .collect();
                                        let others: Vec<_> = packages
                                            .iter()
                                            .filter(|p| !released.contains(*p))
                                            .filter_map(|p| self.compatible(&self.packages[*p], version))
                                            .map(|i| {
                                                let other = &self.entries[*i];
                                                // its lowest and highest releases, with their
                                                // dates, newest first; pre-releases, e.g. a dev
                                                // build, are left out, unless it has only those
                                                let versions = &self.packages[&other.meta.package];
                                                let pick = |keep: &dyn Fn(usize) -> bool| -> Vec<usize> {
                                                    versions.iter().copied().filter(|v| keep(*v)).collect()
                                                };
                                                let mut listed = pick(&|v| self.entries[v].stable());
                                                if listed.is_empty() {
                                                    listed = pick(&|v| self.listed(v));
                                                }
                                                let at = |v: Option<&usize>| {
                                                    v.map(|v| {
                                                        let entry = &self.entries[*v];
                                                        ctx! {
                                                            version => entry.meta.version,
                                                            date => entry.date(),
                                                        }
                                                    })
                                                };
                                                // it links to the highest version it shows
                                                let linked = listed.first().map_or(other, |v| &self.entries[*v]);
                                                ctx! {
                                                    name => other.name(),
                                                    version => linked.meta.version,
                                                    url => linked.url(),
                                                    kind => other.icon_kind(),
                                                    lowest => at(listed.last()),
                                                    highest => at(listed.first()),
                                                    versions => listed.len(),
                                                }
                                            })
                                            .collect();
                                        Some(ctx! {
                                            version,
                                            stable => version::stable(version).is_some(),
                                            others,
                                            // without scripts, a version is chosen from the
                                            // repository's section
                                            url => self.repo_url(repo),
                                            current => i == latest,
                                            date,
                                            revision => revision.as_ref().map(|r| r.chars().take(7).collect::<String>()),
                                            release => members
                                                .iter()
                                                .find_map(|m| self.entries[*m].meta.release.as_deref())
                                                .map(release_link),
                                            commit => revision.as_ref().and_then(|r| commit_url(&repo.web, r)),
                                            listing => listing(&ordered(members)),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        // the packages without a repository each at their own latest version
                        let listing = listing(&ordered(members));
                        ctx! {
                            key,
                            name,
                            version_lines => release_lines(&versions),
                            versions,
                            listing,
                        }
                    })
                    .collect();
                (namespace.clone(), ctx! { namespace, count, groups })
            })
            .collect()
    }

    fn versions(&self, index: usize) -> Vec<VersionLink> {
        let entry = &self.entries[index];
        let comparable = self.listed(index) && self.comparable(&entry.meta.package);
        self.packages[&entry.meta.package]
            .iter()
            .enumerate()
            // a superseded dev version is listed only on its own page
            .filter(|(_, i)| self.listed(**i) || **i == index)
            .map(|(at, i)| {
                let other = &self.entries[*i];
                // the versions are newest first, so the one further on is the older
                let here = self.packages[&entry.meta.package]
                    .iter()
                    .position(|j| *j == index)
                    .unwrap_or(at);
                let compare = (comparable && *i != index).then(|| {
                    let (from, to) = if at > here {
                        (other, entry)
                    } else {
                        (entry, other)
                    };
                    format!(
                        "{}compare/?from={}&to={}",
                        entry.package_url(),
                        from.meta.version,
                        to.meta.version
                    )
                });
                VersionLink {
                    version: other.meta.version.clone(),
                    url: other.url(),
                    current: *i == index,
                    stable: other.stable(),
                    date: other.date(),
                    compare,
                }
            })
            .collect()
    }

    /// Each namespace, to switch to from a breadcrumb.
    fn namespace_pivots(&self, current: &str) -> Vec<Pivot> {
        self.namespace_counts()
            .into_iter()
            .map(|(namespace, count)| Pivot {
                url: format!("/{namespace}/"),
                current: namespace == current,
                note: Some(format!(
                    "{count} package{}",
                    if count == 1 { "" } else { "s" }
                )),
                label: namespace,
                kind: Some("namespace"),
            })
            .collect()
    }

    /// The packages of a version's namespace, to switch to from a breadcrumb.
    fn package_pivots(&self, index: usize) -> Vec<Pivot> {
        let entry = &self.entries[index];
        self.packages
            .iter()
            .filter_map(|(package, indexes)| {
                let latest = &self.entries[*self.latest(indexes)?];
                (latest.namespace() == entry.namespace()).then(|| Pivot {
                    label: latest.name().to_string(),
                    url: latest.package_url(),
                    current: *package == entry.meta.package,
                    note: None,
                    kind: Some(latest.icon_kind()),
                })
            })
            .collect()
    }

    /// Groups `package@version` links by package, as [`group_versions`] does, with the kind of
    /// each: a world of a WIT package is labeled by its path, e.g. `wasi:http/proxy`, an indexed
    /// package has its own kind, and a package that isn't indexed is a dependency, a WIT package.
    fn grouped(&self, links: Vec<Link>) -> Vec<VersionGroup> {
        let mut groups = group_versions(links);
        for group in &mut groups {
            group.kind = self.kind_of(&group.package);
        }
        groups
    }

    /// The kind of a package, or of a world labeled by its path, e.g. `wasi:http/proxy`, for its
    /// icon, its newest version's. A package that isn't indexed is a dependency, a WIT package.
    fn kind_of(&self, package: &str) -> &'static str {
        if package.contains('/') {
            return "world";
        }
        self.packages
            .get(package)
            .and_then(|indexes| indexes.first())
            .map_or("interface", |i| self.entries[*i].icon_kind())
    }

    /// `package@version` links, each with the kind of its package, by label.
    fn listed_packages(&self, mut links: Vec<Link>) -> Vec<minijinja::Value> {
        links.sort();
        links
            .into_iter()
            .map(|link| {
                let package = link
                    .label
                    .rsplit_once('@')
                    .map_or(link.label.as_str(), |(package, _)| package);
                ctx! { kind => self.kind_of(package), label => link.label, url => link.url }
            })
            .collect()
    }

    /// How many of the components a composed component nests, with code of their own, no version
    /// has, e.g. one generated as it was composed.
    fn composed_unknown(&self, index: usize) -> usize {
        let parts = self.entries[index].meta.parts.iter().flatten();
        parts
            .zip(&self.composed_of[index])
            .filter(|(part, found)| part.code && found.is_none())
            .count()
    }

    /// A composition's wiring as its diagram draws it, a block diagram from left to right: the
    /// composition's imports, its parts, each with a port for each of its imports and for each of
    /// its exports something plugs into, and its exports. Each port is linked to the page of its
    /// interface, when it has one, whose tooltip says what it is.
    fn wiring_diagram(&self, index: usize) -> Option<serde_json::Value> {
        use data::Owner;
        use serde_json::json;
        let meta = &self.entries[index].meta;
        let wiring = meta.wiring.as_ref()?;
        let parts = meta.parts.as_deref().unwrap_or_default();
        let found = &self.composed_of[index];
        let shown: Vec<usize> = (0..wiring.instances.len())
            .filter(|i| parts.get(wiring.instances[*i].part).is_some_and(|p| p.code))
            .collect();
        let node = |instance: usize| shown.iter().position(|i| *i == instance);
        // a port by the interface it is: its page, and a short label, e.g. `config/store` for
        // `wasi:config/store@0.2.0-rc.1`, or the name it's plugged in by, e.g. `latch0`
        // each interface the diagram has a port for, labeled as briefly as tells it apart from the
        // others, e.g. `store`, or `config/store` beside `keyvalue/store`
        let ids = wiring
            .instances
            .iter()
            .flat_map(|i| &i.args)
            .chain(&wiring.exports)
            .flat_map(|plug| {
                let import = match &plug.source.owner {
                    Owner::Import(import) => Some(import.as_str()),
                    Owner::Instance(_) => None,
                };
                [
                    Some(plug.name.as_str()),
                    import,
                    plug.source.export.as_deref(),
                ]
            })
            .flatten()
            .filter(|i| is_interface_id(i));
        let labels = interface_labels(ids);
        let port = |name: &str, interface: Option<&str>| {
            let interface = interface.filter(|i| is_interface_id(i));
            json!({
                "name": name,
                "label": labels.get(name).cloned().unwrap_or_else(|| name.to_string()),
                "interface": interface,
                "url": interface.and_then(|i| self.interface_urls.get(i)),
            })
        };
        // where a plug comes from: one of the composition's imports, or a part's export
        let source = |plug: &data::Plug| -> Option<(serde_json::Value, Option<String>)> {
            let source = &plug.source;
            match &source.owner {
                Owner::Import(import) => Some((json!({ "import": import }), Some(import.clone()))),
                Owner::Instance(instance) => {
                    let export = source.export.clone()?;
                    Some((
                        json!({ "part": node(*instance)?, "port": export }),
                        Some(export),
                    ))
                }
            }
        };
        let mut imports: Vec<String> = Vec::new();
        let mut outputs: Vec<Vec<String>> = vec![Vec::new(); shown.len()];
        let mut uses = |plug: &data::Plug| match &plug.source.owner {
            Owner::Import(import) if !imports.contains(import) => imports.push(import.clone()),
            Owner::Instance(instance) => {
                if let (Some(n), Some(export)) = (node(*instance), &plug.source.export)
                    && !outputs[n].contains(export)
                {
                    outputs[n].push(export.clone());
                }
            }
            _ => {}
        };
        for i in &shown {
            wiring.instances[*i].args.iter().for_each(&mut uses);
        }
        wiring.exports.iter().for_each(&mut uses);
        let inputs = |plugs: Vec<&data::Plug>| -> Vec<serde_json::Value> {
            plugs
                .into_iter()
                .filter_map(|plug| {
                    let (from, interface) = source(plug)?;
                    let mut port = port(&plug.name, interface.as_deref());
                    port["from"] = from;
                    Some(port)
                })
                .collect()
        };
        let parts: Vec<_> = shown
            .iter()
            .zip(&outputs)
            .map(|(i, outputs)| {
                let entry = wiring
                    .instances
                    .get(*i)
                    .and_then(|instance| found.get(instance.part).copied().flatten())
                    .map(|e| &self.entries[e]);
                json!({
                    "name": entry.map(|e| e.name()),
                    "label": entry.map(|e| e.id()),
                    "url": entry.map(|e| e.url()),
                    "kind": entry.map_or("component", |e| e.icon_kind()),
                    "inputs": inputs(in_import_order(entry, &wiring.instances[*i].args)),
                    "outputs": outputs.iter().map(|o| port(o, Some(o))).collect::<Vec<_>>(),
                })
            })
            .collect();
        Some(json!({
            "name": meta.package,
            "imports": imports.iter().map(|i| port(i, Some(i))).collect::<Vec<_>>(),
            "parts": parts,
            "exports": inputs(wiring.exports.iter().collect()),
        }))
    }

    /// The other packages the repository of a version released with it, at the same version. One
    /// left over from an earlier build of a dev version was released with the others of that
    /// build, rather than with the version's newest.
    fn released_with(&self, index: usize) -> Vec<Pivot> {
        let entry = &self.entries[index];
        let Some(repo) = self.repo_of(index) else {
            return Vec::new();
        };
        repo.versions
            .iter()
            .find(|(version, _)| *version == entry.meta.version)
            .map(|(version, members)| {
                let members: Vec<usize> = if repo.left_over.contains(&index) {
                    repo.left_over
                        .iter()
                        .copied()
                        .filter(|i| {
                            let other = &self.entries[*i];
                            other.meta.version == *version && other.same_build(entry)
                        })
                        .collect()
                } else {
                    members.clone()
                };
                let mut others: Vec<usize> =
                    members.iter().copied().filter(|i| *i != index).collect();
                // the interface packages, then the components, by name
                others.sort_by(|a, b| {
                    let (a, b) = (&self.entries[*a].meta, &self.entries[*b].meta);
                    (a.kind != Kind::Interface, &a.package)
                        .cmp(&(b.kind != Kind::Interface, &b.package))
                });
                others
                    .into_iter()
                    .map(|i| {
                        let other = &self.entries[i];
                        // its name alone, e.g. `logging-levels`: a repository's packages share
                        // its namespace, which the page's own package already says
                        let label = if other.namespace() == entry.namespace() {
                            other.name().to_string()
                        } else {
                            other.meta.package.clone()
                        };
                        Pivot {
                            label,
                            url: other.url(),
                            current: false,
                            note: None,
                            kind: Some(other.icon_kind()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn header(&self, index: usize) -> minijinja::Value {
        let entry = &self.entries[index];
        let meta = &entry.meta;
        let source = meta
            .annotation("source")
            .map(|s| s.trim_end_matches(".git").to_string());
        let full_revision = meta
            .annotation("revision")
            .map(|r| r.trim_end_matches("+dirty"));
        let revision = full_revision.map(|r| r.chars().take(12).collect::<String>());
        // the commit the version was built from, on GitHub, else the source's own link to it
        let commit = entry
            .repository()
            .zip(full_revision)
            .and_then(|((_, web), revision)| commit_url(&web, revision))
            .or_else(|| meta.annotation("url").map(str::to_string));
        ctx! {
            id => entry.id(),
            package => meta.package,
            namespace => entry.namespace(),
            name => entry.name(),
            version => meta.version,
            kind => meta.kind,
            icon => entry.icon_kind(),
            url => entry.url(),
            package_url => entry.package_url(),
            description => entry.description(),
            // the description, as markdown, for the lede under the page's `<h1>`
            description_html => markdown(&entry.description(), 1),
            origin => meta.origin,
            stable => entry.stable(),
            dev => version::dev(&meta.version).is_some(),
            superseded_by => self.superseded_by[index].map(|i| ctx! {
                version => self.entries[i].meta.version,
                url => self.entries[i].url(),
            }),
            date => entry.date(),
            license => meta.annotation("licenses"),
            source,
            commit,
            repository => self.repo_of(index).map(|repo| ctx! {
                key => repo.key,
                url => self.repo_url(repo),
            }),
            released_with => self.released_with(index),
            compare => self.compare_url(index),
            release => meta.release.as_ref().map(|url| release_link(url)),
            revision,
            versions => self.versions(index),
            version_lines => release_lines(
                &self
                    .versions(index)
                    .iter()
                    .map(|v| minijinja::Value::from(minijinja::value::Serde(v)))
                    .collect::<Vec<_>>(),
            ),
            namespaces => self.namespace_pivots(entry.namespace()),
            packages => self.package_pivots(index),
        }
    }

    fn version(&self, index: usize) -> Result<minijinja::Value> {
        let entry = &self.entries[index];
        let meta = &entry.meta;
        let resolve = &entry.resolve;
        let package = &resolve.packages[entry.pkg];

        let world_items = |items: &wit_parser::IndexMap<WorldKey, WorldItem>| -> Result<Vec<_>> {
            Ok(decode::world_items(resolve, items)
                .iter()
                .map(|item| self.world_item_link(item))
                .collect())
        };
        let mut worlds = Vec::new();
        for (name, id) in &package.worlds {
            let world = &resolve.worlds[*id];
            worlds.push(WorldView {
                name: name.clone(),
                docs: docs(&world.docs, 3),
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
            .map(|i| self.world_item_link(i))
            .collect();
        let exports: Vec<_> = meta
            .exports
            .iter()
            .map(|i| self.world_item_link(i))
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
                digest => debug.digest,
                size => human_size(debug.size),
                signature => debug.signature.as_ref().map(signature_view),
            }
        });

        Ok(ctx! {
            title => entry.id(),
            canonical => entry.url(),
            pkg => self.header(index),
            // the overview sits beside the page's h2 sections, as one of its own
            docs => docs(&package.docs, 2),
            oci => ctx! {
                reference => meta.oci.reference(),
                digest => meta.oci.digest,
                size => human_size(meta.oci.size),
                signature => meta.oci.signature.as_ref().map(signature_view),
            },
            debug,
            worlds,
            interfaces,
            imports,
            exports,
            // a version depends on one version of a package, or now and then on two, e.g. of
            // `wasi:io`, each listed as it is
            dependencies => self.listed_packages(dependencies),
            dependents => self.grouped(dependents),
            // the components it's composed from, and those composed from it
            composed_of => self.listed_packages(
                self.composed_of[index].iter().flatten().map(|i| Link {
                    label: self.entries[*i].id(),
                    url: Some(self.entries[*i].url()),
                }).collect()
            ),
            composed_unknown => self.composed_unknown(index),
            // how its parts are wired together, for its diagram
            wiring => self.wiring_diagram(index),
            used_in => self.grouped(self.used_in.get(&index).cloned().unwrap_or_default()),
            wit => self.package_wit(index)?.0,
            nested_wit => self.package_wit(index)?.1,
        })
    }

    /// The pages of the interfaces a version's WIT links to by their ids, e.g. in a `use`, and of
    /// its own by their names. Foreign ones aren't linked for a version that keeps the foreign
    /// packages it was built against nested in its WIT: the indexed versions of those packages
    /// differ from them, e.g. a dev build that has since moved.
    fn foreign_interface_urls(&self, index: usize) -> HashMap<String, String> {
        let entry = &self.entries[index];
        let mut urls = if entry.nested {
            HashMap::new()
        } else {
            self.interface_urls.clone()
        };
        // the version's own interfaces by name, as a path within its package names them
        for (name, _, _) in entry.interfaces() {
            urls.insert(name.to_string(), format!("{}{name}/", entry.url()));
        }
        urls
    }

    /// A version's WIT, highlighted, each interface linked to its page and each type to its
    /// definition there, by absolute urls, which hold on its page and on a compare page alike.
    ///
    /// The version's own package alone, with, for a version that keeps the foreign packages it was
    /// built against nested in its WIT, those packages apart: the WIT it uses of its dependencies,
    /// which differs from their indexed versions, or isn't indexed.
    fn package_wit(&self, index: usize) -> Result<(String, Option<String>)> {
        let entry = &self.entries[index];
        let resolve = &entry.resolve;
        let scopes: HashMap<String, (String, HashMap<String, String>)> = entry
            .interfaces()
            .into_iter()
            .map(|(name, id, _)| {
                let url = format!("{}{name}/", entry.url());
                let types = resolve.interfaces[id]
                    .types
                    .keys()
                    .map(|t| (t.clone(), format!("{url}#type-{t}")))
                    .collect();
                (name.to_string(), (url, types))
            })
            .collect();
        let print = |nested| {
            wit_html::package_linked(
                resolve,
                entry.pkg,
                nested,
                scopes.clone(),
                self.foreign_interface_urls(index),
            )
        };
        let own = print(false)?;
        if !entry.nested {
            return Ok((own, None));
        }
        // the nested packages follow the version's own, printed alike
        let all = print(true)?;
        let nested = all
            .strip_prefix(own.as_str())
            .map(|rest| rest.trim_start_matches('\n').to_string())
            .filter(|rest| !rest.trim().is_empty());
        Ok((own, nested))
    }

    /// A version's API, for a compare page to diff against another's, see [`api`].
    fn api(&self, index: usize) -> api::Api {
        let entry = &self.entries[index];
        let resolve = &entry.resolve;
        let mut interfaces = BTreeMap::new();
        for (name, id, _) in entry.interfaces() {
            let (types, functions) = self.interface_items(index, id);
            let deprecated =
                |s: &Option<StabilityView>| s.as_ref().is_some_and(|s| s.deprecated.is_some());
            let unstable = |s: &Option<StabilityView>| s.as_ref().and_then(|s| s.unstable.clone());
            // a type's link within its interface's page, e.g. `#type-point`, from the page of the
            // interface, so it holds on a compare page too
            let page = format!("{}{name}/", entry.url());
            let absolute = |html: &str| html.replace("href=\"#", &format!("href=\"{page}#"));
            let esc = wit_html::escape;
            // a member as WIT writes it, e.g. `x: f64`, a case, e.g. `circle(f64)`, or a flag, as
            // text and as highlighted HTML
            let member = |m: &Member, kind: &str| -> (String, String) {
                let ty = api::text(&m.ty);
                let ty_html = absolute(&m.ty);
                let name_html = format!("<span class=\"c\">{}</span>", esc(&m.name));
                match (m.name.is_empty(), ty.is_empty(), kind) {
                    (true, _, _) => (ty, ty_html),
                    (false, true, _) => (m.name.clone(), name_html),
                    (false, false, "variant") => (
                        format!("{}({ty})", m.name),
                        format!("{name_html}({ty_html})"),
                    ),
                    (false, false, _) => (
                        format!("{}: {ty}", m.name),
                        format!("{name_html}: {ty_html}"),
                    ),
                }
            };
            let keyword = |k: &str| format!("<span class=\"k\">{k}</span>");
            let type_name = |t: &str| format!("<span class=\"t\">{}</span>", esc(t));
            // a function's item, its signature as text and as highlighted HTML
            let func = |f: &Member, anchor: String| api::Item {
                kind: "func",
                def: api::text(&f.ty),
                def_html: absolute(&f.ty),
                members: Vec::new(),
                members_html: Vec::new(),
                deprecated: deprecated(&f.stability),
                unstable: unstable(&f.stability),
                anchor,
                used_kind: None,
                modifiers: f.modifiers.clone(),
                summary: api::summary(&f.docs),
                origin: None,
            };
            let mut items = BTreeMap::new();
            for t in &types {
                let kind = t.used_kind.unwrap_or(t.kind);
                let (members, members_html): (Vec<String>, Vec<String>) =
                    t.members.iter().map(|m| member(m, kind)).unzip();
                let (def, def_html) = match t.kind {
                    "use" => {
                        let from = t
                            .from
                            .as_ref()
                            .map(|f| api::unversioned(&f.label))
                            .unwrap_or_default();
                        (
                            format!("use {from}.{{{}}}", t.name),
                            format!(
                                "{} {}.{{{}}}",
                                keyword("use"),
                                esc(&from),
                                type_name(&t.name)
                            ),
                        )
                    }
                    "type" => (
                        format!("type {} = {}", t.name, members.join(", ")),
                        format!(
                            "{} {} = {}",
                            keyword("type"),
                            type_name(&t.name),
                            members_html.join(", ")
                        ),
                    ),
                    "resource" => (
                        format!("resource {}", t.name),
                        format!("{} {}", keyword("resource"), type_name(&t.name)),
                    ),
                    kind => (
                        format!("{kind} {} {{ {} }}", t.name, members.join(", ")),
                        format!(
                            "{} {} {{ {} }}",
                            keyword(kind),
                            type_name(&t.name),
                            members_html.join(", ")
                        ),
                    ),
                };
                // a type's definition, by its kind, name and members, highlighted
                let definition = |kind: &str, name: &str, members_html: &[String]| match kind {
                    "type" => format!(
                        "{} {} = {}",
                        keyword("type"),
                        type_name(name),
                        members_html.join(", ")
                    ),
                    "resource" => format!("{} {}", keyword("resource"), type_name(name)),
                    kind => format!(
                        "{} {} {{ {} }}",
                        keyword(kind),
                        type_name(name),
                        members_html.join(", ")
                    ),
                };
                // the type a use or an alias refers to, as it's defined where it comes from: a
                // used type's members are those it has there
                let origin_summary = api::summary(&t.origin_docs);
                let origin = match (&t.used_kind, &t.aliased) {
                    (Some(used), _) => Some(api::Origin {
                        def_html: definition(used, &t.name, &members_html),
                        from: t.from.as_ref().map(|f| f.label.clone()).unwrap_or_default(),
                        kind: used,
                        name: t.name.clone(),
                        members_html: members_html.clone(),
                        summary: origin_summary,
                    }),
                    (None, Some(aliased)) => {
                        let html: Vec<String> = aliased
                            .members
                            .iter()
                            .map(|m| member(m, aliased.kind).1)
                            .collect();
                        Some(api::Origin {
                            def_html: definition(aliased.kind, &aliased.name, &html),
                            from: aliased
                                .from
                                .as_ref()
                                .map(|f| f.label.clone())
                                .unwrap_or_default(),
                            kind: aliased.kind,
                            name: aliased.name.clone(),
                            members_html: html,
                            summary: origin_summary,
                        })
                    }
                    (None, None) => None,
                };
                items.insert(
                    t.name.clone(),
                    api::Item {
                        kind: t.kind,
                        def,
                        def_html,
                        members,
                        members_html,
                        deprecated: deprecated(&t.stability),
                        unstable: unstable(&t.stability),
                        anchor: format!("type-{}", t.name),
                        used_kind: t.used_kind,
                        modifiers: Vec::new(),
                        summary: api::summary(&t.docs),
                        origin,
                    },
                );
                // a resource's functions, but not those of one used from another interface,
                // which that interface lists
                if t.kind != "use" {
                    for f in &t.functions {
                        items.insert(
                            format!("{}.{}", t.name, f.name),
                            func(f, format!("type-{}.{}", t.name, f.name)),
                        );
                    }
                }
            }
            for f in &functions {
                items.insert(f.name.clone(), func(f, format!("func-{}", f.name)));
            }
            interfaces.insert(
                name.to_string(),
                api::Interface {
                    deprecated: deprecated(&stability(&resolve.interfaces[id].stability)),
                    unstable: unstable(&stability(&resolve.interfaces[id].stability)),
                    summary: resolve.interfaces[id]
                        .docs
                        .contents
                        .as_deref()
                        .map(summary)
                        .unwrap_or_default(),
                    items,
                },
            );
        }
        // whether an imported or exported interface is types alone, which nothing has to provide
        let types_only = |item: &str| -> bool {
            let Some(interface) = decode::interface_of(item) else {
                return false;
            };
            resolve
                .interfaces
                .iter()
                .find(|(id, _)| resolve.id_of(*id).as_deref() == Some(interface))
                .is_some_and(|(_, iface)| {
                    iface.functions.is_empty()
                        && iface.types.values().all(|t| {
                            !matches!(resolve.types[*t].kind, wit_parser::TypeDefKind::Resource)
                        })
                })
        };
        // a component's imports and exports, whose gates were settled when it was built
        let refs = |items: &[String]| {
            items
                .iter()
                .map(|i| api::Ref::new(i, types_only(i), None))
                .collect()
        };
        let worlds = resolve.packages[entry.pkg]
            .worlds
            .iter()
            .filter(|_| entry.meta.kind == Kind::Interface)
            .map(|(name, id)| {
                let world = &resolve.worlds[*id];
                let items = |items| {
                    decode::gated_world_items(resolve, items)
                        .into_iter()
                        .map(|(i, gate)| {
                            let unstable = stability(gate).and_then(|s| s.unstable);
                            api::Ref::new(&i, types_only(&i), unstable)
                        })
                        .collect()
                };
                (
                    name.clone(),
                    api::World {
                        imports: items(&world.imports),
                        exports: items(&world.exports),
                    },
                )
            })
            .collect();
        api::Api {
            package: entry.meta.package.clone(),
            version: entry.meta.version.clone(),
            kind: entry.meta.kind,
            components: entry.components(),
            summary: entry.description(),
            date: entry.date(),
            interfaces,
            worlds,
            imports: refs(&entry.meta.imports),
            exports: refs(&entry.meta.exports),
            dependencies: entry.meta.dependencies.iter().map(|d| d.id()).collect(),
            wit: self
                .package_wit(index)
                .map(|(own, _)| own)
                .unwrap_or_default(),
        }
    }

    /// The version to compare with in the line of `version`, of `versions` newest first: the
    /// line's latest release, rather than a pre-release such as a dev build, unless the line has
    /// no release.
    fn released_in_line(&self, versions: &[usize], version: usize) -> usize {
        let line = release_line(&self.entries[version].meta.version);
        versions
            .iter()
            .copied()
            .filter(|i| release_line(&self.entries[*i].meta.version) == line)
            .find(|i| self.entries[*i].stable())
            .unwrap_or(version)
    }

    /// The comparison of a version with the one before it, or, for the oldest, the one after it.
    /// `None` for a package with one version listed.
    fn compare_url(&self, index: usize) -> Option<String> {
        let entry = &self.entries[index];
        let listed: Vec<usize> = self.packages[&entry.meta.package]
            .iter()
            .copied()
            .filter(|i| self.listed(*i) || *i == index)
            .collect();
        let at = listed.iter().position(|i| *i == index)?;
        let (from, to) = match listed.get(at + 1) {
            Some(older) => (self.released_in_line(&listed[at + 1..], *older), index),
            None => (index, *listed.get(at.checked_sub(1)?)?),
        };
        Some(format!(
            "{}compare/?from={}&to={}",
            entry.package_url(),
            self.entries[from].meta.version,
            self.entries[to].meta.version
        ))
    }

    /// The version of a package most compatible with `version`, of those listed: its newest in
    /// the release line of `version` that isn't newer, e.g. `0.2.6` for `0.2.12`, else its newest
    /// older than `version` in any line, else its latest. A version that isn't semver is matched to
    /// the latest.
    fn compatible<'a>(&self, indexes: &'a [usize], version: &str) -> Option<&'a usize> {
        let Ok(target) = semver::Version::parse(version) else {
            return self.latest(indexes);
        };
        let line = release_line(version);
        let listed: Vec<(&usize, semver::Version)> = indexes
            .iter()
            .filter(|i| self.listed(**i))
            .filter_map(|i| {
                Some((
                    i,
                    semver::Version::parse(&self.entries[*i].meta.version).ok()?,
                ))
            })
            .collect();
        let newest = |candidates: Vec<&(&'a usize, semver::Version)>| {
            candidates
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1))
                .map(|(i, _)| *i)
        };
        newest(
            listed
                .iter()
                .filter(|(i, v)| {
                    *v <= target && release_line(&self.entries[**i].meta.version) == line
                })
                .collect(),
        )
        .or_else(|| newest(listed.iter().filter(|(_, v)| *v <= target).collect()))
        .or_else(|| self.latest(indexes))
    }

    /// Whether a package has a compare page: it lists more than one version.
    fn comparable(&self, package: &str) -> bool {
        self.packages
            .get(package)
            .is_some_and(|indexes| indexes.iter().filter(|i| self.listed(**i)).count() > 1)
    }

    /// The page that compares two versions of a package, `None` for a package with one version.
    fn compare(&self, indexes: &[usize]) -> Option<minijinja::Value> {
        let &latest = self.latest(indexes)?;
        let entry = &self.entries[latest];
        let versions: Vec<_> = indexes
            .iter()
            .filter(|i| self.listed(**i))
            .map(|i| {
                let other = &self.entries[*i];
                ctx! {
                    version => other.meta.version,
                    line => release_line(&other.meta.version),
                    api => format!("{}api.json", other.url()),
                    url => other.url(),
                    date => other.date(),
                    stable => other.stable(),
                }
            })
            .collect();
        if versions.len() < 2 {
            return None;
        }
        // the versions by release line, `0.2.x` for a 0.x version, whose minor versions are each
        // a line of their own, else `1.x`, newest first. A line leads with its latest version, its
        // newest stable one, else its newest, after any newer pre-release, e.g. the next dev
        // build, and folds its older versions, which are seldom compared.
        let lines = release_lines(&versions);
        Some(ctx! {
            lines,
            title => format!("Compare {}", entry.meta.package),
            canonical => format!("{}compare/", entry.package_url()),
            noindex => true,
            pkg => ctx! {
                package => entry.meta.package,
                namespace => entry.namespace(),
                name => entry.name(),
                kind => entry.meta.kind,
                icon => entry.icon_kind(),
                package_url => entry.package_url(),
                namespaces => self.namespace_pivots(entry.namespace()),
                // switching package stays on the comparison, for a package with versions to
                // compare, or goes to the package's page
                packages => self
                    .package_pivots(latest)
                    .into_iter()
                    .map(|mut pivot| {
                        let package = format!("{}:{}", entry.namespace(), pivot.label);
                        if self.comparable(&package) {
                            pivot.url = format!("{}compare/", pivot.url);
                        }
                        pivot
                    })
                    .collect::<Vec<_>>(),
            },
            versions,
            latest => entry.meta.version,
        })
    }

    /// The types and functions of an interface, as its page shows them and its API lists them.
    fn interface_items(&self, index: usize, id: InterfaceId) -> (Vec<TypeView>, Vec<Member>) {
        let entry = &self.entries[index];
        let resolve = &entry.resolve;
        let iface = &resolve.interfaces[id];

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
            // a freestanding function's docs are under its h3, a resource's function is a member
            // under the resource's h3
            let under = if func.kind.resource().is_some() { 4 } else { 3 };
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
                docs: docs(&func.docs, under),
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
                // under the member of a type under its h3
                docs: docs(d, 4),
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

        // where a type is defined: its interface, linking to its definition there
        let from_of = |origin: wit_parser::TypeId, fallback: &str| -> Option<Link> {
            let origin_def = &resolve.types[origin];
            match origin_def.owner {
                TypeOwner::Interface(owner) => resolve.id_of(owner).map(|owner_id| {
                    let url = self.interface_urls.get(&owner_id).map(|url| {
                        format!(
                            "{url}#type-{}",
                            origin_def.name.as_deref().unwrap_or(fallback)
                        )
                    });
                    Link {
                        label: owner_id,
                        url,
                    }
                }),
                _ => None,
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
                    let from = from_of(origin, type_name);
                    TypeView {
                        name: type_name.clone(),
                        kind: "use",
                        used_kind: Some(used_kind),
                        // the docs of the use, or else those of the type it uses
                        docs: match docs(&def.docs, 3) {
                            local if !local.is_empty() => local,
                            _ => docs(&origin_def.docs, 3),
                        },
                        stability: stability(&def.stability),
                        members,
                        functions: if used_kind == "resource" {
                            resource_functions(origin)
                        } else {
                            Vec::new()
                        },
                        from,
                        aliased: None,
                        origin_docs: docs(&origin_def.docs, 3),
                    }
                }
                None => {
                    let (kind, members) = details(*type_id);
                    // an alias of a named type, `type a = b`, refers to the type `b` is, as it's
                    // defined where it comes from
                    let aliased = match def.kind {
                        TypeDefKind::Type(wit_parser::Type::Id(other))
                            if resolve.types[other].name.is_some() =>
                        {
                            Some(origin_of(resolve, other))
                        }
                        _ => None,
                    };
                    let origin_docs = aliased
                        .map(|origin| docs(&resolve.types[origin].docs, 3))
                        .unwrap_or_default();
                    let aliased = aliased.map(|origin| {
                        let (kind, members) = details(origin);
                        let name = resolve.types[origin].name.clone().unwrap_or_default();
                        Aliased {
                            from: from_of(origin, &name),
                            name,
                            kind,
                            members,
                        }
                    });
                    TypeView {
                        name: type_name.clone(),
                        kind,
                        used_kind: None,
                        docs: docs(&def.docs, 3),
                        stability: stability(&def.stability),
                        members,
                        functions: resource_functions(*type_id),
                        from: None,
                        aliased,
                        origin_docs,
                    }
                }
            };
            types.push(view);
        }
        (types, functions)
    }

    fn interface(&self, index: usize, name: &str, id: InterfaceId) -> Result<minijinja::Value> {
        let entry = &self.entries[index];
        let resolve = &entry.resolve;
        let iface = &resolve.interfaces[id];
        let iid = resolve.id_of(id).unwrap_or_else(|| name.to_string());
        let (types, functions) = self.interface_items(index, id);

        // the interface in each version, or that version's page when it doesn't have the interface
        let versions: Vec<VersionLink> = self
            .versions(index)
            .into_iter()
            .map(|mut version| {
                let has = self.packages[&entry.meta.package].iter().any(|i| {
                    self.entries[*i].meta.version == version.version
                        && self.entries[*i]
                            .interfaces()
                            .iter()
                            .any(|(n, _, _)| *n == name)
                });
                if has {
                    version.url = format!("{}{name}/", version.url);
                }
                version
            })
            .collect();
        Ok(ctx! {
            title => iid,
            canonical => format!("{}{name}/", entry.url()),
            pkg => self.header(index),
            iface => ctx! {
                name,
                id => iid,
                // the other interfaces of the version, and this interface in each other version,
                // or that version's page when it doesn't have the interface
                siblings => entry
                    .interfaces()
                    .into_iter()
                    .map(|(other, _, _)| Pivot {
                        label: other.to_string(),
                        url: format!("{}{other}/", entry.url()),
                        current: other == name,
                        note: None,
                        kind: Some("interface"),
                    })
                    .collect::<Vec<_>>(),
                version_lines => release_lines(
                    &versions.iter().map(|v| minijinja::Value::from(minijinja::value::Serde(v))).collect::<Vec<_>>(),
                ),
                versions,
                docs => docs(&iface.docs, 2),
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
                    self.foreign_interface_urls(index),
                )?,
            },
            imported_by => self.grouped(self.imported_by.get(&iid).cloned().unwrap_or_default()),
            exported_by => self.grouped(self.exported_by.get(&iid).cloned().unwrap_or_default()),
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
                signature: None,
            },
            debug: None,
            created: Some("2026-10-15T13:03:05Z".to_string()),
            annotations: BTreeMap::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            dependencies: Vec::new(),
            release: None,
            parts: None,
            wiring: None,
            indexed_at: "2026-11-11T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn labels_interfaces_as_briefly_as_tells_them_apart() {
        let labels = interface_labels([
            "wasi:config/store@0.2.0-rc.1",
            "wasi:keyvalue/store@0.2.0-draft2",
            "componentized:keyvalue/store@0.1.0",
            "wasi:logging/logging@0.1.0-draft",
            "wasi:sockets/types@0.3.0",
            "wasi:sockets/types@0.2.0",
            "componentized:sockets/latch@0.1.0",
        ]);
        let label = |id: &str| labels[id].as_str();
        assert_eq!(label("wasi:config/store@0.2.0-rc.1"), "config/store");
        assert_eq!(
            label("wasi:keyvalue/store@0.2.0-draft2"),
            "wasi:keyvalue/store"
        );
        assert_eq!(
            label("componentized:keyvalue/store@0.1.0"),
            "componentized:keyvalue/store"
        );
        assert_eq!(label("wasi:logging/logging@0.1.0-draft"), "logging");
        assert_eq!(
            label("wasi:sockets/types@0.3.0"),
            "wasi:sockets/types@0.3.0"
        );
        assert_eq!(label("componentized:sockets/latch@0.1.0"), "latch");
    }

    #[test]
    fn renders_cross_links() {
        let root = std::env::temp_dir().join(format!("directory-site-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let logging = "package wasi:logging@0.1.0-draft;\n\ninterface logging {\n  /// A level.\n  enum level { info, warn }\n  @since(version = 0.0.1)\n  @deprecated(version = 0.0.2)\n  log: func(level: level, message: string);\n  @unstable(feature = tracing)\n  trace: func(message: string);\n}\n\ninterface consumer {\n  use logging.{level};\n}\n\nworld app {\n  import logging;\n  @unstable(feature = tracing)\n  import consumer;\n  export run: func();\n}\n";
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
        component.release = Some("https://github.com/example/to/releases/tag/v0.1.0".to_string());
        component.annotations.insert(
            "org.opencontainers.image.revision".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
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
        build(
            &root,
            &root.join("no-samples"),
            &out,
            "https://example.com",
            &Default::default(),
        )
        .unwrap();
        let read = |path: &str| std::fs::read_to_string(out.join(path)).unwrap();

        let iface = read("wasi/logging/0.1.0-draft/logging/index.html");
        assert!(
            iface
                .contains(r##"<use href="#kind-component"/></svg><code>example:to-stdout</code>"##),
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
        // a world's imports and exports show the icon of their kind
        let package = read("wasi/logging/0.1.0-draft/index.html");
        assert!(package.contains(r##"<li><svg class="kind-icon kind-interface""##));
        assert!(package.contains(r##"<li><svg class="kind-icon kind-func""##));
        // with no title, whose browser tooltip would cover the link's own
        assert!(package.contains(r#"<span class="visually-hidden">interface </span>"#));
        assert!(!package.contains(r#"<li title="#));
        assert!(package.contains(r##"<use href="#kind-world"/>"##));
        // a `use` links its interface to its page, and each type it uses to its definition there
        let used = consumer.replace("&#x2f;", "/");
        assert!(
            used.contains(r#"<a class="path" href="/wasi/logging/0.1.0-draft/logging/">"#),
            "the interface used"
        );
        assert!(
            used.contains(
                r#"<a class="t" href="/wasi/logging/0.1.0-draft/logging/#type-level">level</a>"#
            ),
            "the type used"
        );
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

        // a namespace lists a repository's packages at one version, the latest at first, and
        // switches in place to each other version, leaving out a superseded dev build
        let namespace = read("example/index.html");
        assert!(namespace.contains(r#"<span data-pivot-label>0.1.0</span>"#));
        assert!(namespace.contains(r#"<div data-version-panel="0.1.0">"#));
        assert!(namespace.contains(r#"<div data-version-panel="0.2.0-dev" hidden>"#));
        assert!(!namespace.contains("0.1.0-dev"));

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

        // a repository is listed in its namespace's section, and each package links the
        // others it released with it, at the same version
        assert!(!out.join("repos").exists(), "no repository pages");
        assert!(version.contains(r#"<dt>Repository</dt>"#));
        // each version's API is written for comparing versions, on the package's one compare page
        let api: serde_json::Value =
            serde_json::from_str(&read("example/to-stdout/0.1.0/api.json")).unwrap();
        assert_eq!(api["kind"], "component");
        assert_eq!(api["imports"][0]["key"], "wasi:logging/logging");
        // with its WIT highlighted, for a line by line diff of its text
        assert!(
            api::plain(api["wit"].as_str().unwrap())
                .contains("import wasi:logging/logging@0.1.0-draft;")
        );
        let logging: serde_json::Value =
            serde_json::from_str(&read("wasi/logging/0.1.0-draft/api.json")).unwrap();
        assert_eq!(
            logging["interfaces"]["logging"]["items"]["level"]["def"],
            "enum level { info, warn }"
        );
        assert_eq!(
            logging["interfaces"]["logging"]["items"]["log"]["def"],
            "log: func(level: level, message: string)"
        );
        // an item behind a feature gate names the feature, which a compare page labels
        assert_eq!(
            logging["interfaces"]["logging"]["items"]["trace"]["unstable"],
            "tracing"
        );
        assert!(
            logging["interfaces"]["logging"]["items"]["log"]
                .get("unstable")
                .is_none()
        );
        let imports = &logging["worlds"]["app"]["imports"];
        assert!(imports[0].get("unstable").is_none(), "{imports}");
        assert_eq!(imports[1]["unstable"], "tracing", "{imports}");
        // a package's WIT links each interface to its page, and each type to its definition
        let wit = logging["wit"].as_str().unwrap();
        assert!(wit.contains(r#"<a class="i" href="&#x2f;wasi&#x2f;logging&#x2f;0.1.0-draft&#x2f;logging&#x2f;">logging</a>"#) || wit.contains(r#"<a class="i" href="/wasi/logging/0.1.0-draft/logging/">logging</a>"#), "{wit}");
        assert!(wit.contains("logging/#type-level") || wit.contains("logging&#x2f;#type-level"));
        let compare = read("example/to-stdout/compare/index.html");
        // its versions by release line, each line leading with its latest
        assert!(compare.contains(r#"<li class="pivot-line" aria-hidden="true">0.2.x</li>"#));
        assert!(compare.contains(r#"data-version="0.2.0-dev""#));
        assert!(version.contains("compare&#x2f;?from=") || version.contains("compare/?from="));
        assert!(
            !out.join("wasi/logging/compare").exists(),
            "a package with one version has nothing to compare"
        );

        // a version links the commit it was built from
        assert!(version.contains(
            "https:&#x2f;&#x2f;github.com&#x2f;example&#x2f;to&#x2f;commit&#x2f;0123456789abcdef0123456789abcdef01234567"
        ));
        // a version links its GitHub release, by its tag
        assert!(version.contains("<dt>Release</dt>"));
        assert!(version.contains("<code>v0.1.0</code></a>"));
        assert!(
            read("example/index.html")
                .contains(r##"<use href="#meta-notes"/></svg>release notes</a>"##)
        );
        assert!(
            version.contains("/example/#example/to")
                || version.contains("&#x2f;example&#x2f;#example&#x2f;to")
        );

        // a second build replaces the output it rendered
        build(
            &root,
            &root.join("no-samples"),
            &out,
            "https://example.com",
            &Default::default(),
        )
        .unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn leaves_out_an_earlier_build_of_a_dev_version() {
        // a repository's packages at `0.3.0-dev`: two built again from a new commit, one left
        // over from an earlier build, and one without a commit, built on the newest's day
        let entry = |name: &str, created: &str, revision: Option<&str>| {
            let mut meta = metadata(
                &format!("a:{name}"),
                "0.3.0-dev",
                Kind::Interface,
                Origin::Release,
            );
            meta.created = Some(created.to_string());
            meta.annotations.insert(
                "org.opencontainers.image.source".to_string(),
                "https://github.com/a/repo".to_string(),
            );
            if let Some(revision) = revision {
                meta.annotations.insert(
                    "org.opencontainers.image.revision".to_string(),
                    revision.to_string(),
                );
            }
            let mut resolve = Resolve::new();
            let pkg = resolve
                .push_str("package.wit", &format!("package a:{name}@0.3.0-dev;\n"))
                .unwrap();
            Entry {
                meta,
                nested: false,
                resolve,
                pkg,
            }
        };
        let site = Site::new(vec![
            entry("x", "2026-11-13T10:00:00Z", Some("new")),
            entry("y", "2026-11-13T10:01:00Z", Some("new+dirty")),
            entry("old", "2026-10-02T09:00:00Z", Some("old")),
            entry("old-too", "2026-10-02T09:00:00Z", Some("old")),
            entry("z", "2026-11-13T12:00:00Z", None),
        ]);
        let repo = &site.repos["a/repo"];
        let package = |i: &usize| site.entries[*i].meta.package.as_str();
        let (version, members) = &repo.versions[0];
        assert_eq!(version, "0.3.0-dev");
        assert_eq!(
            members.iter().map(package).collect::<Vec<_>>(),
            ["a:x", "a:y", "a:z"]
        );
        assert_eq!(
            repo.left_over.iter().map(package).collect::<Vec<_>>(),
            ["a:old", "a:old-too"]
        );
        // each was released with the others of its own build
        let with = |i: usize| -> Vec<String> {
            site.released_with(i).into_iter().map(|p| p.label).collect()
        };
        // by their names alone, as they share the namespace
        assert_eq!(with(0), ["y", "z"]);
        assert_eq!(with(2), ["old-too"]);
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
    fn renders_samples_apart_from_the_index() {
        let root =
            std::env::temp_dir().join(format!("directory-site-samples-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (index, samples, out) = (root.join("index"), root.join("samples"), root.join("_site"));
        let wit = "package wasi:logging@0.1.0-draft;\n\ninterface logging {\n  log: func(message: string);\n}\n";
        data::write(
            &index,
            &metadata(
                "wasi:logging",
                "0.1.0-draft",
                Kind::Interface,
                Origin::Dependency,
            ),
            wit,
        )
        .unwrap();
        let mut sample = metadata("sink:sample", "0.1.0", Kind::Component, Origin::Synthetic);
        sample.dependencies = vec![Dependency {
            package: "sink:types".to_string(),
            version: Some("0.1.0".to_string()),
        }];
        data::write(
            &samples,
            &metadata("sink:types", "0.1.0", Kind::Interface, Origin::Synthetic),
            "package sink:types@0.1.0;\n\ninterface shapes {\n  area: func() -> f64;\n}\n",
        )
        .unwrap();
        // the imports and exports come from the sample's WIT, its metadata names none
        data::write(
            &samples,
            &sample,
            "package root:component;\n\nworld root {\n  import sink:types/shapes@0.1.0;\n  export run: func();\n}\n",
        )
        .unwrap();

        build(
            &index,
            &samples,
            &out,
            "https://example.com",
            &Default::default(),
        )
        .unwrap();
        let read = |path: &str| std::fs::read_to_string(out.join(path)).unwrap();

        // the samples' pages are published, marked as synthetic and kept out of search engines
        let page = read("sink/sample/0.1.0/index.html");
        assert!(page.contains("Synthetic sample."));
        assert!(page.contains(r#"<meta name="robots" content="noindex">"#));
        assert!(!page.contains(r#"id="pull""#), "nothing to pull");
        assert!(
            page.contains("sink/types/0.1.0/shapes/")
                || page.contains("sink&#x2f;types&#x2f;0.1.0&#x2f;shapes&#x2f;"),
            "the samples link each other"
        );
        assert!(
            page.contains(r#"<span class="visually-hidden">func </span>"#),
            "the exports come from the WIT"
        );
        assert!(read("sink/types/0.1.0/shapes/index.html").contains("Synthetic sample."));
        assert!(read("sink/index.html").contains("Synthetic sample."));
        // and no page of the index lists, counts or links them
        for path in [
            "index.html",
            "all/index.html",
            "search.json",
            "wasi/index.html",
            "wasi/logging/0.1.0-draft/index.html",
            "wasi/logging/0.1.0-draft/logging/index.html",
        ] {
            let page = read(path);
            assert!(!page.contains("sink"), "{path} mentions the samples");
            assert!(!page.contains("Synthetic sample."), "{path}");
        }

        // a sample must be synthetic, and keep to a namespace of its own
        let mut real = metadata("wasi:other", "0.1.0", Kind::Interface, Origin::Synthetic);
        data::write(&samples, &real, "package wasi:other@0.1.0;\n").unwrap();
        let err = build(
            &index,
            &samples,
            &out,
            "https://example.com",
            &Default::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("which the index holds as well"));
        real.origin = Origin::Release;
        data::write(&samples, &real, "package wasi:other@0.1.0;\n").unwrap();
        let err = build(
            &index,
            &samples,
            &out,
            "https://example.com",
            &Default::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("isn't synthetic"));
        std::fs::remove_dir_all(samples.join("wasi")).unwrap();
        // and the index holds no samples
        data::write(
            &index,
            &sample,
            "package root:component;\n\nworld root {}\n",
        )
        .unwrap();
        let err = build(
            &index,
            &samples,
            &out,
            "https://example.com",
            &Default::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("belongs in the samples"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fits_docs_headings_under_the_page() {
        // the top level becomes the one under the page's heading, the levels below it follow
        assert_eq!(
            markdown("Text.\n\n# Errors\n\n## Timeouts\n", 3),
            "<p>Text.</p>\n<h4>Errors</h4>\n<h5>Timeouts</h5>\n"
        );
        // whatever level the docs start at
        assert_eq!(markdown("### Errors\n", 2), "<h3>Errors</h3>\n");
        // and none deeper than h6
        assert_eq!(
            markdown("# A\n\n## B\n\n### C\n", 4),
            "<h5>A</h5>\n<h6>B</h6>\n<h6>C</h6>\n"
        );
        assert_eq!(markdown("No headings.", 3), "<p>No headings.</p>\n");
    }

    #[test]
    fn strips_relative_links_from_docs() {
        // relative links and images leave their text
        assert_eq!(
            markdown(
                "See [the guide](docs/guide.md), [above](#usage) and ![a logo](logo.png).",
                1
            ),
            "<p>See the guide, above and a logo.</p>\n"
        );
        // absolute ones stay
        assert_eq!(
            markdown(
                "[Site](https://example.com), [mail](mailto:a@example.com), [host](//example.com)",
                1
            ),
            "<p><a href=\"https://example.com\">Site</a>, <a href=\"mailto:a@example.com\">mail</a>, <a href=\"//example.com\">host</a></p>\n"
        );
        // and so do autolinks
        assert_eq!(
            markdown("<https://example.com>", 1),
            "<p><a href=\"https://example.com\">https://example.com</a></p>\n"
        );
    }

    #[test]
    fn groups_versions_by_release_line() {
        // before 1.0 each minor version is a line, its patches compatible
        assert_eq!(release_line("0.2.0"), "0.2.x");
        assert_eq!(release_line("0.2.6"), "0.2.x");
        assert_eq!(release_line("0.3.0-dev"), "0.3.x");
        // from 1.0 each major version is a line, its minor versions and patches compatible
        assert_eq!(release_line("1.0.0"), "1.x");
        assert_eq!(release_line("1.2.1"), "1.x");
        assert_eq!(release_line("2.1.0-dev"), "2.x");
        assert_eq!(release_line("0.1.0-draft"), "0.1.x");
        assert_eq!(release_line("latest"), "latest");
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
