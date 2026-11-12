//! Prints WIT as syntax highlighted HTML, through the WIT printer of wit-component.

use anyhow::Result;
use std::collections::HashMap;
use wit_component::{Output, TypeKind, WitPrinter};
use wit_parser::{
    Function, FunctionKind, Handle, InterfaceId, PackageId, Resolve, Type, TypeDefKind, TypeId,
};

#[derive(Default)]
pub struct Html {
    indent: usize,
    output: String,
    needs_indent: bool,
    /// The url of each type a reference links to, by name.
    links: HashMap<String, String>,
}

impl Html {
    fn span(&mut self, class: &str, src: &str) {
        self.indent_if_needed();
        self.output.push_str(&format!("<span class=\"{class}\">"));
        self.push_str(src);
        self.output.push_str("</span>");
    }
}

pub fn escape(src: &str) -> String {
    let mut escaped = String::with_capacity(src.len());
    for c in src.chars() {
        match c {
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            c => escaped.push(c),
        }
    }
    escaped
}

impl Output for Html {
    fn push_str(&mut self, src: &str) {
        self.output.push_str(&escape(src));
    }

    fn indent_if_needed(&mut self) -> bool {
        if self.needs_indent {
            for _ in 0..self.indent {
                self.output.push_str("  ");
            }
            self.needs_indent = false;
            true
        } else {
            false
        }
    }

    fn indent_start(&mut self) {
        self.output.push_str(" {");
        self.indent += 1;
        self.newline();
    }

    fn indent_end(&mut self) {
        self.indent = self.indent.saturating_sub(1);
        self.indent_if_needed();
        self.output.push('}');
        self.newline();
    }

    fn newline(&mut self) {
        self.output.push('\n');
        self.needs_indent = true;
    }

    fn keyword(&mut self, src: &str) {
        self.span("k", src);
    }

    fn ty(&mut self, src: &str, kind: TypeKind) {
        // a reference to a named type, or a resource in a handle, links to the type's definition
        if matches!(kind, TypeKind::Other | TypeKind::Resource)
            && let Some(url) = self.links.get(src.trim_start_matches('%')).cloned()
        {
            self.indent_if_needed();
            let html = format!(
                "<a class=\"t\" href=\"{}\">{}</a>",
                escape(&url),
                escape(src)
            );
            self.output.push_str(&html);
            return;
        }
        let class = match kind {
            TypeKind::BuiltIn => "b",
            TypeKind::NamespaceDeclaration
            | TypeKind::NamespacePath
            | TypeKind::PackageNameDeclaration
            | TypeKind::PackageNamePath
            | TypeKind::VersionDeclaration
            | TypeKind::VersionPath
            | TypeKind::VersionAnnotation => "n",
            TypeKind::FunctionFreestanding
            | TypeKind::FunctionMethod
            | TypeKind::FunctionStatic => "f",
            TypeKind::InterfaceDeclaration
            | TypeKind::InterfacePath
            | TypeKind::WorldDeclaration => "i",
            _ => "t",
        };
        self.span(class, src);
    }

    fn param(&mut self, src: &str) {
        self.span("p", src);
    }

    fn case(&mut self, src: &str) {
        self.span("c", src);
    }

    fn doc(&mut self, doc: &str) {
        self.indent_if_needed();
        let doc = if doc.is_empty() {
            "///".to_string()
        } else {
            format!("/// {doc}")
        };
        self.span("d", &doc);
        self.newline();
    }
}

/// A package, followed by the foreign packages it uses when `nested`.
pub fn package(resolve: &Resolve, pkg: PackageId, nested: bool) -> Result<String> {
    let nested: Vec<_> = if nested {
        resolve
            .topological_packages()
            .into_iter()
            .filter(|id| *id != pkg)
            .collect()
    } else {
        Vec::new()
    };
    let mut printer = WitPrinter::new(Html::default());
    printer.print(resolve, pkg, &nested)?;
    Ok(printer.output.output)
}

/// A single interface, `interface name { ... }`, with each type it names linked to the url
/// in `links`.
pub fn interface(
    resolve: &Resolve,
    id: InterfaceId,
    name: &str,
    links: HashMap<String, String>,
) -> Result<String> {
    let mut printer = WitPrinter::new(Html {
        links,
        ..Html::default()
    });
    printer.print_interface_outer(resolve, id, name)?;
    printer.output.indent_start();
    printer.print_interface(resolve, id)?;
    printer.output.indent_end();
    Ok(printer.output.output)
}

/// The url of a named type's definition, if it has a page.
pub type Linker<'a> = &'a dyn Fn(TypeId) -> Option<String>;

