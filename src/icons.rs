//! The site's icons as standalone SVG files, for pages elsewhere that link to the directory:
//! `/_icons/{variant}/{name}.svg`, e.g. `/_icons/neutral/component.svg`. A WIT namespace can't
//! hold an `_`, so the directory never collides with a namespace's pages, `/{namespace}/`.
//!
//! They're drawn from the sprite every page inlines, `kind_sprite` in `templates/macros.html`, so
//! they stay the icons the site shows. Each comes in three variants:
//!
//! - `light` and `dark`, in the colors of the site's light and dark themes
//! - `neutral`, for a page whose theme isn't known: colors between the two, of at least 4.5:1
//!   contrast against both white and black
//!
//! On the site, a badge on an icon is parted from it by a halo in the page's background. A file
//! can't know its background, so the halo is cut out of the icon underneath instead, leaving it
//! transparent, and a badge's fill is translucent rather than blended toward the background.

use anyhow::{Result, anyhow};
use serde::Serialize;
use std::collections::HashMap;

/// The colors of a variant, the site's color tokens of the same names.
struct Palette {
    /// types
    t: &'static str,
    /// resources and namespaces, and the `static` badge
    n: &'static str,
    /// keywords, the badge of a used type and the `constructor` badge
    k: &'static str,
    /// functions
    f: &'static str,
    /// the `async` badge
    b: &'static str,
    component: &'static str,
    interface: &'static str,
    /// repositories
    accent: &'static str,
}

/// The variants, by name. `light` and `dark` are the tokens of `static/style.css`; `neutral` is
/// each token's light and dark colors mixed, in Oklab, to the color with the most contrast
/// against both white and black, 4.58:1.
const VARIANTS: [(&str, Palette); 3] = [
    (
        "light",
        Palette {
            t: "#1f6f86",
            n: "#7a4cc2",
            k: "#a626a4",
            f: "#3b7d1f",
            b: "#b04a00",
            component: "#0f7b6c",
            interface: "#8a5a00",
            accent: "#5b3fd4",
        },
    ),
    (
        "dark",
        Palette {
            t: "#5cc4e0",
            n: "#c3a6ff",
            k: "#e08be0",
            f: "#8ad36a",
            b: "#ff9f5a",
            component: "#5fd0bd",
            interface: "#f0bf5e",
            accent: "#a995ff",
        },
    ),
    (
        "neutral",
        Palette {
            t: "#2b7f97",
            n: "#885fcf",
            k: "#b747b5",
            f: "#428426",
            b: "#bd5817",
            component: "#1b8474",
            interface: "#9d6c18",
            accent: "#7561e4",
        },
    ),
];

/// How much of its color a badge keeps, the site's `--badge-strength`.
const BADGE_OPACITY: &str = "0.7";
/// The size of a badge, and how far it hangs past the icon's edge, as on the site.
const BADGE_SIZE: f32 = 11.0;
const BADGE_OVERHANG: f32 = 5.0;
/// The side of every icon's frame: the icon's 16, and the badges' overhang on either side.
const FRAME: f32 = 16.0 + 2.0 * BADGE_OVERHANG;

/// Where a badge sits on an icon.
#[derive(Clone, Copy)]
enum Corner {
    TopRight,
    BottomRight,
    BottomLeft,
}

impl Corner {
    fn origin(self) -> (f32, f32) {
        let near = -BADGE_OVERHANG;
        let far = 16.0 + BADGE_OVERHANG - BADGE_SIZE;
        match self {
            Corner::TopRight => (far, near),
            Corner::BottomRight => (far, far),
            Corner::BottomLeft => (near, far),
        }
    }
}

