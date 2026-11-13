//! A version's API, as the data a compare page diffs against another version's in the browser:
//! each interface's types and functions, each world's imports and exports, and a component's.
//! Comparing in the browser keeps the site to a file per version, rather than a page per pair.
//!
//! Each item is its definition as WIT text, e.g. `record point { x: f64, y: f64 }`, with its
//! members, e.g. `x: f64`, so a change to one field or case can be told apart. A type is referred
//! to by its name alone, and the source of a `use` without its version, so a dependency's new
//! version doesn't change every item that uses it; the dependencies are compared on their own.

use crate::data::Kind;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Api {
    pub package: String,
    pub version: String,
    pub kind: Kind,
    pub interfaces: BTreeMap<String, Interface>,
    pub worlds: BTreeMap<String, World>,
    /// A component's imports and exports.
    pub imports: Vec<Ref>,
    pub exports: Vec<Ref>,
    /// The versions of the packages it depends on, e.g. `wasi:io@0.2.6`.
    pub dependencies: Vec<String>,
    /// Its WIT as its page shows it, highlighted HTML, its types linked to their definitions, for a
    /// line by line diff of its text, which the browser reads from the HTML.
    pub wit: String,
}

#[derive(Serialize)]
pub struct Interface {
    pub deprecated: bool,
    /// The feature its `@unstable(feature = ..)` gate names, e.g. `clocks-timezone`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unstable: Option<String>,
    /// Its types, its functions, e.g. `get-value`, and its resources' functions, e.g.
    /// `canvas.plot`, by name. An interface's types and functions share its names.
    pub items: BTreeMap<String, Item>,
}

#[derive(Serialize)]
pub struct Item {
    /// `record`, `variant`, `enum`, `flags`, `resource`, `type`, `use` or `func`.
    pub kind: &'static str,
    /// The definition as WIT text, e.g. `record point { x: f64, y: f64 }`, which is compared.
    pub def: String,
    /// The definition highlighted, its types linked to their definitions, which is shown.
    pub def_html: String,
    /// A record's fields, a variant's or enum's cases, or a flags' flags, e.g. `x: f64`, and for a
    /// `use`, those of the type it uses.
    pub members: Vec<String>,
    /// The members highlighted, in the same order.
    pub members_html: Vec<String>,
    pub deprecated: bool,
    /// The feature its `@unstable(feature = ..)` gate names: it's only there when the feature is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unstable: Option<String>,
    /// Its anchor on its interface's page, e.g. `type-point` or `func-area`.
    pub anchor: String,
    /// The kind of the type a `use` refers to, e.g. `resource`, which its icon shows, badged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_kind: Option<&'static str>,
    /// A function's modifiers, `static`, `async`, `get`, `set` or `constructor`, which badge its
    /// icon.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<&'static str>,
    /// The first paragraph of its docs, as text, for a tooltip.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// For a `use`, the type it uses: its definition, highlighted, and the interface defining it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// The type a `use` refers to.
#[derive(Serialize)]
pub struct Origin {
    /// Its definition, highlighted, e.g. `type duration = u64`.
    pub def_html: String,
    /// The interface that defines it, e.g. `wasi:clocks/types@0.3.0`.
    pub from: String,
}

/// The first paragraph of rendered docs, as text.
pub fn summary(docs_html: &str) -> String {
    let first = docs_html
        .split_once("</p>")
        .map_or(docs_html, |(first, _)| first);
    text(first)
}

#[derive(Serialize)]
pub struct World {
    pub imports: Vec<Ref>,
    pub exports: Vec<Ref>,
}

/// An item a world imports or exports, e.g. `wasi:io/streams@0.2.6`, keyed without its version.
#[derive(Serialize)]
pub struct Ref {
    pub key: String,
    pub label: String,
    /// An interface of types alone, with no functions and no resources of its own, e.g.
    /// `wasi:cli/types@0.3.0`. Importing it asks nothing of a host, as there's nothing to call.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub types_only: bool,
    /// The feature its `@unstable(feature = ..)` gate names, e.g. `clocks-timezone` for a world's
    /// `wasi:clocks/timezone` import, which only a host with the feature provides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unstable: Option<String>,
}

impl Ref {
    pub fn new(label: &str, types_only: bool, unstable: Option<String>) -> Self {
        Self {
            key: unversioned(label),
            label: label.to_string(),
            types_only,
            unstable,
        }
    }
}

/// The text of rendered HTML: without its tags, its entities decoded, and a signature that runs
/// over lines on one, e.g. `f: func(a: u32, b: u32)`.
pub fn text(html: &str) -> String {
    plain(html)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
}

/// The text of rendered HTML, without its tags and its entities decoded, keeping its lines.
pub fn plain(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            c => text.push(c),
        }
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x2f;", "/")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

/// A WIT reference without its versions, e.g. `wasi:io/streams.{error}` for
/// `wasi:io/streams@0.2.6.{error}`.
pub fn unversioned(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '@' {
            out.push(c);
            continue;
        }
        let mut version = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+') {
                version.push(c);
                chars.next();
            } else {
                break;
            }
        }
        // the `.` that separates a version from what follows it, e.g. `.{error}`
        if version.ends_with('.') {
            out.push('.');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_text_of_html() {
        assert_eq!(
            text(
                r#"<span class="f">read</span>: <span class="k">func</span>(<span class="p">len</span>: <span class="b">u64</span>) -&gt; <span class="b">list</span>&lt;<span class="b">u8</span>&gt;"#
            ),
            "read: func(len: u64) -> list<u8>"
        );
        // a long signature is on one line
        assert_eq!(
            text("f: func(\n  a: u32,\n  b: u32,\n) -&gt; u8"),
            "f: func(a: u32, b: u32) -> u8"
        );
    }

    #[test]
    fn summarizes_docs() {
        assert_eq!(
            summary("<p>A point on a plane.\nIn two dimensions.</p>\n<p>More.</p>"),
            "A point on a plane. In two dimensions."
        );
        assert_eq!(summary(""), "");
    }

    #[test]
    fn drops_versions() {
        assert_eq!(unversioned("wasi:io/streams@0.2.6"), "wasi:io/streams");
        assert_eq!(
            unversioned("wasi:io/streams@0.2.6.{error}"),
            "wasi:io/streams.{error}"
        );
        assert_eq!(
            unversioned("latch0: componentized:http/latch@0.1.0-dev"),
            "latch0: componentized:http/latch"
        );
        assert_eq!(unversioned("run: func"), "run: func");
    }
}
