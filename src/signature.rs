//! Checks an image's cosign signature, once, the first time a scan sees the image.
//!
//! The signature is found in the registry: a Sigstore bundle, the format cosign 3 writes, which
//! refers to the image it signs and is found by the referrers tag `sha256-{digest}`, or else an
//! older cosign signature, tagged `sha256-{digest}.sig`. `cosign verify` then verifies it: that its
//! certificate is from Sigstore's certificate authority, Fulcio, and that the signature is in
//! Sigstore's transparency log, Rekor, by the log's inclusion proof and signed timestamp. Any
//! identity is accepted, as an image can be signed by another repository than its source, e.g.
//! `WebAssembly/wasi-cli` for the WASI CLI packages in `webassembly/wasi`. The identity is recorded,
//! as its certificate names it, for the directory to show who signed the image.

use crate::data::{Signature, Signer};
use crate::http::Oci;
use anyhow::{Context, Result, anyhow};
use base64::Engine;
use serde::Deserialize;
use std::process::Command;

const BUNDLE: &str = "application/vnd.dev.sigstore.bundle.v0.3+json";

/// Whether `cosign` can be run.
pub fn available() -> bool {
    Command::new("cosign")
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Checks the signature of the image `{registry}/{repository}@{digest}`.
pub fn check(oci: &Oci, registry: &str, repository: &str, digest: &str) -> Result<Signature> {
    let Some(found) = find(oci, registry, repository, digest)? else {
        return Ok(Signature::Unsigned);
    };
    let reference = format!("{registry}/{repository}@{digest}");
    let output = Command::new("cosign")
        .args([
            "verify",
            "--certificate-identity-regexp",
            ".*",
            "--certificate-oidc-issuer-regexp",
            ".*",
            "--output",
            "json",
            &reference,
        ])
        .output()
        .context("running cosign verify")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("cosign verify failed")
            .trim_start_matches("Error: ")
            .to_string();
        return Ok(Signature::Failed { reason });
    }
    let mut signer = signer(&found.certificate)?;
    signer.log_index = found.log_index;
    Ok(Signature::Verified(signer))
}

/// A signature found for an image: its signing certificate, DER, and its transparency log entry.
struct Found {
    certificate: Vec<u8>,
    log_index: Option<u64>,
}

/// Finds the signature of an image, a Sigstore bundle or else an older cosign signature.
fn find(oci: &Oci, registry: &str, repository: &str, digest: &str) -> Result<Option<Found>> {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    // a bundle, by the referrers tag: an index of the artifacts that refer to the image
    if let Some((_, referrers)) =
        oci.find_manifest(registry, repository, &format!("sha256-{hex}"))?
    {
        for referrer in &referrers.manifests {
            let (_, manifest) = oci.manifest(registry, repository, &referrer.digest)?;
            let is_bundle = manifest.artifact_type.as_deref() == Some(BUNDLE);
            let signs = manifest
                .subject
                .as_ref()
                .is_some_and(|s| s.digest == digest);
            let Some(layer) = manifest.layers.iter().find(|l| l.media_type == BUNDLE) else {
                continue;
            };
            if !is_bundle || !signs {
                continue;
            }
            let bytes = oci.blob(registry, repository, &layer.digest)?;
            return bundle(&bytes).map(Some);
        }
    }
    // an older cosign signature: the image's simple signing payload, signed, its certificate and
    // its transparency log entry in the layer's annotations
    if let Some((_, manifest)) =
        oci.find_manifest(registry, repository, &format!("sha256-{hex}.sig"))?
    {
        let layer = manifest
            .layers
            .first()
            .ok_or_else(|| anyhow!("a cosign signature without layers"))?;
        let pem = layer
            .annotations
            .get("dev.sigstore.cosign/certificate")
            .ok_or_else(|| {
                anyhow!("a cosign signature without a certificate, signed with a key")
            })?;
        let (_, pem) = x509_parser::pem::parse_x509_pem(pem.as_bytes())
            .map_err(|err| anyhow!("reading the signature's certificate: {err}"))?;
        #[derive(Deserialize)]
        struct Bundle {
            #[serde(rename = "Payload")]
            payload: Payload,
        }
        #[derive(Deserialize)]
        struct Payload {
            #[serde(rename = "logIndex")]
            log_index: u64,
        }
        let log_index = layer
            .annotations
            .get("dev.sigstore.cosign/bundle")
            .and_then(|bundle| serde_json::from_str::<Bundle>(bundle).ok())
            .map(|bundle| bundle.payload.log_index);
        return Ok(Some(Found {
            certificate: pem.contents,
            log_index,
        }));
    }
    Ok(None)
}

