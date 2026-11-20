//! SPDX license expressions, e.g. `Apache-2.0 WITH LLVM-exception OR MIT`, as a version's SBOM
//! lists them: each license and exception they name, with what the SPDX license list says of it.

use crate::wit_html::escape;
use serde::Serialize;

/// A license or an exception an expression names, as the SPDX license list has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Named {
    /// Its SPDX identifier, e.g. `Apache-2.0`, or a `LicenseRef-…` of the document's own.
    pub id: String,
    /// Its full name, e.g. `Apache License 2.0`, for one on the list.
    pub name: Option<String>,
    /// Its page on the SPDX license list.
    pub url: Option<String>,
    pub exception: bool,
    pub osi: bool,
    pub fsf: bool,
    pub copyleft: bool,
    pub deprecated: bool,
}

impl Named {
    fn license(token: &str) -> Self {
        // `GPL-2.0+`, the older way to say `GPL-2.0-or-later`, which SPDX has deprecated, or `+`
        // on another license, this version of it or any later one
        if let Some(base) = token.strip_suffix('+') {
            if let Some(id) = spdx::gnu_license_id(base, true) {
                return Named {
                    deprecated: true,
                    ..Self::listed(token, id)
                };
            }
            if let Some(id) = spdx::license_id(base) {
                let listed = Self::listed(token, id);
                return Named {
                    name: listed.name.map(|n| format!("{n} or later")),
                    ..listed
                };
            }
            return Self::unlisted(token, false);
        }
        match spdx::license_id(token) {
            Some(id) => Self::listed(token, id),
            None => Self::unlisted(token, false),
        }
    }

    fn listed(token: &str, id: spdx::LicenseId) -> Self {
        Named {
            id: token.to_string(),
            name: Some(id.full_name.to_string()),
            url: Some(format!("https://spdx.org/licenses/{}.html", id.name)),
            exception: false,
            osi: id.is_osi_approved(),
            fsf: id.is_fsf_free_libre(),
            copyleft: id.is_copyleft(),
            deprecated: id.is_deprecated(),
        }
    }

    fn exception(token: &str) -> Self {
        match spdx::exception_id(token) {
            Some(id) => Named {
                id: token.to_string(),
                name: Some(id.name.to_string()),
                url: Some(format!("https://spdx.org/licenses/{}.html", id.name)),
                exception: true,
                osi: false,
                fsf: false,
                copyleft: false,
                deprecated: id.is_deprecated(),
            },
            None => Self::unlisted(token, true),
        }
    }

    fn unlisted(token: &str, exception: bool) -> Self {
        Named {
            id: token.to_string(),
            name: None,
            url: None,
            exception,
            osi: false,
            fsf: false,
            copyleft: false,
            deprecated: false,
        }
    }

    /// What the list says of it, e.g. `Apache License 2.0 · OSI approved · FSF free/libre`.
    pub fn title(&self) -> String {
        let mut parts = vec![match &self.name {
            Some(name) => name.clone(),
            None if self.id.starts_with("LicenseRef-") => "a license of the SBOM's own".to_string(),
            None => "not on the SPDX license list".to_string(),
        }];
        if self.exception {
            parts.push("an exception to the license it's with".to_string());
        }
        if self.osi {
            parts.push("OSI approved".to_string());
        }
        if self.fsf {
            parts.push("FSF free/libre".to_string());
        }
        if self.copyleft {
            parts.push("copyleft".to_string());
        }
        if self.deprecated {
            parts.push("a deprecated identifier".to_string());
        }
        parts.join(" · ")
    }
}