/// How a badge is drawn.
#[derive(Clone, Copy)]
enum BadgeStyle {
    /// a translucent filled glyph in a color, parted from the icon by a halo
    Filled { color: fn(&Palette) -> &'static str },
    /// a component's outline, the badge of a composition
    Outline,
}

struct Badge {
    symbol: &'static str,
    corner: Corner,
    style: BadgeStyle,
}

/// An icon: a symbol of the sprite in a color, with its badges.
struct Icon {
    name: String,
    /// what it stands for, for the icons' index page
    label: String,
    symbol: String,
    color: fn(&Palette) -> &'static str,
    badges: Vec<Badge>,
}

/// The icons, the kinds `kind_icon` and `func_icon` show.
fn icons() -> Vec<Icon> {
    let plain = |kind: &str, label: &str, color: fn(&Palette) -> &'static str| Icon {
        name: kind.to_string(),
        label: label.to_string(),
        symbol: format!("kind-{kind}"),
        color,
        badges: Vec::new(),
    };
    let mut icons = vec![
        plain("namespace", "namespace", |p| p.n),
        plain("repo", "repository", |p| p.accent),
        plain("component", "component", |p| p.component),
        plain("package", "WIT package", |p| p.interface),
        plain("world", "world", |p| p.component),
        plain("interface", "interface", |p| p.interface),
        plain("resource", "resource", |p| p.n),
        plain("record", "record", |p| p.t),
        plain("variant", "variant", |p| p.t),
        plain("enum", "enum", |p| p.t),
        plain("flags", "flags", |p| p.t),
        plain("type", "type alias", |p| p.t),
        plain("use", "used type", |p| p.k),
        plain("func", "function", |p| p.f),
    ];
    icons.push(Icon {
        name: "composition".to_string(),
        label: "composition, a component nested in a component".to_string(),
        symbol: "kind-component".to_string(),
        color: |p| p.component,
        badges: vec![Badge {
            symbol: "kind-component",
            corner: Corner::BottomRight,
            style: BadgeStyle::Outline,
        }],
    });
    // a type used from another interface: the icon of the type it uses, badged as used
    for kind in ["resource", "record", "variant", "enum", "flags", "type"] {
        let base = icons.iter().find(|icon| icon.name == kind).unwrap();
        let (color, label) = (base.color, base.label.clone());
        icons.push(Icon {
            name: format!("use-{kind}"),
            label: format!("used {label}"),
            symbol: format!("kind-{kind}"),
            color,
            badges: vec![Badge {
                symbol: "kind-used",
                corner: Corner::BottomRight,
                style: BadgeStyle::Filled { color: |p| p.k },
            }],
        });
    }
    // a function's modifiers, the combinations `wit_html::modifiers` gives
    for mods in [
        &["async"][..],
        &["get"],
        &["set"],
        &["static"],
        &["static", "async"],
        &["static", "get"],
        &["static", "set"],
        &["constructor"],
    ] {
        let badges = mods
            .iter()
            .map(|m| {
                let (corner, color): (Corner, fn(&Palette) -> &'static str) = match *m {
                    "static" => (Corner::TopRight, |p| p.n),
                    "async" => (Corner::BottomRight, |p| p.b),
                    "constructor" => (Corner::BottomLeft, |p| p.k),
                    _ => (Corner::BottomLeft, |p| p.t),
                };
                Badge {
                    symbol: match *m {
                        "static" => "mod-static",
                        "async" => "mod-async",
                        "get" => "mod-get",
                        "set" => "mod-set",
                        _ => "mod-constructor",
                    },
                    corner,
                    style: BadgeStyle::Filled { color },
                }
            })
            .collect();
        icons.push(Icon {
            name: format!("func-{}", mods.join("-")),
            label: format!("{} function", mods.join(" ")),
            symbol: "kind-func".to_string(),
            color: |p| p.f,
            badges,
        });
    }
    icons
}

/// The symbols of the sprite, by id: each one's contents, drawn in a 16 by 16 box.
fn symbols(sprite: &str) -> Result<HashMap<&str, &str>> {
    let mut symbols = HashMap::new();
    let mut rest = sprite;
    while let Some(start) = rest.find("<symbol id=\"") {
        rest = &rest[start + "<symbol id=\"".len()..];
        let (id, after) = rest
            .split_once('"')
            .ok_or_else(|| anyhow!("a symbol's id isn't closed"))?;
        if !after.starts_with(" viewBox=\"0 0 16 16\">") {
            anyhow::bail!("the symbol {id} isn't drawn in a 16 by 16 box");
        }
        let body = &after[" viewBox=\"0 0 16 16\">".len()..];
        let end = body
            .find("</symbol>")
            .ok_or_else(|| anyhow!("the symbol {id} isn't closed"))?;
        symbols.insert(id, &body[..end]);
        rest = &body[end..];
    }
    Ok(symbols)
}

