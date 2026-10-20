//! A small element tree for SOAP replies. Prefixes are dropped (only local names matter to the
//! contract), a DOCTYPE is refused, and nesting is bounded.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

const MAX_DEPTH: usize = 64;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Node {
    /// The local name.
    pub name: String,
    /// `xsi:nil="true"`.
    pub nil: bool,
    /// The text directly inside this element, entities resolved.
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    /// The first direct child with this local name.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The first element with this local name, depth first, this one included.
    pub fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
}

fn open(e: &BytesStart<'_>) -> Result<Node, String> {
    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
    let mut nil = false;
    for a in e.attributes() {
        let a = a.map_err(|e| e.to_string())?;
        if a.key.local_name().as_ref() == b"nil" {
            let v = a.unescape_value().map_err(|e| e.to_string())?;
            nil = v == "true" || v == "1";
        }
    }
    Ok(Node {
        name,
        nil,
        ..Node::default()
    })
}

pub fn parse(bytes: &[u8]) -> Result<Node, String> {
    let mut reader = Reader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("XML: {e}"))?;
        match ev {
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err("XML nested too deeply".to_owned());
                }
                stack.push(open(&e)?);
            }
            Event::Empty(e) => {
                let n = open(&e)?;
                match stack.last_mut() {
                    Some(p) => p.children.push(n),
                    None => root = Some(n),
                }
            }
            Event::End(_) => {
                let n = stack.pop().ok_or("XML: unbalanced end tag")?;
                match stack.last_mut() {
                    Some(p) => p.children.push(n),
                    None => root = Some(n),
                }
            }
            Event::Text(t) => {
                if let Some(n) = stack.last_mut() {
                    n.text
                        .push_str(&t.unescape().map_err(|e| format!("XML: {e}"))?);
                }
            }
            Event::CData(t) => {
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(&String::from_utf8_lossy(&t.into_inner()));
                }
            }
            Event::DocType(_) => return Err("XML: a DOCTYPE is not accepted".to_owned()),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !stack.is_empty() {
        return Err("XML: unexpected end of document".to_owned());
    }
    root.ok_or_else(|| "XML: empty document".to_owned())
}

/// `text` as element content or an attribute value. Characters XML 1.0 cannot carry are an
/// error, not silently dropped: a key with a control character in it must not be sent altered.
pub fn escape(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {
                return Err(format!("character U+{:04X} cannot be sent", c as u32))
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_nil_and_entities() {
        let n = parse(
            br#"<s:Envelope xmlns:s="x"><s:Body><R xmlns="t"><a:v xmlns:a="u" xmlns:i="i" i:nil="true"/><t>a &amp; b</t></R></s:Body></s:Envelope>"#,
        )
        .unwrap();
        assert_eq!(n.name, "Envelope");
        let r = n.find("R").unwrap();
        assert!(r.child("v").unwrap().nil);
        assert_eq!(r.child("t").unwrap().text, "a & b");
        assert!(r.child("zzz").is_none());
    }

    #[test]
    fn a_doctype_is_refused() {
        assert!(
            parse(b"<!DOCTYPE a [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><a>&x;</a>").is_err()
        );
    }

    #[test]
    fn broken_and_deep_documents_are_errors() {
        assert!(parse(b"<a><b></a>").is_err());
        assert!(parse(b"<a>").is_err());
        assert!(parse(b"").is_err());
        let deep = "<a>".repeat(200) + &"</a>".repeat(200);
        assert!(parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn escape_refuses_what_xml_cannot_carry() {
        assert_eq!(escape("a<b&\"c'>").unwrap(), "a&lt;b&amp;&quot;c&apos;&gt;");
        assert!(escape("bad\u{1}").is_err());
        assert_eq!(escape("tab\there").unwrap(), "tab\there");
    }
}
