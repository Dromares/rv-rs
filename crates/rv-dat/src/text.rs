//! Text decoding and the line/token loader of the text-format readers
//! (`DAT/DATReader/Utils/DatFileLoader.cs`, and the nested loader of
//! `DatROMCenterReader.cs:368-408`).

use crate::error::{Abort, R};

/// What .NET's `new StreamReader(stream, Encoding.UTF8, detectEncodingFromByteOrderMarks: true)`
/// does: honour a UTF-8, UTF-16 or UTF-32 byte order mark, otherwise read UTF-8, turning
/// invalid bytes into U+FFFD. The mark itself is dropped.
pub fn decode_text(bytes: &[u8]) -> String {
    match bom(bytes) {
        Some((enc, skip)) => decode_with(enc, &bytes[skip..]),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Enc {
    Utf8,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
}

/// The encoding named by a byte order mark and the mark's length.
pub(crate) fn bom(b: &[u8]) -> Option<(Enc, usize)> {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((Enc::Utf8, 3))
    } else if b.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        Some((Enc::Utf32Le, 4))
    } else if b.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        Some((Enc::Utf32Be, 4))
    } else if b.starts_with(&[0xFF, 0xFE]) {
        Some((Enc::Utf16Le, 2))
    } else if b.starts_with(&[0xFE, 0xFF]) {
        Some((Enc::Utf16Be, 2))
    } else {
        None
    }
}

pub(crate) fn decode_with(enc: Enc, b: &[u8]) -> String {
    match enc {
        Enc::Utf8 => String::from_utf8_lossy(b).into_owned(),
        Enc::Utf16Le | Enc::Utf16Be => {
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| {
                    if enc == Enc::Utf16Le {
                        u16::from_le_bytes(*p)
                    } else {
                        u16::from_be_bytes(*p)
                    }
                })
                .collect();
            String::from_utf16_lossy(&units)
        }
        Enc::Utf32Le | Enc::Utf32Be => b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let v = if enc == Enc::Utf32Le {
                    u32::from_le_bytes(*p)
                } else {
                    u32::from_be_bytes(*p)
                };
                char::from_u32(v).unwrap_or('\u{FFFD}')
            })
            .collect(),
    }
}

/// `StreamReader.ReadLine` splitting: `\n`, `\r` and `\r\n` end a line, and a final line
/// terminator does not start an empty line.
pub fn split_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    let mut pending = false;
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                lines.push(std::mem::take(&mut cur));
                pending = false;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines.push(std::mem::take(&mut cur));
                pending = false;
            }
            _ => {
                cur.push(c);
                pending = true;
            }
        }
    }
    if pending {
        lines.push(cur);
    }
    lines
}