/// A symbol's contents in a color, as the site's `.kind-icon` draws them: stroked, except the dots
/// of a variant's and an enum's icons, which are filled.
fn stroked(symbol: &str, contents: &str, color: &str) -> String {
    if matches!(symbol, "kind-variant" | "kind-enum") {
        contents.replace(
            "<circle ",
            &format!(r#"<circle fill="{color}" stroke="none" "#),
        )
    } else {
        contents.to_string()
    }
}

fn number(n: f32) -> String {
    let s = format!("{n:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// An icon as a standalone SVG file in a variant's colors.
fn render(
    icon: &Icon,
    variant: &str,
    palette: &Palette,
    symbols: &HashMap<&str, &str>,
) -> Result<String> {
    // the mask's id is the file's own, should a page inline several of them
    let mask = format!("{variant}-{}-halo", icon.name);
    let symbol = |id: &str| {
        symbols
            .get(id)
            .copied()
            .ok_or_else(|| anyhow!("the sprite has no symbol {id}"))
    };
    let color = (icon.color)(palette);

    // every icon shares one frame: the icon's 16 by 16 box in the middle, and a margin all round
    // for the badges that hang past its edges, so an icon sits in the same place, at the same size,
    // with or without badges
    let (x, y, side) = (-BADGE_OVERHANG, -BADGE_OVERHANG, FRAME);
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{0}" height="{0}" viewBox="{1} {1} {0} {0}">"#,
        number(FRAME),
        number(-BADGE_OVERHANG)
    );
    let place = |badge: &Badge| {
        let (bx, by) = badge.corner.origin();
        format!(
            "translate({} {}) scale({})",
            number(bx),
            number(by),
            number(BADGE_SIZE / 16.0)
        )
    };
    let base = format!(
        r#"fill="none" stroke="{color}" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round""#
    );
    if icon.badges.is_empty() {
        svg.push_str(&format!(
            "<g {base}>{}</g>",
            stroked(&icon.symbol, symbol(&icon.symbol)?, color)
        ));
    } else {
        // each badge's halo, cut out of the icon
        svg.push_str(&format!(
            r##"<defs><mask id="{3}" maskUnits="userSpaceOnUse" x="{0}" y="{1}" width="{2}" height="{2}"><rect x="{0}" y="{1}" width="{2}" height="{2}" fill="#fff"/>"##,
            number(x),
            number(y),
            number(side),
            mask
        ));
        for badge in &icon.badges {
            let (fill, width) = match badge.style {
                BadgeStyle::Filled { .. } => ("#000", "3.2"),
                BadgeStyle::Outline => ("none", "5"),
            };
            svg.push_str(&format!(
                r##"<g transform="{}" fill="{fill}" stroke="#000" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round">{}</g>"##,
                place(badge),
                symbol(badge.symbol)?
            ));
        }
        svg.push_str("</mask></defs>");
        svg.push_str(&format!(
            r#"<g mask="url(#{mask})" {base}>{}</g>"#,
            stroked(&icon.symbol, symbol(&icon.symbol)?, color)
        ));
        for badge in &icon.badges {
            let style = match badge.style {
                BadgeStyle::Filled { color } => format!(
                    r#"fill="{}" fill-opacity="{BADGE_OPACITY}" stroke="none""#,
                    color(palette)
                ),
                BadgeStyle::Outline => format!(
                    r#"fill="none" stroke="{}" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round""#,
                    palette.component
                ),
            };
            svg.push_str(&format!(
                r#"<g transform="{}" {style}>{}</g>"#,
                place(badge),
                symbol(badge.symbol)?
            ));
        }
    }
    svg.push_str("</svg>\n");
    Ok(svg)
}

/// An icon, as the icons' index page lists it.
#[derive(Serialize)]
pub struct Listed {
    pub name: String,
    pub label: String,
}

/// A file's path under `/_icons/`, e.g. `neutral/component.svg`, and its contents.
pub type File = (String, String);

/// The icons' files, and the icons.
pub fn files(sprite: &str) -> Result<(Vec<File>, Vec<Listed>)> {
    let symbols = symbols(sprite)?;
    let icons = icons();
    let mut files = Vec::new();
    for (variant, palette) in &VARIANTS {
        for icon in &icons {
            files.push((
                format!("{variant}/{}.svg", icon.name),
                render(icon, variant, palette, &symbols)?,
            ));
        }
    }
    let listed = icons
        .into_iter()
        .map(|icon| Listed {
            name: icon.name,
            label: icon.label,
        })
        .collect();
    Ok((files, listed))
}

/// The variants' names, e.g. `neutral`.
pub fn variants() -> Vec<&'static str> {
    VARIANTS.iter().map(|(name, _)| *name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPRITE: &str = include_str!("../templates/macros.html");

    fn luminance(hex: &str) -> f64 {
        let channel = |i: usize| {
            let c = f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn neutral_colors_read_on_light_and_dark() {
        let (_, neutral) = &VARIANTS[2];
        for color in [
            neutral.t,
            neutral.n,
            neutral.k,
            neutral.f,
            neutral.b,
            neutral.component,
            neutral.interface,
            neutral.accent,
        ] {
            // the contrast WCAG asks of text, more than the 3:1 it asks of icons
            assert!(contrast(color, "#ffffff") >= 4.5, "{color} on white");
            assert!(contrast(color, "#000000") >= 4.5, "{color} on black");
            // and on the site's own backgrounds
            assert!(
                contrast(color, "#fbfbfa") >= 4.0,
                "{color} on the light theme"
            );
            assert!(
                contrast(color, "#131416") >= 4.0,
                "{color} on the dark theme"
            );
        }
    }

    #[test]
    fn variants_match_the_site_tokens() {
        let style = include_str!("../static/style.css");
        let (_, light) = &VARIANTS[0];
        let (_, dark) = &VARIANTS[1];
        for (token, light, dark) in [
            ("t", light.t, dark.t),
            ("n", light.n, dark.n),
            ("k", light.k, dark.k),
            ("f", light.f, dark.f),
            ("b", light.b, dark.b),
            ("component", light.component, dark.component),
            ("interface", light.interface, dark.interface),
            ("accent", light.accent, dark.accent),
        ] {
            let declared = format!("--{token}: light-dark({light}, {dark});");
            assert!(style.contains(&declared), "{declared}");
        }
    }

    #[test]
    fn renders_every_icon() {
        let (files, listed) = files(SPRITE).unwrap();
        assert_eq!(files.len(), listed.len() * VARIANTS.len());
        let file = |path: &str| &files.iter().find(|(p, _)| p == path).unwrap().1;

        let component = file("neutral/component.svg");
        assert!(component.starts_with(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="26" height="26" viewBox="-5 -5 26 26">"#
        ));
        assert!(component.contains(r##"stroke="#1b8474""##));
        assert!(!component.contains("mask"));

        // the dots of a variant are filled, a world's circle isn't
        assert!(file("light/variant.svg").contains(r##"<circle fill="#1f6f86" stroke="none" "##));
        assert!(!file("light/world.svg").contains("<circle fill"));

        // a badge hangs past the icon, inside the frame, its halo cut out of the icon
        let used = file("dark/use-record.svg");
        assert!(used.contains(r#"<mask id="dark-use-record-halo""#));
        assert!(used.contains(r#"mask="url(#dark-use-record-halo)""#));
        assert!(used.contains(r##"fill="#e08be0" fill-opacity="0.7""##));

        // every icon shares the frame, badged or not, so the icon doesn't move
        for (path, svg) in &files {
            assert!(
                svg.contains(r#"width="26" height="26" viewBox="-5 -5 26 26""#),
                "{path}"
            );
        }
    }
}