/// A token of an expression.
enum Token<'a> {
    Open,
    Close,
    /// `AND`, `OR` or `WITH`, as written
    Operator(&'a str),
    /// a license, or after `WITH`, an exception
    Name(&'a str),
}

fn tokens(expression: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    for word in expression.split_whitespace() {
        let mut rest = word;
        while let Some(inner) = rest.strip_prefix('(') {
            tokens.push(Token::Open);
            rest = inner;
        }
        let closes = rest.len() - rest.trim_end_matches(')').len();
        let rest = rest.trim_end_matches(')');
        // `MIT/Apache-2.0`, a loose `OR`
        for (i, part) in rest.split('/').enumerate() {
            if i > 0 {
                tokens.push(Token::Operator("/"));
            }
            if !part.is_empty() {
                tokens.push(match part.to_ascii_uppercase().as_str() {
                    "AND" | "OR" | "WITH" => Token::Operator(part),
                    _ => Token::Name(part),
                });
            }
        }
        tokens.extend((0..closes).map(|_| Token::Close));
    }
    tokens
}

/// Whether an expression is one SPDX allows, strictly or as it's commonly loosened, e.g. with
/// `/` for `OR`. Only a valid one is read for the licenses it names.
fn valid(expression: &str) -> bool {
    spdx::Expression::parse(expression).is_ok()
        || spdx::Expression::parse_mode(expression, spdx::ParseMode::LAX).is_ok()
}

/// The licenses and exceptions an expression names, each once, in order. None for one that isn't
/// a valid expression, which is shown as it's written.
pub fn named(expression: &str) -> Vec<Named> {
    if !valid(expression) {
        return Vec::new();
    }
    let mut named: Vec<Named> = Vec::new();
    let mut after_with = false;
    for token in tokens(expression) {
        match token {
            Token::Operator(op) => after_with = op.eq_ignore_ascii_case("WITH"),
            Token::Name(name) => {
                let item = if after_with {
                    Named::exception(name)
                } else {
                    Named::license(name)
                };
                if !named.contains(&item) {
                    named.push(item);
                }
                after_with = false;
            }
            Token::Open | Token::Close => after_with = false,
        }
    }
    named
}

/// An expression as HTML: each license and exception it names linked to its page on the SPDX
/// license list, titled with what the list says of it, and its operators apart. One that isn't a
/// valid expression is shown as it's written.
pub fn html(expression: &str) -> String {
    if named(expression).is_empty() {
        return escape(expression);
    }
    let mut out = String::new();
    let mut after_with = false;
    let mut space = false;
    for token in tokens(expression) {
        let piece = match token {
            Token::Open => {
                let p = if space { " (" } else { "(" };
                out.push_str(p);
                space = false;
                continue;
            }
            Token::Close => {
                out.push(')');
                continue;
            }
            Token::Operator(op) => {
                after_with = op.eq_ignore_ascii_case("WITH");
                format!(r#"<span class="license-op">{}</span>"#, escape(op))
            }
            Token::Name(name) => {
                let item = if after_with {
                    Named::exception(name)
                } else {
                    Named::license(name)
                };
                after_with = false;
                let class = if item.copyleft {
                    "license copyleft"
                } else {
                    "license"
                };
                match &item.url {
                    Some(url) => format!(
                        r#"<a class="{class}" href="{}" title="{}">{}</a>"#,
                        escape(url),
                        escape(&item.title()),
                        escape(name)
                    ),
                    None => format!(
                        r#"<span class="{class}" title="{}">{}</span>"#,
                        escape(&item.title()),
                        escape(name)
                    ),
                }
            }
        };
        if space {
            out.push(' ');
        }
        out.push_str(&piece);
        space = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_each_license_and_exception() {
        let apache_llvm = named("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT");
        let ids: Vec<_> = apache_llvm
            .iter()
            .map(|n| (n.id.as_str(), n.exception))
            .collect();
        assert_eq!(
            ids,
            [
                ("Apache-2.0", false),
                ("LLVM-exception", true),
                ("MIT", false)
            ]
        );
        let apache = &apache_llvm[0];
        assert_eq!(apache.name.as_deref(), Some("Apache License 2.0"));
        assert!(apache.osi && apache.fsf && !apache.copyleft);
        assert_eq!(
            apache.url.as_deref(),
            Some("https://spdx.org/licenses/Apache-2.0.html")
        );
        // copyleft, and the older `+` for `-or-later`, deprecated, or on another license
        let gpl = &named("GPL-2.0+")[0];
        assert!(gpl.copyleft && gpl.deprecated);
        assert_eq!(
            gpl.name.as_deref(),
            Some("GNU General Public License v2.0 or later")
        );
        assert_eq!(
            gpl.url.as_deref(),
            Some("https://spdx.org/licenses/GPL-2.0-or-later.html")
        );
        assert_eq!(
            named("Apache-2.0+")[0].name.as_deref(),
            Some("Apache License 2.0 or later")
        );
        assert!(named("MPL-2.0")[0].copyleft);
        // a license of the SBOM's own isn't on the list
        let own = &named("LicenseRef-example AND MIT")[0];
        assert_eq!((own.name.as_ref(), own.url.as_ref()), (None, None));
        // a loose `/` for `OR`
        assert_eq!(named("MIT/Apache-2.0").len(), 2);
        // what isn't an expression names nothing
        assert!(named("Some custom terms").is_empty());
    }

    #[test]
    fn renders_an_expression() {
        let html = html("(MIT OR Apache-2.0) AND Unicode-3.0");
        assert!(html.starts_with(r#"(<a class="license" href="https://spdx.org/licenses/MIT.html" title="MIT License · OSI approved · FSF free/libre">MIT</a> <span class="license-op">OR</span> "#));
        assert!(html.contains(r#"Apache-2.0</a>) <span class="license-op">AND</span> <a class="license" href="https://spdx.org/licenses/Unicode-3.0.html""#));
        assert!(super::html("GPL-3.0-only").contains(r#"class="license copyleft""#));
        assert_eq!(
            super::html("Some <custom> terms"),
            "Some &lt;custom&gt; terms"
        );
    }
}
