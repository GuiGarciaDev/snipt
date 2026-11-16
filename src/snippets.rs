//! Snippet storage: a plain-text file you can edit by hand, plus fuzzy search.
//!
//! File format (`%APPDATA%\Snipt\snippets.txt`):
//!
//! ```text
//! # Lines starting with '#' before the first entry are comments.
//! @ GitHub profile [url]
//! https://github.com/you
//!
//! @ Email sign-off
//! Best regards,
//! {cursor}
//! ```
//!
//! Every line starting with `@ ` begins a new snippet. An optional `[url]`,
//! `[text]` or `[template]` tag at the end of the name sets the type; without
//! it the type is detected from the content. Adding `pin` (`[pin]`,
//! `[url, pin]`) keeps the snippet at the top of the list. Start a body line with `\@` to
//! write a literal `@` at the beginning of a line.

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Url,
    Template,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Url, Kind::Template, Kind::Text];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Text => "TEXT",
            Kind::Url => "URL",
            Kind::Template => "TEMPLATE",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Url => "url",
            Kind::Template => "template",
        }
    }
}

impl Snippet {
    pub fn new(name: &str, body: &str, kind: Kind) -> Snippet {
        let preview = body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect::<String>();
        Snippet {
            name: name.to_string(),
            body: body.to_string(),
            kind,
            pinned: false,
            preview,
            name_lower: name.to_lowercase(),
            body_lower: body.to_lowercase(),
        }
    }
}

/// Checks a URL snippet: one line, no spaces, http(s):// or www. plus a host.
pub fn valid_url(s: &str) -> bool {
    let s = s.trim();
    let lower = s.to_lowercase();
    let rest = ["https://", "http://", "www."]
        .iter()
        .find_map(|p| lower.strip_prefix(p));
    match rest {
        Some(rest) if !s.chars().any(char::is_whitespace) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = host.split(':').next().unwrap_or("");
            host == "localhost" || (host.contains('.') && !host.starts_with('.') && !host.ends_with('.'))
        }
        _ => false,
    }
}

/// Writes all snippets back to the file, keeping the comment header on top.
pub fn save(all: &[Snippet]) -> std::io::Result<()> {
    let path = file_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let existing = existing.trim_start_matches('\u{feff}');
    let header: Vec<&str> = existing.lines().take_while(|l| !l.starts_with("@ ")).collect();
    let out = serialize(&header.join("\n"), all);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write-then-rename so a crash never leaves a half-written file.
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, &path)
}