/// A reference to a type, e.g. `list<descriptor>`, each named type linked to its definition.
pub fn type_ref(resolve: &Resolve, ty: &Type, link: Linker) -> String {
    let builtin = |name: &str| format!("<span class=\"b\">{name}</span>");
    match ty {
        Type::Bool => builtin("bool"),
        Type::U8 => builtin("u8"),
        Type::U16 => builtin("u16"),
        Type::U32 => builtin("u32"),
        Type::U64 => builtin("u64"),
        Type::S8 => builtin("s8"),
        Type::S16 => builtin("s16"),
        Type::S32 => builtin("s32"),
        Type::S64 => builtin("s64"),
        Type::F32 => builtin("f32"),
        Type::F64 => builtin("f64"),
        Type::Char => builtin("char"),
        Type::String => builtin("string"),
        Type::ErrorContext => builtin("error-context"),
        Type::Id(id) => match &resolve.types[*id].name {
            Some(name) => match link(*id) {
                Some(url) => format!(
                    "<a class=\"t\" href=\"{}\">{}</a>",
                    escape(&url),
                    escape(name)
                ),
                None => format!("<span class=\"t\">{}</span>", escape(name)),
            },
            None => structure(resolve, &resolve.types[*id].kind, link),
        },
    }
}

/// The structure of an anonymous type, its generic names highlighted as builtins like the WIT
/// printer does,, or what a named type aliases, e.g. `option<string>`.
/// Empty for a record, variant, enum, flags or resource, which are only named.
pub fn structure(resolve: &Resolve, kind: &TypeDefKind, link: Linker) -> String {
    let generic = |name: &str, args: Vec<String>| {
        format!("<span class=\"b\">{name}</span>&lt;{}&gt;", args.join(", "))
    };
    let r = |ty: &Type| type_ref(resolve, ty, link);
    match kind {
        TypeDefKind::List(ty) => generic("list", vec![r(ty)]),
        TypeDefKind::FixedLengthList(ty, len) => generic("list", vec![r(ty), len.to_string()]),
        TypeDefKind::Option(ty) => generic("option", vec![r(ty)]),
        TypeDefKind::Result(result) => match (&result.ok, &result.err) {
            (None, None) => "<span class=\"b\">result</span>".to_string(),
            (Some(ok), None) => generic("result", vec![r(ok)]),
            (None, Some(err)) => generic("result", vec!["_".to_string(), r(err)]),
            (Some(ok), Some(err)) => generic("result", vec![r(ok), r(err)]),
        },
        TypeDefKind::Tuple(tuple) => generic("tuple", tuple.types.iter().map(r).collect()),
        TypeDefKind::Map(key, value) => generic("map", vec![r(key), r(value)]),
        TypeDefKind::Handle(Handle::Own(id)) => r(&Type::Id(*id)),
        TypeDefKind::Handle(Handle::Borrow(id)) => generic("borrow", vec![r(&Type::Id(*id))]),
        TypeDefKind::Future(None) => "<span class=\"b\">future</span>".to_string(),
        TypeDefKind::Future(Some(ty)) => generic("future", vec![r(ty)]),
        TypeDefKind::Stream(None) => "<span class=\"b\">stream</span>".to_string(),
        TypeDefKind::Stream(Some(ty)) => generic("stream", vec![r(ty)]),
        TypeDefKind::Type(ty) => r(ty),
        TypeDefKind::Record(_)
        | TypeDefKind::Variant(_)
        | TypeDefKind::Enum(_)
        | TypeDefKind::Flags(_)
        | TypeDefKind::Resource
        | TypeDefKind::Unknown => String::new(),
    }
}

/// A function's modifiers, as badges: `static`, `async`, `get`, `set` or `constructor`. A static
/// function may also be async, a getter or a setter; a constructor is only a constructor.
pub fn modifiers(kind: &FunctionKind) -> Vec<&'static str> {
    use FunctionKind::*;
    match kind {
        Freestanding | Method(_) => vec![],
        AsyncFreestanding | AsyncMethod(_) => vec!["async"],
        Getter | MethodGetter(_) => vec!["get"],
        Setter | MethodSetter(_) => vec!["set"],
        Static(_) => vec!["static"],
        AsyncStatic(_) => vec!["static", "async"],
        StaticGetter(_) => vec!["static", "get"],
        StaticSetter(_) => vec!["static", "set"],
        Constructor(_) => vec!["constructor"],
    }
}