/// ASCII-case-insensitive `str::find`, for the `OrdinalIgnoreCase` tests with ASCII needles.
pub(crate) fn find_ignore_ascii_case(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// `DatFileLoader` (`DatFileLoader.cs:6-115`): the tokenizer of ClrMamePro text and DOSCenter.
pub(crate) struct Loader {
    lines: Vec<String>,
    pos: usize,
    /// `_line`: the unread part of the current line is `line[off..]`. Tokens advance `off`
    /// instead of copying the rest of the line, so a line costs O(its length) in total and
    /// not per token.
    line: String,
    off: usize,
    /// `Next`
    pub next: String,
    /// `LineNumber`
    pub line_number: usize,
}

impl Loader {
    pub fn new(text: &str) -> Loader {
        Loader {
            lines: split_lines(text),
            pos: 0,
            line: String::new(),
            off: 0,
            next: String::new(),
            line_number: 0,
        }
    }

    /// `_line`, the unread part of the current line.
    fn rest(&self) -> &str {
        &self.line[self.off..]
    }

    /// `_line = _line.Trim(' ')` after `off` moved: skip leading spaces by moving `off`, and
    /// cut trailing ones.
    fn trim_spaces(&mut self) {
        let b = self.line.as_bytes();
        let mut s = self.off;
        while s < b.len() && b[s] == b' ' {
            s += 1;
        }
        let mut e = b.len();
        while e > s && b[e - 1] == b' ' {
            e -= 1;
        }
        self.line.truncate(e);
        self.off = s;
    }

    /// `EndOfStream()`: the underlying reader has no more lines (`:28-31`).
    pub fn end_of_stream(&self) -> bool {
        self.pos >= self.lines.len()
    }

    /// True when `Gn()` can only return `""` from now on: the buffer is empty and the file is
    /// finished. The C# loops `while (Next != ")")` spin forever there; callers use this to
    /// stop instead.
    pub fn stuck(&self) -> bool {
        self.rest().trim().is_empty() && self.end_of_stream()
    }

    /// `GnRest()`: the rest of the line with every `"` removed (`:33-39`).
    pub fn gn_rest(&mut self) -> String {
        let r = self.rest().replace('"', "");
        self.line.clear();
        self.off = 0;
        self.next = r.clone();
        r
    }

    /// `GnRestQ()`: the rest of the line unchanged (`:40-46`).
    pub fn gn_rest_q(&mut self) -> String {
        let r = self.rest().to_owned();
        self.line.clear();
        self.off = 0;
        self.next = r.clone();
        r
    }

    /// `GnNameToSize()` (`:48-55`): text up to ` size `, which stays in the buffer. With no
    /// ` size ` C# throws (`Substring(0, -1)`).
    pub fn gn_name_to_size(&mut self) -> R<String> {
        let Some(pos) = find_ignore_ascii_case(self.rest(), " size ") else {
            return Err(Abort(
                "Length cannot be less than zero (no ' size ' in line)".into(),
            ));
        };
        let r = self.rest()[..pos].to_owned();
        self.off += pos + 1;
        self.next = r.clone();
        Ok(r)
    }

    /// `Gn()` (`:57-113`).
    pub fn gn(&mut self) -> String {
        while self.rest().trim().is_empty() && !self.end_of_stream() {
            let raw = &self.lines[self.pos];
            self.pos += 1;
            self.line_number += 1;
            let mut l = raw.replace('\t', " ");
            // `//` ends the line even inside quotes (`:67-71`).
            if let Some(i) = l.find("//") {
                l.truncate(i);
            }
            // `#` and `;` comment lines, only if longer than one character (`:79-86`).
            let ts = l.trim_start();
            if ts.chars().nth(1).is_some() && (ts.starts_with('#') || ts.starts_with(';')) {
                l.clear();
            }
            self.line = format!("{} ", l.trim());
            self.off = 0;
        }

        let ret;
        if !self.rest().trim().is_empty() {
            let rest = self.rest();
            let used;
            if let Some(inner) = rest.strip_prefix('"') {
                // The closing quote is searched after the opening one; unterminated runs to
                // the end.
                let end = inner.find('"').unwrap_or(inner.len());
                ret = inner[..end].to_owned();
                used = (end + 2).min(rest.len());
            } else {
                let sp = rest.find(' ').unwrap_or(rest.len());
                ret = rest[..sp].to_owned();
                used = sp;
            }
            self.off += used;
            self.trim_spaces();
        } else {
            ret = String::new();
        }
        self.next = ret.clone();
        ret
    }
}

/// The ROMCenter loader (`DatROMCenterReader.cs:368-408`): one non-blank line at a time.
pub(crate) struct LineLoader {
    lines: Vec<String>,
    pos: usize,
    pub next: String,
    pub line_number: usize,
}

impl LineLoader {
    pub fn new(text: &str) -> LineLoader {
        LineLoader {
            lines: split_lines(text),
            pos: 0,
            next: String::new(),
            line_number: 0,
        }
    }

    pub fn end_of_stream(&self) -> bool {
        self.pos >= self.lines.len()
    }

    fn read_line(&mut self) -> Option<String> {
        let l = self.lines.get(self.pos).cloned();
        if l.is_some() {
            self.pos += 1;
        }
        l
    }

    /// `Gn()` (`:394-406`): reads a line, skips blank ones while there is more to read. A
    /// `null` line (read past the end) is a `NullReferenceException` in C#.
    pub fn gn(&mut self) -> R<String> {
        let mut line = self
            .read_line()
            .ok_or_else(|| Abort("Object reference not set to an instance of an object".into()))?;
        self.line_number += 1;
        while line.trim().is_empty() && !self.end_of_stream() {
            line = self.read_line().unwrap_or_default();
            self.line_number += 1;
        }
        self.next = line.clone();
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_like_streamreader() {
        assert_eq!(split_lines(""), Vec::<String>::new());
        assert_eq!(split_lines("a"), ["a"]);
        assert_eq!(split_lines("a\n"), ["a"]);
        assert_eq!(split_lines("a\n\n"), ["a", ""]);
        assert_eq!(split_lines("a\r\nb\rc\nd"), ["a", "b", "c", "d"]);
        assert_eq!(split_lines("\n"), [""]);
    }

    #[test]
    fn bom_decoding() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFab"), "ab");
        assert_eq!(decode_text(b"\xFF\xFEa\x00b\x00"), "ab");
        assert_eq!(decode_text(b"\xFE\xFF\x00a\x00b"), "ab");
        assert_eq!(decode_text(b"\xFF\xFE\x00\x00a\x00\x00\x00"), "a");
        assert_eq!(decode_text(b"a\xAC"), "a\u{FFFD}");
    }

    #[test]
    fn tokens() {
        let mut l = Loader::new("game ( name \"A B\" // c\n  rom ( name x ) )\n# note\n");
        let mut got = vec![];
        loop {
            let t = l.gn();
            if t.is_empty() && l.stuck() {
                break;
            }
            got.push(t);
        }
        assert_eq!(
            got,
            ["game", "(", "name", "A B", "rom", "(", "name", "x", ")", ")"]
        );
    }

    #[test]
    fn rest_of_line_strips_quotes() {
        let mut l = Loader::new("name \"A B\" c\n");
        assert_eq!(l.gn(), "name");
        assert_eq!(l.gn_rest(), "A B c");
    }

    #[test]
    fn comment_cuts_inside_quotes() {
        // `DatFileLoader.cs:67-71`: URLs in headers lose everything after `//`.
        let mut l = Loader::new("url \"http://x.org\"\n");
        assert_eq!(l.gn(), "url");
        assert_eq!(l.gn_rest(), "http:");
    }

    #[test]
    fn name_to_size() {
        let mut l = Loader::new("name My File.bin size 12 crc 1\n");
        assert_eq!(l.gn(), "name");
        assert_eq!(l.gn_name_to_size().unwrap(), "My File.bin");
        assert_eq!(l.gn(), "size");
        assert_eq!(l.gn(), "12");
        let mut bad = Loader::new("name noSizeHere\n");
        bad.gn();
        assert!(bad.gn_name_to_size().is_err());
    }

    /// A one-line file of N tokens must tokenize in O(N). The loader used to re-slice the
    /// rest of the line per token: 160k tokens took 0.56 s and every doubling cost 4x.
    #[test]
    fn long_single_line_is_linear() {
        let text = "( ".repeat(1_000_000);
        let start = std::time::Instant::now();
        let mut l = Loader::new(&text);
        let mut n = 0usize;
        while !l.gn().is_empty() {
            n += 1;
        }
        assert_eq!(n, 1_000_000);
        // Linear: ~0.1 s in a debug build. The old loader needed ~35 s here (release).
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "tokenizing took {:?}",
            start.elapsed()
        );
    }
}
