//! Known advisories of the packages the indexed versions' SBOMs list, from OSV.dev, kept in one
//! file of the index, `advisories.json`: the packages checked, those with advisories, and each
//! advisory once. It's by package rather than by version, so it grows with the distinct packages
//! that have advisories, not with the versions, and a new advisory changes one file. It holds no
//! time of its own, so it only changes when what's known does.
//!
//! OSV.dev gathers RustSec, the GitHub Advisory Database and the advisories of other ecosystems.
//! Its batch query is asked of each distinct package once, however many SBOMs list it.

use crate::data;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The file of the index the advisories are kept in.
pub const FILE: &str = "advisories.json";

const OSV: &str = "https://api.osv.dev/v1";
/// The most queries OSV takes in a batch.
const BATCH: usize = 1000;

/// The package URL types OSV.dev knows advisories of. A nested component's `pkg:oci`, and a
/// `pkg:github` the SBOM names a toolchain by, e.g. the Rust standard library's
/// `pkg:github/rust-lang/rust@{commit}`, which OSV would take for a GitHub Action, are left out.
const ECOSYSTEMS: &[&str] = &[
    "cargo", "npm", "pypi", "golang", "maven", "nuget", "gem", "composer", "hex", "pub", "swift",
    "hackage", "cran", "conan",
];

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Advisories {
    /// Every package checked, by its package URL without qualifiers, e.g.
    /// `pkg:cargo/anyhow@1.0.102`, so a version with one not yet checked isn't said to have none.
    pub checked: BTreeSet<String>,
    /// The advisories of each package that has any, by their ids.
    pub packages: BTreeMap<String, Vec<String>>,
    /// Each advisory, by its id: RustSec's, else the GitHub Advisory Database's, else OSV's.
    pub advisories: BTreeMap<String, Advisory>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Advisory {
    pub summary: String,
    /// Its other ids, e.g. a CVE and a GHSA.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// `vulnerability`, or an informational advisory RustSec makes, e.g. `unmaintained` or
    /// `unsound`.
    pub kind: String,
    /// How severe, as the GitHub Advisory Database rates it, e.g. `HIGH`, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Its CVSS vector, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvss: Option<String>,
    /// The versions that fix it, of the packages it affects, e.g. `1.0.103`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<String>,
    pub published: String,
}

impl Advisories {
    /// The advisories in the index, none when it hasn't any yet.
    pub fn load(root: &Path) -> Result<Self> {
        match std::fs::read_to_string(root.join(FILE)) {
            Ok(json) => serde_json::from_str(&json).context("parsing the advisories"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).context("reading the advisories"),
        }
    }

    fn write(&self, root: &Path) -> Result<()> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        std::fs::write(root.join(FILE), json).context("writing the advisories")
    }
}

/// A package URL as the advisories key it: without its qualifiers and subpath, e.g.
/// `pkg:cargo/anyhow@1.0.102` for `pkg:cargo/anyhow@1.0.102?download_url=…`. `None` for one
/// OSV.dev has no advisories of, or without a version.
pub fn key(purl: &str) -> Option<String> {
    let purl = purl.split(['?', '#']).next()?;
    let kind = purl.strip_prefix("pkg:")?.split('/').next()?;
    (ECOSYSTEMS.contains(&kind) && purl.contains('@')).then(|| purl.to_string())
}

/// The package URLs of every package an SBOM lists, nested ones too.
pub fn purls(sbom: &serde_json::Value) -> Vec<&str> {
    fn walk<'a>(list: &'a serde_json::Value, out: &mut Vec<&'a str>) {
        for component in list.as_array().into_iter().flatten() {
            out.extend(component["purl"].as_str());
            walk(&component["components"], out);
        }
    }
    let mut out = Vec::new();
    walk(&sbom["components"], &mut out);
    out
}