fn serialize(header: &str, all: &[Snippet]) -> String {
    let header = header.trim_end();
    let mut out = String::new();
    out.push_str(if header.is_empty() { SAMPLE_HEADER.trim_end() } else { header });
    out.push('\n');
    for s in all {
        out.push_str("\n@ ");
        out.push_str(&s.name);
        let mut tags = Vec::new();
        if detect(&s.body) != s.kind {
            tags.push(s.kind.tag());
        }
        if s.pinned {
            tags.push("pin");
        }
        if !tags.is_empty() {
            out.push_str(&format!(" [{}]", tags.join(", ")));
        }
        out.push('\n');
        for line in s.body.lines() {
            if line.starts_with("@ ") {
                out.push('\\');
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Snippet {
    pub name: String,
    pub body: String,
    pub kind: Kind,
    /// Always listed first.
    pub pinned: bool,
    /// Single-line preview shown under the name.
    pub preview: String,
    name_lower: String,
    body_lower: String,
}

const SAMPLE_HEADER: &str = "\
# Snipt snippets file. Edit it here or from Snipt (gear icon / Ctrl+E).
# Each entry starts with a line '@ Name' and an optional [url] / [text] / [template] tag.
# Add 'pin' to keep a snippet at the top, e.g. [pin] or [url, pin].
# Template variables: {date} {time} {datetime} {clipboard} {cursor}
";

const SAMPLE: &str = concat!(
    "\
# Snipt snippets file. Edit it here or from Snipt (gear icon / Ctrl+E).
# Each entry starts with a line '@ Name' and an optional [url] / [text] / [template] tag.
# Add 'pin' to keep a snippet at the top, e.g. [pin] or [url, pin].
# Template variables: {date} {time} {datetime} {clipboard} {cursor}
",
    "
@ Snipt on GitHub [url]
https://github.com/

@ Email sign-off [pin]
Best regards,
{cursor}

@ Meeting follow-up [template]
Hi {cursor},

Thanks for your time today ({date}). As discussed, here are the next steps:

-
-

Talk soon!

@ Today's date
{date}

@ Wrap clipboard in a code block
```
{clipboard}
```
"
);

pub fn file_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Snipt").join("snippets.txt")
}

/// Loads the snippets file, creating it with examples on first run.
pub fn load() -> Vec<Snippet> {
    let path = file_path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, SAMPLE);
    }
    match std::fs::read(&path) {
        Ok(bytes) => {
            // Tolerate a UTF-8 BOM (Notepad may add one).
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            parse(&String::from_utf8_lossy(bytes))
        }
        Err(_) => Vec::new(),
    }
}

pub fn parse(text: &str) -> Vec<Snippet> {
    let mut out = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;

    for line in text.lines() {
        if let Some(header) = line.strip_prefix("@ ") {
            if let Some((name, body)) = current.take() {
                push(&mut out, name, &body);
            }
            current = Some((header.trim().to_string(), Vec::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line.strip_prefix('\\').filter(|l| l.starts_with('@')).unwrap_or(line));
        }
        // Lines before the first entry are comments.
    }
    if let Some((name, body)) = current.take() {
        push(&mut out, name, &body);
    }
    out
}

fn push(out: &mut Vec<Snippet>, header: String, lines: &[&str]) {
    let (name, tag, pinned) = split_tag(&header);
    if name.is_empty() {
        return;
    }
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(start, |i| i + 1);
    let body = lines[start..end.max(start)].join("\n");

    let kind = tag.unwrap_or_else(|| detect(&body));
    let mut snippet = Snippet::new(&name, &body, kind);
    snippet.pinned = pinned;
    out.push(snippet);
}

/// Splits `Name [url, pin]` into the name, an optional type and the pin flag.
/// A bracket with anything unknown inside is treated as part of the name.
fn split_tag(header: &str) -> (String, Option<Kind>, bool) {
    if header.ends_with(']') {
        if let Some(open) = header.rfind('[') {
            let (mut kind, mut pin, mut known) = (None, false, true);
            for t in header[open + 1..header.len() - 1].split(',') {
                match t.trim().to_lowercase().as_str() {
                    "url" | "link" => kind = Some(Kind::Url),
                    "text" | "txt" => kind = Some(Kind::Text),
                    "template" | "tpl" => kind = Some(Kind::Template),
                    "pin" | "pinned" => pin = true,
                    _ => known = false,
                }
            }
            if known {
                return (header[..open].trim().to_string(), kind, pin);
            }
        }
    }
    (header.trim().to_string(), None, false)
}

pub fn detect(body: &str) -> Kind {
    let one_line = !body.contains('\n');
    let lower = body.trim().to_lowercase();
    if one_line && (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.")) {
        Kind::Url
    } else if ["{date}", "{time}", "{datetime}", "{clipboard}", "{cursor}"]
        .iter()
        .any(|v| body.contains(v))
    {
        Kind::Template
    } else {
        Kind::Text
    }
}

/// Returns indices into `all` in display order. Every space-separated word of
/// the query must match the name (fuzzy) or appear in the body (substring).
///
/// Order: pinned snippets first, then the best match, then `usage` (how much
/// you have used a snippet lately). With an empty query that means pinned,
/// then most used, then the rest in file order.
pub fn search(all: &[Snippet], query: &str, usage: impl Fn(&Snippet) -> f64) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let mut scored: Vec<(bool, i32, f64, usize)> = all
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let mut total = 0;
            for w in &words {
                let name_score = fuzzy(&s.name_lower, w);
                let body_score = s.body_lower.contains(w.as_str()).then_some(w.chars().count() as i32 * 3);
                total += name_score.max(body_score)?;
            }
            Some((s.pinned, total, usage(s), i))
        })
        .collect();
    // Stable sort keeps file order for ties.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    scored.into_iter().map(|(.., i)| i).collect()
}

