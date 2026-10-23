//! JSX spellings: an inline `<svg>` from the SVG writer rewritten as JSX, text as
//! JSX children, and the names a CSS module and an asset import are read through.

/// The SVG writer's markup as JSX: attribute names camel-cased (`stroke-width`
/// → `strokeWidth`, `xlink:href` → `xlinkHref`), `class` read from the module,
/// `style` as an object, and text as string expressions, since JSX would read
/// `{` and collapse the whitespace the SVG keeps.
///
/// The markup is the SVG writer's own — tags, attributes in double quotes, text,
/// no comments or CDATA — which is all this reads.
pub(crate) fn from_svg(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len() + svg.len() / 8);
    let mut rest = svg;
    while let Some(lt) = rest.find('<') {
        text(&mut out, &rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = tag_end(rest) else {
            // Not a tag the writer makes; keep it as text rather than lose it.
            text(&mut out, rest);
            return out;
        };
        tag(&mut out, &rest[..=gt]);
        rest = &rest[gt + 1..];
    }
    text(&mut out, rest);
    out
}

/// The index of the `>` closing the tag `s` starts with, outside quotes.
fn tag_end(s: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '>' if !quoted => return Some(i),
            _ => {}
        }
    }
    None
}

/// One tag, `<…>` inclusive, with its attributes respelled.
fn tag(out: &mut String, t: &str) {
    if t.starts_with("</") {
        out.push_str(t);
        return;
    }
    let self_closing = t.ends_with("/>");
    let inner = &t[1..t.len() - if self_closing { 2 } else { 1 }];
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    out.push('<');
    out.push_str(&inner[..name_end]);
    let mut attrs = &inner[name_end..];
    loop {
        attrs = attrs.trim_start();
        let Some(eq) = attrs.find("=\"") else { break };
        let name = &attrs[..eq];
        let after = &attrs[eq + 2..];
        let Some(close) = after.find('"') else { break };
        let value = &after[..close];
        attrs = &after[close + 1..];
        out.push(' ');
        match name {
            "class" => {
                out.push_str("className=");
                out.push_str(&module_class(&decode(value)));
            }
            "style" => {
                out.push_str("style={{ ");
                let decls: Vec<String> = decode(value)
                    .split(';')
                    .filter_map(|d| {
                        let (p, v) = d.split_once(':')?;
                        Some(format!("{}: {}", camel(p.trim()), js_string(v.trim())))
                    })
                    .collect();
                out.push_str(&decls.join(", "));
                out.push_str(" }}");
            }
            _ => {
                out.push_str(&attr_name(name));
                out.push_str("=\"");
                out.push_str(value);
                out.push('"');
            }
        }
    }
    out.push_str(if self_closing { " />" } else { ">" });
}

/// Text between tags: indentation is left for JSX to drop, anything else is a
/// string expression so it reaches the DOM exactly.
fn text(out: &mut String, t: &str) {
    if t.is_empty() {
        return;
    }
    if t.trim().is_empty() && t.contains('\n') {
        out.push_str(t);
        return;
    }
    out.push('{');
    out.push_str(&js_string(&decode(t)));
    out.push('}');
}

/// An attribute's JSX name: `data-` and `aria-` stay as written, a namespace or a
/// hyphen camel-cases.
fn attr_name(name: &str) -> String {
    if name.starts_with("data-") || name.starts_with("aria-") {
        return name.to_string();
    }
    camel(name)
}