/// Checks every package the index's SBOMs list against OSV.dev, and writes what's known of them.
pub fn run(root: &Path) -> Result<()> {
    let mut checked = BTreeSet::new();
    for stored in data::load(root)? {
        if let Some(sbom) = data::read_sbom(&stored.dir) {
            checked.extend(purls(&sbom).into_iter().filter_map(key));
        }
    }
    let client = crate::http::client()?;
    let found = query(&client, &checked)?;
    eprintln!(
        "checked {} packages, {} have advisories",
        checked.len(),
        found.len()
    );

    let mut advisories = BTreeMap::new();
    let mut packages = BTreeMap::new();
    let ids: BTreeSet<&String> = found.values().flatten().collect();
    let mut records = BTreeMap::new();
    for id in ids {
        records.insert(id.clone(), fetch(&client, id)?);
    }
    for (purl, ids) in &found {
        // the same advisory under each of its ids, e.g. a RustSec's and a GHSA's, once
        let mut seen: Vec<(String, BTreeSet<String>)> = Vec::new();
        for id in ids {
            let record = &records[id];
            if record.withdrawn.is_some() {
                continue;
            }
            let names: BTreeSet<String> = record
                .aliases
                .iter()
                .cloned()
                .chain(std::iter::once(id.clone()))
                .collect();
            match seen
                .iter_mut()
                .find(|(_, other)| !other.is_disjoint(&names))
            {
                Some((_, other)) => other.extend(names),
                None => seen.push((id.clone(), names)),
            }
        }
        let mut kept = Vec::new();
        for (_, names) in seen {
            let id = canonical(&names);
            let record = cached(&client, &mut records, &id)?;
            let mut severity = record.severity();
            // RustSec rates none; the GHSA of the same advisory may
            // an alias OSV.dev hasn't, e.g. a GHSA a project published only in its own repository,
            // rates nothing
            for alias in names.iter().filter(|n| n.starts_with("GHSA-") && **n != id) {
                if severity.is_some() {
                    break;
                }
                severity =
                    cached_if_known(&client, &mut records, alias)?.and_then(|r| r.severity());
            }
            let advisory = Advisory {
                summary: record
                    .summary
                    .clone()
                    .unwrap_or_else(|| record.details_line()),
                aliases: names.iter().filter(|n| **n != id).cloned().collect(),
                kind: record.kind(),
                severity,
                cvss: record.cvss(),
                fixed: record.fixed(purl),
                published: record.published.get(..10).unwrap_or_default().to_string(),
            };
            advisories.insert(id.clone(), advisory);
            kept.push(id);
        }
        if !kept.is_empty() {
            kept.sort();
            packages.insert(purl.clone(), kept);
        }
    }
    let known = Advisories {
        checked,
        packages,
        advisories,
    };
    if known != Advisories::load(root).unwrap_or_default() {
        known.write(root)?;
        eprintln!(
            "{} packages have {} advisories",
            known.packages.len(),
            known.advisories.len()
        );
    }
    Ok(())
}

/// The id an advisory is kept by, of all of its ids: RustSec's, else a GHSA, else the first.
fn canonical(names: &BTreeSet<String>) -> String {
    names
        .iter()
        .find(|n| n.starts_with("RUSTSEC-"))
        .or_else(|| names.iter().find(|n| n.starts_with("GHSA-")))
        .or_else(|| names.iter().next())
        .cloned()
        .unwrap_or_default()
}

/// The ids of the advisories of each package that has any, from OSV.dev's batch query.
fn query(
    client: &reqwest::blocking::Client,
    purls: &BTreeSet<String>,
) -> Result<BTreeMap<String, Vec<String>>> {
    #[derive(Deserialize)]
    struct Response {
        results: Vec<Result_>,
    }
    #[derive(Deserialize)]
    struct Result_ {
        #[serde(default)]
        vulns: Vec<Vuln>,
        next_page_token: Option<String>,
    }
    #[derive(Deserialize)]
    struct Vuln {
        id: String,
    }
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let purls: Vec<&String> = purls.iter().collect();
    for batch in purls.chunks(BATCH) {
        // a query whose results run past a page is asked again with its token, until done
        let mut pending: Vec<(&String, Option<String>)> =
            batch.iter().map(|p| (*p, None)).collect();
        while !pending.is_empty() {
            let queries: Vec<serde_json::Value> = pending
                .iter()
                .map(|(purl, token)| match token {
                    Some(token) => {
                        serde_json::json!({ "package": { "purl": purl }, "page_token": token })
                    }
                    None => serde_json::json!({ "package": { "purl": purl } }),
                })
                .collect();
            let response: Response = client
                .post(format!("{OSV}/querybatch"))
                .json(&serde_json::json!({ "queries": queries }))
                .send()
                .context("querying OSV.dev")?
                .error_for_status()
                .context("querying OSV.dev")?
                .json()
                .context("reading OSV.dev's results")?;
            if response.results.len() != pending.len() {
                return Err(anyhow!(
                    "OSV.dev answered a batch with another number of results"
                ));
            }
            let mut next = Vec::new();
            for ((purl, _), result) in pending.iter().zip(response.results) {
                if !result.vulns.is_empty() {
                    found
                        .entry((*purl).clone())
                        .or_default()
                        .extend(result.vulns.into_iter().map(|v| v.id));
                }
                if let Some(token) = result.next_page_token {
                    next.push((*purl, Some(token)));
                }
            }
            pending = next;
        }
    }
    for ids in found.values_mut() {
        ids.sort();
        ids.dedup();
    }
    Ok(found)
}