/// Subsequence match with bonuses for consecutive chars, word starts and prefix.
fn fuzzy(hay: &str, needle: &str) -> Option<i32> {
    let hay: Vec<char> = hay.chars().collect();
    let mut score = 0;
    let mut hi = 0;
    let mut prev: Option<usize> = None;
    for nc in needle.chars() {
        let pos = (hi..hay.len()).find(|&j| hay[j] == nc)?;
        score += 10;
        if prev == Some(pos.wrapping_sub(1)) {
            score += 15;
        }
        if pos == 0 || !hay[pos - 1].is_alphanumeric() {
            score += 20;
        }
        prev = Some(pos);
        hi = pos + 1;
    }
    if hay.iter().collect::<String>().starts_with(needle) {
        score += 50;
    }
    // Shorter names win ties.
    Some(score - hay.len() as i32 / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_tags_and_detection() {
        let s = parse("# comment\n@ Site [url]\nhttps://a.b\n\n@ Sig\nBest,\n{cursor}\n\n@ Plain\nhello\n\\@ not a header\n");
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].name.as_str(), s[0].kind), ("Site", Kind::Url));
        assert_eq!(s[1].kind, Kind::Template);
        assert_eq!(s[1].body, "Best,\n{cursor}");
        assert_eq!(s[2].kind, Kind::Text);
        assert_eq!(s[2].body, "hello\n@ not a header");
    }

    #[test]
    fn validates_urls() {
        for ok in ["https://github.com", "http://a.b/c?d#e", "www.example.com", "http://localhost:3000/x"] {
            assert!(valid_url(ok), "{ok}");
        }
        for bad in ["github.com", "https://", "https://nodot", "https://a b.com", "ftp://x.y", "https://.com"] {
            assert!(!valid_url(bad), "{bad}");
        }
    }

    #[test]
    fn serialized_entries_parse_back() {
        let original = vec![
            Snippet::new("Site", "https://a.b", Kind::Url),
            Snippet::new("Forced text", "https://a.b", Kind::Text),
            Snippet::new("Tpl", "Hi {cursor}\n@ tricky line", Kind::Template),
        ];
        let text = serialize("# my notes", &original);
        assert!(text.starts_with("# my notes\n"));
        let back = parse(&text);
        assert_eq!(back.len(), 3);
        for (a, b) in original.iter().zip(&back) {
            assert_eq!((&a.name, &a.body, a.kind), (&b.name, &b.body, b.kind));
        }
    }

    #[test]
    fn detects_bare_url() {
        assert_eq!(parse("@ x\nhttps://example.com\n")[0].kind, Kind::Url);
    }

    #[test]
    fn search_ranks_name_prefix_first() {
        let s = parse("@ Address home\n1 Main St\n@ Email address\nme@x.com\n@ Other\nnothing\n");
        let none = |_: &Snippet| 0.0;
        let r = search(&s, "addr", none);
        assert_eq!(r, vec![0, 1]);
        assert_eq!(search(&s, "main", none), vec![0]);
        assert_eq!(search(&s, "", none).len(), 3);
        assert!(search(&s, "zzz", none).is_empty());
    }

    #[test]
    fn pin_tags_parse_and_round_trip() {
        let s = parse("@ A [url, pin]\nhttps://a.b\n@ B [pin]\nhi\n@ C [Pinned, TEXT]\nhttps://c.d\n@ D [draft]\nx\n");
        assert_eq!((s[0].kind, s[0].pinned), (Kind::Url, true));
        assert_eq!((s[1].name.as_str(), s[1].kind, s[1].pinned), ("B", Kind::Text, true));
        assert_eq!((s[2].kind, s[2].pinned), (Kind::Text, true));
        assert_eq!((s[3].name.as_str(), s[3].pinned), ("D [draft]", false)); // unknown tag stays in the name
        let text = serialize("#", &s);
        assert!(text.contains("@ A [pin]\n")); // url is detected anyway
        assert!(text.contains("@ C [text, pin]\n"));
        let back = parse(&text);
        for (a, b) in s.iter().zip(&back) {
            assert_eq!((&a.name, a.kind, a.pinned), (&b.name, b.kind, b.pinned));
        }
    }

    #[test]
    fn order_is_pinned_then_match_then_usage() {
        let s = parse("@ Alpha\na\n@ Beta\nb\n@ Gamma [pin]\ng\n@ Delta\nd\n");
        let usage = |x: &Snippet| match x.name.as_str() {
            "Delta" => 5.0,
            "Beta" => 2.0,
            _ => 0.0,
        };
        // Empty search: pinned, then most used, then file order.
        assert_eq!(search(&s, "", usage), vec![2, 3, 1, 0]);
        // While searching, the match decides and usage only breaks ties.
        let s = parse("@ Note one\nx\n@ Note two\ny\n");
        let usage = |x: &Snippet| if x.name == "Note two" { 1.0 } else { 0.0 };
        assert_eq!(search(&s, "note", usage), vec![1, 0]);
    }
}