/// The certificate and transparency log entry of a Sigstore bundle.
fn bundle(bytes: &[u8]) -> Result<Found> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Bundle {
        verification_material: Material,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Material {
        certificate: Option<Raw>,
        x509_certificate_chain: Option<Chain>,
        #[serde(default)]
        tlog_entries: Vec<Entry>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        raw_bytes: String,
    }
    #[derive(Deserialize)]
    struct Chain {
        certificates: Vec<Raw>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        log_index: String,
    }
    let bundle: Bundle = serde_json::from_slice(bytes).context("reading the Sigstore bundle")?;
    let material = bundle.verification_material;
    let raw = material
        .certificate
        .or_else(|| {
            material
                .x509_certificate_chain
                .and_then(|c| c.certificates.into_iter().next())
        })
        .ok_or_else(|| anyhow!("a Sigstore bundle without a certificate, signed with a key"))?;
    Ok(Found {
        certificate: base64::engine::general_purpose::STANDARD
            .decode(raw.raw_bytes)
            .context("decoding the bundle's certificate")?,
        log_index: material
            .tlog_entries
            .first()
            .and_then(|entry| entry.log_index.parse().ok()),
    })
}

/// Who a keyless signing certificate was issued to: its subject alternative name, and Fulcio's
/// extensions, e.g. the OIDC issuer, and for GitHub Actions the repository and ref it ran on.
fn signer(der: &[u8]) -> Result<Signer> {
    use x509_parser::prelude::*;
    let (_, cert) = X509Certificate::from_der(der)
        .map_err(|err| anyhow!("reading the signing certificate: {err}"))?;
    let identity = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .and_then(|san| {
            san.value.general_names.iter().find_map(|name| match name {
                GeneralName::URI(uri) => Some(uri.to_string()),
                GeneralName::RFC822Name(email) => Some(email.to_string()),
                _ => None,
            })
        })
        .ok_or_else(|| anyhow!("a signing certificate without an identity"))?;
    // a Fulcio extension's value: DER, e.g. a UTF8String, for the newer ones, `1.3.6.1.4.1.57264.1.8`
    // on; the raw string for the older ones
    let extension = |oid: &str| -> Option<String> {
        let ext = cert
            .extensions()
            .iter()
            .find(|ext| ext.oid.to_id_string() == oid)?;
        let value = ext.value;
        match x509_parser::der_parser::asn1_rs::Utf8String::from_der(value) {
            Ok((_, s)) => Some(s.string()),
            Err(_) => std::str::from_utf8(value).ok().map(ToString::to_string),
        }
    };
    let fulcio = |n: u32| format!("1.3.6.1.4.1.57264.1.{n}");
    let issuer = extension(&fulcio(8))
        .or_else(|| extension(&fulcio(1)))
        .unwrap_or_default();
    // the source repository's URI and ref, or the older GitHub workflow repository and ref
    let repository = extension(&fulcio(12))
        .map(|uri| uri.trim_start_matches("https://github.com/").to_string())
        .or_else(|| extension(&fulcio(5)));
    let git_ref = extension(&fulcio(14)).or_else(|| extension(&fulcio(6)));
    Ok(Signer {
        identity,
        issuer,
        repository,
        git_ref,
        log_index: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_who_signed_a_bundle() {
        // the Sigstore bundle of `ghcr.io/componentized/filesystem/latch-readonly:0.3.0-dev`
        let found = bundle(include_bytes!("testdata/sigstore-bundle.json")).unwrap();
        assert_eq!(found.log_index, Some(3864299460));
        let signer = signer(&found.certificate).unwrap();
        assert_eq!(
            signer.identity,
            "https://github.com/componentized/filesystem/.github/workflows/ci.yaml@refs/heads/main"
        );
        assert_eq!(signer.issuer, "https://token.actions.githubusercontent.com");
        assert_eq!(
            signer.repository.as_deref(),
            Some("componentized/filesystem")
        );
        assert_eq!(signer.git_ref.as_deref(), Some("refs/heads/main"));
    }
}