/// An advisory as OSV.dev has it, the parts kept.
#[derive(Debug, Clone, Deserialize)]
struct Record {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    details: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    published: String,
    #[serde(default)]
    withdrawn: Option<String>,
    #[serde(default)]
    severity: Vec<Severity>,
    #[serde(default)]
    affected: Vec<Affected>,
    #[serde(default)]
    database_specific: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct Severity {
    #[serde(rename = "type")]
    kind: String,
    score: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Affected {
    #[serde(default)]
    package: Option<Package>,
    #[serde(default)]
    ranges: Vec<Range>,
    #[serde(default)]
    database_specific: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct Package {
    #[serde(default)]
    purl: Option<String>,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Range {
    #[serde(default)]
    events: Vec<BTreeMap<String, String>>,
}

impl Record {
    /// RustSec's informational kind, e.g. `unmaintained`, else a vulnerability.
    fn kind(&self) -> String {
        self.affected
            .iter()
            .find_map(|a| a.database_specific["informational"].as_str())
            .unwrap_or("vulnerability")
            .to_string()
    }

    fn severity(&self) -> Option<String> {
        self.database_specific["severity"]
            .as_str()
            .map(str::to_uppercase)
    }

    fn cvss(&self) -> Option<String> {
        let mut vectors: Vec<&Severity> = self
            .severity
            .iter()
            .filter(|s| s.kind.starts_with("CVSS"))
            .collect();
        // the newest version of CVSS it's scored in
        vectors.sort_by(|a, b| b.kind.cmp(&a.kind));
        vectors.first().map(|s| s.score.clone())
    }

    /// The versions that fix it of the package `purl` names.
    fn fixed(&self, purl: &str) -> Vec<String> {
        let name = purl
            .rsplit_once('@')
            .map_or(purl, |(path, _)| path)
            .rsplit('/')
            .next()
            .unwrap_or_default();
        let mut fixed: Vec<String> = self
            .affected
            .iter()
            .filter(|a| {
                a.package.as_ref().is_some_and(|p| {
                    p.name == name
                        || p.purl
                            .as_deref()
                            .is_some_and(|q| q.ends_with(&format!("/{name}")))
                })
            })
            .flat_map(|a| &a.ranges)
            .flat_map(|r| &r.events)
            .filter_map(|e| e.get("fixed").cloned())
            .collect();
        fixed.sort();
        fixed.dedup();
        fixed
    }

    /// Its details' first line, for one without a summary.
    fn details_line(&self) -> String {
        self.details.lines().next().unwrap_or_default().to_string()
    }
}

/// An advisory, fetched once.
fn cached(
    client: &reqwest::blocking::Client,
    records: &mut BTreeMap<String, Record>,
    id: &str,
) -> Result<Record> {
    if !records.contains_key(id) {
        records.insert(id.to_string(), fetch(client, id)?);
    }
    Ok(records[id].clone())
}

/// An advisory, fetched once, `None` when OSV.dev hasn't it.
fn cached_if_known(
    client: &reqwest::blocking::Client,
    records: &mut BTreeMap<String, Record>,
    id: &str,
) -> Result<Option<Record>> {
    if !records.contains_key(id) {
        let response = client
            .get(format!("{OSV}/vulns/{id}"))
            .send()
            .with_context(|| format!("fetching {id} from OSV.dev"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let record = response
            .error_for_status()
            .with_context(|| format!("fetching {id} from OSV.dev"))?
            .json()
            .with_context(|| format!("reading {id}"))?;
        records.insert(id.to_string(), record);
    }
    Ok(records.get(id).cloned())
}

fn fetch(client: &reqwest::blocking::Client, id: &str) -> Result<Record> {
    client
        .get(format!("{OSV}/vulns/{id}"))
        .send()
        .with_context(|| format!("fetching {id} from OSV.dev"))?
        .error_for_status()
        .with_context(|| format!("fetching {id} from OSV.dev"))?
        .json()
        .with_context(|| format!("reading {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_the_packages_osv_knows() {
        assert_eq!(
            key("pkg:cargo/anyhow@1.0.102?download_url=file://.").as_deref(),
            Some("pkg:cargo/anyhow@1.0.102")
        );
        assert_eq!(
            key("pkg:npm/%40scope/name@1.2.3").as_deref(),
            Some("pkg:npm/%40scope/name@1.2.3")
        );
        // a nested component, a toolchain by its commit, and one without a version
        assert_eq!(
            key("pkg:oci/latch@sha256%3A00?repository_url=ghcr.io/x"),
            None
        );
        assert_eq!(key("pkg:github/rust-lang/rust@0ef1bc386059"), None);
        assert_eq!(key("pkg:cargo/anyhow"), None);
    }

    #[test]
    fn keeps_an_advisory_by_its_rustsec_id() {
        let names: BTreeSet<String> = ["GHSA-x", "CVE-2026-1", "RUSTSEC-2026-0001"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(canonical(&names), "RUSTSEC-2026-0001");
    }

    #[test]
    fn reads_an_osv_record() {
        let record: Record = serde_json::from_value(serde_json::json!({
            "id": "RUSTSEC-2026-0190",
            "summary": "Unsoundness in `Error::downcast_mut()`",
            "published": "2026-06-25T12:00:00Z",
            "affected": [{
                "package": { "name": "anyhow", "ecosystem": "crates.io", "purl": "pkg:cargo/anyhow" },
                "ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0.0.0-0" }, { "fixed": "1.0.103" }] }],
                "database_specific": { "informational": "unsound" }
            }]
        }))
        .unwrap();
        assert_eq!(record.kind(), "unsound");
        assert_eq!(record.fixed("pkg:cargo/anyhow@1.0.102"), ["1.0.103"]);
        assert_eq!(record.fixed("pkg:cargo/other@1.0.0"), Vec::<String>::new());
        assert_eq!(record.cvss(), None);
    }
}