/// `stroke-width` → `strokeWidth`, `xlink:href` → `xlinkHref`.
pub(crate) fn camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut up = false;
    for c in name.chars() {
        if c == '-' || c == ':' {
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A JSX child for text: as written when JSX would read it back unchanged,
/// else a string expression.
pub(crate) fn child_text(t: &str) -> String {
    let plain = !t.is_empty()
        && t.trim() == t
        && !t.contains(['{', '}', '<', '>', '&', '\n', '\r', '\t'])
        && !t.contains("  ");
    if plain {
        t.to_string()
    } else {
        format!("{{{}}}", js_string(t))
    }
}

/// How the module's class `class` is read: `{styles.card}`, or
/// `{styles["card-2"]}` for a name that is not an identifier.
pub(crate) fn module_class(class: &str) -> String {
    let ident = class.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && class.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident {
        format!("{{styles.{class}}}")
    } else {
        format!("{{styles[{}]}}", js_string(class))
    }
}

/// A double-quoted JavaScript string literal.
pub(crate) fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A single-quoted JavaScript string, for an expression inside a double-quoted
/// attribute (which `dom::escape` then makes safe).
pub(crate) fn single_quoted(s: &str) -> String {
    let inner = js_string(s);
    let inner = &inner[1..inner.len() - 1];
    format!("'{}'", inner.replace("\\\"", "\"").replace('\'', "\\'"))
}

/// The SVG writer's escapes undone (`escape_xml`'s five).
fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The binding a swap property `prop` is destructured into, which JSX can use
/// as a tag: `icon` → `IconSlot`. The walk keeps it off every component's name.
pub(crate) fn slot_binding(prop: &str) -> String {
    let mut c = prop.chars();
    match c.next() {
        Some(f) => format!("{}{}Slot", f.to_ascii_uppercase(), c.as_str()),
        None => "Slot".into(),
    }
}

/// The binding an asset is imported as: `assets/card-box.png` → `card_box_png`.
/// Snake case, with the extension always a word of its own, so it never meets a
/// prop or a component, whose names have no `_`.
pub(crate) fn asset_binding(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let mut out = String::new();
    let mut gap = false;
    for c in file.chars() {
        if c.is_ascii_alphanumeric() {
            // A leading digit, or a word after the first.
            if (out.is_empty() && c.is_ascii_digit()) || (gap && !out.is_empty()) {
                out.push('_');
            }
            out.push(c);
            gap = false;
        } else {
            gap = true;
        }
    }
    if !out.contains('_') {
        out.insert_str(0, "asset_");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Attribute names, the class, the style and text all come out as JSX reads them.
    #[test]
    fn svg_markup_is_respelled_for_jsx() {
        let svg = "<svg class=\"logo-2\" xmlns=\"http://www.w3.org/2000/svg\" width=\"10\">\n  \
                   <mask id=\"m\" style=\"mask-type:alpha\">\n  \
                   <path stroke-width=\"2\" xlink:href=\"#a\" data-x=\"1\"/>\n  \
                   <text xml:space=\"preserve\">a {b} &amp; &lt;c&gt;</text>\n</svg>";
        let jsx = from_svg(svg);
        assert!(
            jsx.starts_with("<svg className={styles[\"logo-2\"]} xmlns="),
            "{jsx}"
        );
        assert!(jsx.contains("style={{ maskType: \"alpha\" }}"), "{jsx}");
        assert!(
            jsx.contains("<path strokeWidth=\"2\" xlinkHref=\"#a\" data-x=\"1\" />"),
            "{jsx}"
        );
        assert!(
            jsx.contains("<text xmlSpace=\"preserve\">{\"a {b} & <c>\"}</text>"),
            "{jsx}"
        );
        assert!(jsx.ends_with("</svg>"), "{jsx}");
    }

    /// Only text JSX would read back unchanged goes in bare.
    #[test]
    fn text_that_jsx_would_alter_is_a_string_expression() {
        assert_eq!(child_text("Sign in"), "Sign in");
        assert_eq!(child_text(" lead"), "{\" lead\"}");
        assert_eq!(child_text("a  b"), "{\"a  b\"}");
        assert_eq!(child_text("{x}"), "{\"{x}\"}");
        assert_eq!(child_text("Tom & Jerry"), "{\"Tom & Jerry\"}");
    }

    #[test]
    fn asset_bindings_are_identifiers() {
        assert_eq!(asset_binding("assets/card-box.png"), "card_box_png");
        assert_eq!(asset_binding("assets/2up.jpg"), "_2up_jpg");
        assert_eq!(asset_binding("assets/logo"), "asset_logo");
        assert_eq!(module_class("card"), "{styles.card}");
        assert_eq!(module_class("card-2"), "{styles[\"card-2\"]}");
    }
}
