//! A small DOM that behaves like the parts of `System.Xml.XmlDocument` the readers use:
//! child selection by name (`SelectNodes("game")`), `InnerText`, attributes by name.
//!
//! Reader settings in ROMVault: `XmlResolver = null`, `CheckCharacters = false`,
//! `DtdProcessing.Ignore` (`DAT/DATReader/DatReader.cs:112-123`). So: a DOCTYPE is skipped,
//! entities declared in it are not expanded, and character references to control characters
//! are accepted.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::text::{bom, decode_with, Enc};

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Element(Element),
    /// Text and CDATA.
    Text(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Element {
    /// The qualified name as written.
    pub name: String,
    /// True when the element is in a namespace (a non-empty default `xmlns`, or a prefix).
    /// `SelectNodes("game")` only matches elements with no namespace.
    pub in_namespace: bool,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    /// `Attributes.GetNamedItem(name)`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// `VarFix.String(Attributes.GetNamedItem(name))`.
    pub fn attr_str(&self, name: &str) -> &str {
        self.attr(name).unwrap_or("")
    }

    /// `SelectNodes(name)`: child elements with this name.
    pub fn select<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter_map(move |n| match n {
            Node::Element(e) if e.name == name && !e.in_namespace => Some(e),
            _ => None,
        })
    }

    /// `SelectSingleNode(name)`.
    pub fn select_one<'a>(&'a self, name: &'a str) -> Option<&'a Element> {
        self.select(name).next()
    }

    /// `InnerText`: the text of all descendant text and CDATA nodes.
    pub fn inner_text(&self) -> String {
        let mut s = String::new();
        self.collect_text(&mut s);
        s
    }

    fn collect_text(&self, out: &mut String) {
        for n in &self.children {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => e.collect_text(out),
            }
        }
    }

    /// `VarFix.String(SelectSingleNode(name))`.
    pub fn child_text(&self, name: &str) -> String {
        self.select_one(name)
            .map(Element::inner_text)
            .unwrap_or_default()
    }
}

/// The document element.
pub(crate) struct Document {
    pub root: Element,
}

/// Decode the bytes of an XML file to text: a byte order mark decides, else the `encoding`
/// of the XML declaration, else UTF-8 with invalid bytes replaced.
fn decode_xml(bytes: &[u8]) -> String {
    if let Some((enc, skip)) = bom(bytes) {
        return decode_with(enc, &bytes[skip..]);
    }
    let head = &bytes[..bytes.len().min(256)];
    if head.starts_with(b"<?xml") {
        let end = head.iter().position(|&b| b == b'>').unwrap_or(head.len());
        let decl = String::from_utf8_lossy(&head[..end]).to_lowercase();
        if let Some(i) = decl.find("encoding") {
            let rest = &decl[i + "encoding".len()..];
            let rest = rest
                .trim_start()
                .strip_prefix('=')
                .unwrap_or("")
                .trim_start();
            if let Some(q) = rest.chars().next().filter(|q| *q == '"' || *q == '\'') {
                if let Some(label) = rest[1..].split(q).next() {
                    if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                        if enc != encoding_rs::UTF_8 {
                            return enc.decode_without_bom_handling(bytes).0.into_owned();
                        }
                    }
                }
            }
        }
    }
    decode_with(Enc::Utf8, bytes)
}

/// XML 1.0 end-of-line handling (`\r\n` and lone `\r` become `\n`).
fn normalize_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_owned();
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Whether a text node is dropped: `XmlDocument.PreserveWhitespace` is false, so nodes of
/// white space only are not kept.
fn is_ws_only(t: &str) -> bool {
    t.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
}

/// Deepest element nesting accepted; `XmlReader` has no limit, but a million nested elements
/// would overflow the stack when the tree is dropped. Real DATs nest a few levels.
const MAX_DEPTH: usize = 512;

fn xml_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Parse a document. Errors are the text the C# reports as
/// `Error Occured Reading Dat:\r\n<message>`.
pub(crate) fn parse(bytes: &[u8]) -> Result<Document, String> {
    let text = normalize_newlines(&decode_xml(bytes));
    let mut reader = Reader::from_str(&text);
    reader.config_mut().check_end_names = true;
    let mut stack: Vec<Element> = Vec::new();
    // Whether a non-empty default namespace is in force, per open element (plus the outside).
    let mut default_ns: Vec<bool> = vec![false];
    let mut root: Option<Element> = None;

    loop {
        let ev = reader.read_event().map_err(xml_err)?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if root.is_some() {
                    return Err("There are multiple root elements.".into());
                }
                let mut el = Element {
                    name: String::from_utf8_lossy(e.name().as_ref()).into_owned(),
                    ..Element::default()
                };
                let mut ns = *default_ns.last().unwrap_or(&false);
                for a in e.attributes() {
                    let a = a.map_err(xml_err)?;
                    let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                    // Literal tab and line feed in an attribute value become spaces, before
                    // references are expanded.
                    let raw = String::from_utf8_lossy(&a.value).replace(['\n', '\t'], " ");
                    let value = quick_xml::escape::unescape(&raw)
                        .map_err(xml_err)?
                        .into_owned();
                    if key == "xmlns" {
                        ns = !value.is_empty();
                    }
                    el.attrs.push((key, value));
                }
                el.in_namespace = ns || el.name.contains(':');
                if matches!(ev, Event::Start(_)) {
                    if stack.len() >= MAX_DEPTH {
                        return Err(format!("Elements are nested more than {MAX_DEPTH} deep."));
                    }
                    stack.push(el);
                    default_ns.push(ns);
                } else if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Element(el));
                } else {
                    root = Some(el);
                }
            }
            Event::End(_) => {
                default_ns.pop();
                let el = stack.pop().ok_or("Unexpected end tag.")?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Element(el));
                } else {
                    root = Some(el);
                }
            }
            Event::Text(ref t) => {
                let raw = String::from_utf8_lossy(t.as_ref()).into_owned();
                let s = quick_xml::escape::unescape(&raw)
                    .map_err(xml_err)?
                    .into_owned();
                match stack.last_mut() {
                    Some(parent) => {
                        if !is_ws_only(&raw) {
                            parent.children.push(Node::Text(s));
                        }
                    }
                    None => {
                        if !is_ws_only(&raw) {
                            return Err("Data at the root level is invalid.".into());
                        }
                    }
                }
            }
            Event::CData(ref t) => {
                let s = String::from_utf8_lossy(t.as_ref()).into_owned();
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Text(s)),
                    None => return Err("Data at the root level is invalid.".into()),
                }
            }
            Event::Eof => break,
            // Declaration, comments, processing instructions and the DOCTYPE (ignored,
            // `DtdProcessing.Ignore`) carry nothing the readers use.
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(format!(
            "Unexpected end of file has occurred. The following elements are not closed: {}.",
            stack
                .iter()
                .rev()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    match root {
        Some(root) => Ok(Document { root }),
        None => Err("Root element is missing.".into()),
    }
}
