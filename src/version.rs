//! Classifies OCI tags into the versions the directory indexes.

use semver::Version;

/// The role of an OCI tag.
#[derive(Debug, PartialEq, Eq)]
pub enum Tag {
    /// A stable release, e.g. `0.3.1`.
    Release(Version),
    /// A development build of an upcoming release, e.g. `0.3.2-dev`. The tag moves with each
    /// build until the release is published.
    Dev(Version),
    /// The debug build of a release or development build, e.g. `0.3.1_debug` is the debug build
    /// of `0.3.1`.
    Debug(Version),
    /// Anything else: other pre-releases, build metadata, `v` prefixed duplicates, `dev`, cosign
    /// signatures, ... These are only indexed when a release references them.
    Other,
}

/// Parses a stable semver version, `x.y.z`, without a pre-release or build metadata. The tag must
/// be the canonical form of the version, `v0.1.0` and `00.1.0` are not.
pub fn stable(tag: &str) -> Option<Version> {
    let version = Version::parse(tag).ok()?;
    (version.pre.is_empty() && version.build.is_empty() && version.to_string() == tag)
        .then_some(version)
}

/// Parses a development version, `x.y.z-dev`, in its canonical form.
pub fn dev(tag: &str) -> Option<Version> {
    let version = Version::parse(tag).ok()?;
    (version.pre.as_str() == "dev" && version.build.is_empty() && version.to_string() == tag)
        .then_some(version)
}

/// The release a development version leads up to, e.g. `0.3.2` for `0.3.2-dev`.
pub fn release_of(dev_version: &str) -> Option<String> {
    let version = dev(dev_version)?;
    Some(format!(
        "{}.{}.{}",
        version.major, version.minor, version.patch
    ))
}

pub fn classify(tag: &str) -> Tag {
    if let Some(version) = stable(tag) {
        return Tag::Release(version);
    }
    if let Some(version) = dev(tag) {
        return Tag::Dev(version);
    }
    if let Some(version) = tag
        .strip_suffix("_debug")
        .and_then(|base| stable(base).or_else(|| dev(base)))
    {
        return Tag::Debug(version);
    }
    Tag::Other
}

/// Orders version strings newest first, by semver precedence when they parse.
pub fn cmp_desc(a: &str, b: &str) -> std::cmp::Ordering {
    match (Version::parse(a), Version::parse(b)) {
        (Ok(a), Ok(b)) => b.cmp(&a),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => b.cmp(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn releases() {
        assert_eq!(classify("0.3.1"), Tag::Release(v("0.3.1")));
        assert_eq!(classify("1.0.0"), Tag::Release(v("1.0.0")));
    }

    #[test]
    fn debug_builds() {
        assert_eq!(classify("0.3.1_debug"), Tag::Debug(v("0.3.1")));
        assert_eq!(classify("0.3.1-dev_debug"), Tag::Debug(v("0.3.1-dev")));
        assert_eq!(classify("0.3.1-rc.1_debug"), Tag::Other);
    }

    #[test]
    fn dev_builds() {
        assert_eq!(classify("0.3.2-dev"), Tag::Dev(v("0.3.2-dev")));
        assert_eq!(release_of("0.3.2-dev").as_deref(), Some("0.3.2"));
        assert_eq!(release_of("0.3.2"), None);
    }

    #[test]
    fn ignored() {
        for tag in [
            "0.3.2-dev.1",
            "0.3.2-dev+build",
            "0.2.0-rc.1",
            "0.1.0-draft",
            "0.1.0+build",
            "0.1.0_build",
            "v0.1.0",
            "dev",
            "latest",
            "sha256-5fec5755ff515beb54eec0fc041dbae916f7cbcc8f20fa7b2d99224f8f7165ec",
            "01.0.0",
        ] {
            assert_eq!(classify(tag), Tag::Other, "{tag}");
        }
    }

    #[test]
    fn ordering() {
        let mut versions = vec!["0.2.0", "0.10.0", "0.3.0-dev", "0.3.0"];
        versions.sort_by(|a, b| cmp_desc(a, b));
        assert_eq!(versions, ["0.10.0", "0.3.0", "0.3.0-dev", "0.2.0"]);
    }
}
