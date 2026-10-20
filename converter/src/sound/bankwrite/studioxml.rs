//! FMOD Studio project metadata XML (`metadata\**\*.xml`): a list of
//!
//! ```xml
//! <object class="Event" id="{guid}">
//!     <property name="name"><value>Engine</value></property>
//!     <relationship name="folder"><destination>{guid}</destination></relationship>
//! </object>
//! ```
//!
//! An object may also be self-closing (`<object ... />`), a property may hold
//! several `<value>`s and a relationship several `<destination>`s. Values come
//! back as written (entities not decoded; `prj` unescapes them).

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct XmlObj {
    pub class: String,
    pub id: String,
    pub props: HashMap<String, Vec<String>>,
    pub rels: HashMap<String, Vec<String>>,
}

/// The value of attribute `name` in a tag's text (`<object a="..." b="...">`).
fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(p) = rest.find(name) {
        let before = rest[..p].chars().last();
        let after = &rest[p + name.len()..];
        let after_t = after.trim_start();
        if before.map(|c| c.is_whitespace()).unwrap_or(false) && after_t.starts_with('=') {
            let v = after_t[1..].trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let end = v[1..].find(q)?;
                return Some(v[1..1 + end].to_string());
            }
        }
        rest = &rest[p + name.len()..];
    }
    None
}

/// The text of every `<inner>...</inner>` (or `""` for `<inner/>`) in `body`.
fn texts(body: &str, inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let open = format!("<{inner}");
    let close = format!("</{inner}>");
    let mut i = 0;
    while let Some(p) = body[i..].find(&open) {
        let s = i + p;
        let Some(gt) = body[s..].find('>') else { break };
        let tag_end = s + gt;
        if body[..tag_end].ends_with('/') {
            out.push(String::new());
            i = tag_end + 1;
            continue;
        }
        let Some(c) = body[tag_end + 1..].find(&close) else { break };
        out.push(body[tag_end + 1..tag_end + 1 + c].to_string());
        i = tag_end + 1 + c + close.len();
    }
    out
}

/// `<section name="...">body</section>` blocks of one kind in an object body.
fn named_blocks(body: &str, section: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let open = format!("<{section}");
    let close = format!("</{section}>");
    let mut i = 0;
    while let Some(p) = body[i..].find(&open) {
        let s = i + p;
        let Some(gt) = body[s..].find('>') else { break };
        let tag = &body[s..s + gt];
        let name = attr(tag, "name").unwrap_or_default();
        if tag.ends_with('/') {
            out.push((name, String::new()));
            i = s + gt + 1;
            continue;
        }
        let Some(c) = body[s + gt + 1..].find(&close) else { break };
        out.push((name, body[s + gt + 1..s + gt + 1 + c].to_string()));
        i = s + gt + 1 + c + close.len();
    }
    out
}

/// Every `<object>` of one metadata file.
pub fn parse(text: &str) -> Vec<XmlObj> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = text[i..].find("<object ") {
        let s = i + p;
        let Some(gt) = text[s..].find('>') else { break };
        let tag = &text[s..s + gt];
        let mut o = XmlObj { class: attr(tag, "class").unwrap_or_default(), id: attr(tag, "id").unwrap_or_default(), ..Default::default() };
        if tag.ends_with('/') {
            out.push(o);
            i = s + gt + 1;
            continue;
        }
        let body_start = s + gt + 1;
        let end = text[body_start..].find("</object>").map(|e| body_start + e).unwrap_or(text.len());
        let body = &text[body_start..end];
        for (name, b) in named_blocks(body, "property") {
            o.props.entry(name).or_default().extend(texts(&b, "value"));
        }
        for (name, b) in named_blocks(body, "relationship") {
            o.rels.entry(name).or_default().extend(texts(&b, "destination"));
        }
        out.push(o);
        i = end + "</object>".len().min(text.len() - end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects() {
        let t = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<objects serializationModel=\"Studio.02.03.00\">\n\t<object class=\"Bus\" id=\"{a}\" />\n\t<object class=\"Event\" id=\"{b}\">\n\t\t<property name=\"name\">\n\t\t\t<value>Eng &amp; ine</value>\n\t\t</property>\n\t\t<property name=\"list\">\n\t\t\t<value>1</value>\n\t\t\t<value>2</value>\n\t\t</property>\n\t\t<relationship name=\"folder\">\n\t\t\t<destination>{c}</destination>\n\t\t</relationship>\n\t</object>\n</objects>\n";
        let v = parse(t);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].class, "Bus");
        assert_eq!(v[1].id, "{b}");
        assert_eq!(v[1].props["name"], vec!["Eng &amp; ine".to_string()]);
        assert_eq!(v[1].props["list"].len(), 2);
        assert_eq!(v[1].rels["folder"], vec!["{c}".to_string()]);
    }
}