/// A function's signature as WIT writes it, e.g. `read: async func(len: u64) -> list<u8>`,
/// `new: static func()`, `size: get() -> u64` or `constructor(path: string)`.
pub fn signature(resolve: &Resolve, func: &Function, link: Linker) -> String {
    use FunctionKind::*;
    let keyword = |k: &str| format!("<span class=\"k\">{k}</span>");
    let mut html = String::new();
    if !matches!(func.kind, Constructor(_)) {
        html.push_str(&format!(
            "<span class=\"f\">{}</span>: ",
            escape(func.item_name())
        ));
    }
    if matches!(
        func.kind,
        Static(_) | AsyncStatic(_) | StaticGetter(_) | StaticSetter(_)
    ) {
        html.push_str(&keyword("static"));
        html.push(' ');
    }
    if func.kind.is_async() {
        html.push_str(&keyword("async"));
        html.push(' ');
    }
    html.push_str(&keyword(match func.kind {
        Constructor(_) => "constructor",
        Getter | MethodGetter(_) | StaticGetter(_) => "get",
        Setter | MethodSetter(_) | StaticSetter(_) => "set",
        _ => "func",
    }));
    html.push('(');
    // a method's `self` is implied
    let skip_self = matches!(
        func.kind,
        Method(_) | AsyncMethod(_) | MethodGetter(_) | MethodSetter(_)
    );
    let params: Vec<_> = func
        .params
        .iter()
        .skip(usize::from(skip_self))
        .map(|param| {
            format!(
                "<span class=\"p\">{}</span>: {}",
                escape(&param.name),
                type_ref(resolve, &param.ty, link)
            )
        })
        .collect();
    let result = match (&func.result, matches!(func.kind, Constructor(_))) {
        (Some(result), false) => format!(" -&gt; {}", type_ref(resolve, result, link)),
        _ => String::new(),
    };
    // a signature that would run long puts each parameter on a line of its own, with a trailing
    // comma, the way rustfmt formats a long Rust signature
    let one_line = format!("{html}{}){result}", params.join(", "));
    if text_len(&one_line) <= SIGNATURE_WIDTH || params.is_empty() {
        return one_line;
    }
    for param in &params {
        html.push_str("\n  ");
        html.push_str(param);
        html.push(',');
    }
    html.push_str("\n)");
    html.push_str(&result);
    html
}

/// The width a signature fits on one line within, in characters.
const SIGNATURE_WIDTH: usize = 72;

/// The length of the text of HTML, without its tags and with its entities as one character.
fn text_len(html: &str) -> usize {
    let mut len = 0;
    let mut in_tag = false;
    let mut in_entity = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            '&' => {
                in_entity = true;
                len += 1;
            }
            ';' if in_entity => in_entity = false,
            _ if in_entity => {}
            _ => len += 1,
        }
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_combinations() {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_str(
                "r.wit",
                "package example:r@0.1.0;\ninterface i { resource r; }\n",
            )
            .unwrap();
        let r = resolve.interfaces[resolve.packages[pkg].interfaces["i"]].types["r"];
        use FunctionKind::*;
        for (kind, expected) in [
            (Freestanding, vec![]),
            (Method(r), vec![]),
            (AsyncFreestanding, vec!["async"]),
            (AsyncMethod(r), vec!["async"]),
            (Static(r), vec!["static"]),
            (AsyncStatic(r), vec!["static", "async"]),
            (MethodGetter(r), vec!["get"]),
            (StaticSetter(r), vec!["static", "set"]),
            (Constructor(r), vec!["constructor"]),
        ] {
            assert_eq!(modifiers(&kind), expected, "{kind:?}");
        }
    }

    #[test]
    fn signatures_as_wit_writes_them() {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_str(
                "r.wit",
                r#"
package example:r@0.1.0;

interface i {
  resource r {
    constructor(path: string);
    open: static async func(path: string) -> r;
    read: async func(len: u64) -> list<u8>;
    write: func(contents: list<u8>, offset: u64, options: option<string>) -> result<u64, string>;
  }
}
"#,
            )
            .unwrap();
        let iface = resolve.packages[pkg].interfaces["i"];
        let none = |_: TypeId| None;
        let sig = |name: &str| {
            let html = signature(&resolve, &resolve.interfaces[iface].functions[name], &none);
            let text = html
                .split('<')
                .map(|part| part.split_once('>').map_or(part, |(_, text)| text))
                .collect::<String>();
            text.replace("&lt;", "<").replace("&gt;", ">")
        };
        assert_eq!(sig("[constructor]r"), "constructor(path: string)");
        assert_eq!(
            sig("[static]r.open"),
            "open: static async func(path: string) -> r"
        );
        assert_eq!(
            sig("[method]r.read"),
            "read: async func(len: u64) -> list<u8>"
        );
        assert_eq!(
            sig("[method]r.write"),
            "write: func(\n  contents: list<u8>,\n  offset: u64,\n  options: option<string>,\n) -> result<u64, string>"
        );
    }

    #[test]
    fn links_named_types() {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_str(
                "types.wit",
                r#"
package example:types@0.1.0;

interface clocks {
  record datetime { seconds: u64 }
}

interface fs {
  use clocks.{datetime};
  resource descriptor;
  stat: func(fd: borrow<descriptor>) -> result<option<datetime>, string>;
}
"#,
            )
            .unwrap();
        let fs = resolve.packages[pkg].interfaces["fs"];
        let func = &resolve.interfaces[fs].functions["stat"];
        let link = |id: TypeId| {
            resolve.types[id]
                .name
                .as_ref()
                .map(|n| format!("#type-{n}"))
        };
        let html = signature(&resolve, func, &link);
        assert!(html.contains(r##"<span class="b">borrow</span>&lt;<a class="t" href="#type-descriptor">descriptor</a>&gt;"##), "{html}");
        assert!(
            html.contains(r##"<a class="t" href="#type-datetime">datetime</a>"##),
            "{html}"
        );
        assert!(html.contains(r#"<span class="b">string</span>"#), "{html}");
    }
}
