//! Minimal clients for the OCI distribution API, the GitHub API and wasm-pkg registry metadata.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, LINK, WWW_AUTHENTICATE};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;

const USER_AGENT: &str = concat!("componentized-directory/", env!("CARGO_PKG_VERSION"));

pub fn client() -> Result<Client> {
    Ok(Client::builder().user_agent(USER_AGENT).build()?)
}

pub fn sha256(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut digest = String::from("sha256:");
    for byte in hash {
        digest.push_str(&format!("{byte:02x}"));
    }
    digest
}

/// The OCI distribution API with anonymous bearer tokens, negotiated from the registry's
/// `WWW-Authenticate` challenge.
pub struct Oci {
    client: Client,
    tokens: RefCell<HashMap<(String, String), String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub config: Option<Descriptor>,
    #[serde(default)]
    pub layers: Vec<Descriptor>,
    #[serde(default)]
    pub annotations: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
}

/// The wasm OCI config, `application/vnd.wasm.config.v0+json`.
#[derive(Debug, Deserialize, Default)]
pub struct WasmConfig {
    #[serde(default)]
    pub created: Option<String>,
}

impl Oci {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            tokens: RefCell::default(),
        }
    }

    /// GETs a registry API url, answering an auth challenge with an anonymous token.
    fn get(&self, registry: &str, repository: &str, url: &str, accept: &str) -> Result<Response> {
        let key = (registry.to_string(), repository.to_string());
        let send = |token: Option<&String>| {
            let mut request = self.client.get(url).header(ACCEPT, accept);
            if let Some(token) = token {
                request = request.header(AUTHORIZATION, format!("Bearer {token}"));
            }
            request.send()
        };
        let token = self.tokens.borrow().get(&key).cloned();
        let response = send(token.as_ref())?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        // a registry denies a token for a repository that does not exist, leave the 401 to the caller
        let Some(token) = self.token(response.headers(), repository)? else {
            return Ok(response);
        };
        self.tokens.borrow_mut().insert(key, token.clone());
        Ok(send(Some(&token))?)
    }

    fn token(&self, headers: &HeaderMap, repository: &str) -> Result<Option<String>> {
        let challenge = headers
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| anyhow!("registry did not send an auth challenge"))?;
        let params = parse_challenge(challenge)?;
        let realm = params
            .get("realm")
            .ok_or_else(|| anyhow!("auth challenge without a realm: {challenge}"))?;
        let mut query = vec![(
            "scope",
            params
                .get("scope")
                .cloned()
                .unwrap_or_else(|| format!("repository:{repository}:pull")),
        )];
        if let Some(service) = params.get("service") {
            query.push(("service", service.clone()));
        }
        #[derive(Deserialize)]
        struct Token {
            #[serde(alias = "access_token")]
            token: String,
        }
        let response = self.client.get(realm).query(&query).send()?;
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => Ok(None),
            status if !status.is_success() => {
                bail!("token request for {repository} failed: {status}")
            }
            _ => Ok(Some(response.json::<Token>()?.token)),
        }
    }

    /// Lists the tags of a repository, `None` when the repository does not exist or is private.
    pub fn tags(&self, registry: &str, repository: &str) -> Result<Option<Vec<String>>> {
        #[derive(Deserialize)]
        struct TagList {
            #[serde(default)]
            tags: Option<Vec<String>>,
        }
        let mut tags = Vec::new();
        let mut url = format!("https://{registry}/v2/{repository}/tags/list?n=1000");
        loop {
            let response = self.get(registry, repository, &url, "application/json")?;
            match response.status() {
                StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    return Ok(None);
                }
                status if !status.is_success() => {
                    bail!("listing tags of {registry}/{repository}: {status}")
                }
                _ => {}
            }
            let next = next_link(response.headers());
            tags.extend(response.json::<TagList>()?.tags.unwrap_or_default());
            match next {
                Some(next) if next.starts_with('/') => url = format!("https://{registry}{next}"),
                Some(next) => url = next,
                None => return Ok(Some(tags)),
            }
        }
    }

    /// Fetches a manifest by tag or digest, returning its digest along with it.
    pub fn manifest(
        &self,
        registry: &str,
        repository: &str,
        reference: &str,
    ) -> Result<(String, Manifest)> {
        let url = format!("https://{registry}/v2/{repository}/manifests/{reference}");
        let response = self.get(
            registry,
            repository,
            &url,
            "application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json",
        )?;
        if !response.status().is_success() {
            bail!(
                "fetching manifest {registry}/{repository}:{reference}: {}",
                response.status()
            );
        }
        let bytes = response.bytes()?;
        let digest = sha256(&bytes);
        let manifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing manifest {registry}/{repository}:{reference}"))?;
        Ok((digest, manifest))
    }

    /// Fetches a blob, verifying its digest.
    pub fn blob(&self, registry: &str, repository: &str, digest: &str) -> Result<Vec<u8>> {
        let url = format!("https://{registry}/v2/{repository}/blobs/{digest}");
        let response = self.get(registry, repository, &url, "*/*")?;
        if !response.status().is_success() {
            bail!(
                "fetching blob {registry}/{repository}@{digest}: {}",
                response.status()
            );
        }
        let bytes = response.bytes()?.to_vec();
        let actual = sha256(&bytes);
        if actual != digest {
            bail!("blob {registry}/{repository}@{digest} has digest {actual}");
        }
        Ok(bytes)
    }
}

fn next_link(headers: &HeaderMap) -> Option<String> {
    let link = headers.get(LINK)?.to_str().ok()?;
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        params.contains("rel=\"next\"").then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
    })
}

fn parse_challenge(challenge: &str) -> Result<HashMap<String, String>> {
    let rest = challenge
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow!("unsupported auth challenge: {challenge}"))?;
    let mut params = HashMap::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
            chars.next();
        }
        let key: String = chars.by_ref().take_while(|c| *c != '=').collect();
        if key.is_empty() {
            break;
        }
        let value: String = if chars.peek() == Some(&'"') {
            chars.next();
            chars.by_ref().take_while(|c| *c != '"').collect()
        } else {
            chars.by_ref().take_while(|c| *c != ',').collect()
        };
        params.insert(key.trim().to_string(), value);
    }
    Ok(params)
}

/// Where a WIT namespace's packages are published, from the registry's
/// `/.well-known/wasm-pkg/registry.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct OciLocation {
    pub registry: String,
    pub namespace_prefix: String,
}

impl OciLocation {
    pub fn repository(&self, namespace: &str, name: &str) -> String {
        format!("{}{namespace}/{name}", self.namespace_prefix)
    }
}

/// Locates a registry's packages from its `/.well-known/wasm-pkg/registry.json`.
pub fn well_known(client: &Client, domain: &str) -> Result<OciLocation> {
    let url = format!("https://{domain}/.well-known/wasm-pkg/registry.json");
    let response = client.get(&url).send()?;
    if !response.status().is_success() {
        bail!("fetching {url}: {}", response.status());
    }
    let metadata = response.json().with_context(|| format!("parsing {url}"))?;
    oci_location(metadata).with_context(|| format!("reading {url}"))
}

/// Reads the OCI location from registry metadata, in its current form,
/// `{ "oci": { "registry": .., "namespacePrefix": .. } }`, or its older form,
/// `{ "ociRegistry": .., "ociNamespacePrefix": .. }`.
pub fn oci_location(metadata: serde_json::Value) -> Result<OciLocation> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Metadata {
        oci_registry: Option<String>,
        oci_namespace_prefix: Option<String>,
        oci: Option<Oci>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Oci {
        registry: Option<String>,
        namespace_prefix: Option<String>,
    }
    let metadata: Metadata = serde_json::from_value(metadata)?;
    let (registry, prefix) = match metadata.oci {
        Some(oci) => (oci.registry, oci.namespace_prefix),
        None => (metadata.oci_registry, metadata.oci_namespace_prefix),
    };
    Ok(OciLocation {
        registry: registry.ok_or_else(|| anyhow!("the registry metadata names no OCI registry"))?,
        namespace_prefix: prefix.unwrap_or_default(),
    })
}

/// The few GitHub API calls discovery needs.
pub struct GitHub {
    client: Client,
    token: Option<String>,
}

impl GitHub {
    pub fn new(client: Client, token: Option<String>) -> Self {
        Self { client, token }
    }

    /// GETs every page of a list endpoint, `None` when the endpoint answers 403 or 404.
    pub fn list<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Option<Vec<T>>> {
        let mut items = Vec::new();
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut url = format!("https://api.github.com{path}{separator}per_page=100");
        loop {
            let mut request = self
                .client
                .get(&url)
                .header(ACCEPT, "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if let Some(token) = &self.token {
                request = request.header(AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = request.send()?;
            match response.status() {
                StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => {
                    return Ok(None);
                }
                status if !status.is_success() => bail!("GET {url}: {status}"),
                _ => {}
            }
            let next = next_link(response.headers());
            let page: serde_json::Value = response.json()?;
            match page {
                serde_json::Value::Array(_) => {
                    items.extend(serde_json::from_value::<Vec<T>>(page)?)
                }
                // a contents listing of a file rather than a directory
                _ => return Ok(None),
            }
            match next {
                Some(next) => url = next,
                None => return Ok(Some(items)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge() {
        let params = parse_challenge(
            r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:componentized/logging:pull""#,
        )
        .unwrap();
        assert_eq!(params["realm"], "https://ghcr.io/token");
        assert_eq!(params["service"], "ghcr.io");
        assert_eq!(params["scope"], "repository:componentized/logging:pull");
    }

    #[test]
    fn metadata_forms() {
        let expected = OciLocation {
            registry: "ghcr.io".to_string(),
            namespace_prefix: "webassembly/".to_string(),
        };
        let current = serde_json::json!({
            "preferredProtocol": "oci",
            "oci": { "registry": "ghcr.io", "namespacePrefix": "webassembly/" }
        });
        let older = serde_json::json!({
            "ociRegistry": "ghcr.io",
            "ociNamespacePrefix": "webassembly/"
        });
        assert_eq!(oci_location(current).unwrap(), expected);
        assert_eq!(oci_location(older).unwrap(), expected);
        assert!(oci_location(serde_json::json!({})).is_err());
    }

    #[test]
    fn link() {
        let mut headers = HeaderMap::new();
        headers.insert(
            LINK,
            r#"</v2/a/tags/list?last=b&n=1000>; rel="next""#.parse().unwrap(),
        );
        assert_eq!(
            next_link(&headers).as_deref(),
            Some("/v2/a/tags/list?last=b&n=1000")
        );
    }
}
