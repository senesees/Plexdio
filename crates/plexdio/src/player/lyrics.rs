//! Lyrics: parsing what the server returns for a lyrics stream (`/library/streams/{id}`).
//!
//! Plex keeps lyrics as a `Stream` with `streamType=4` on the track's media part. Sidecar `.lrc`
//! files are time-synced, `.txt` files are plain. The server sends the file text; some versions
//! wrap it in a JSON `MediaContainer` with `Lyrics` → `Line` → `Span` entries instead, which is
//! accepted too.

/// One line of lyrics; `at_ms` is `None` for plain (unsynced) lyrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub at_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// Every line has a time.
    pub timed: bool,
    /// Where the lyrics come from, when the server says (e.g. "LyricFind").
    pub provider: Option<String>,
}

impl Lyrics {
    /// Index of the line being sung at `pos_ms` (the last line that has started).
    pub fn current(&self, pos_ms: u64) -> Option<usize> {
        if !self.timed {
            return None;
        }
        let n = self.lines.partition_point(|l| l.at_ms.is_some_and(|t| t <= pos_ms));
        n.checked_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.text.trim().is_empty())
    }
}

/// `[mm:ss.xx]` / `[mm:ss]` / `[mm:ss:xx]` → milliseconds.
fn stamp(tag: &str) -> Option<u64> {
    let (m, rest) = tag.split_once(':')?;
    let m: u64 = m.trim().parse().ok()?;
    let (s, frac) = match rest.split_once(['.', ':']) {
        Some((s, f)) => (s, f),
        None => (rest, ""),
    };
    let s: u64 = s.trim().parse().ok()?;
    if s >= 60 {
        return None;
    }
    let frac_ms = if frac.is_empty() {
        0
    } else {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).take(3).collect();
        let v: u64 = digits.parse().ok()?;
        match digits.len() {
            1 => v * 100,
            2 => v * 10,
            _ => v,
        }
    };
    Some(m * 60_000 + s * 1000 + frac_ms)
}

/// Parse LRC (`[00:12.34]text`, several stamps per line allowed, `[offset:±ms]` honoured,
/// other `[tag:value]` lines skipped). Text without any stamps comes back as plain lyrics.
pub fn parse_text(text: &str) -> Lyrics {
    let text = text.trim_start_matches('\u{feff}');
    let mut timed: Vec<Line> = Vec::new();
    let mut plain: Vec<Line> = Vec::new();
    let mut offset: i64 = 0;
    for raw in text.lines() {
        let mut rest = raw.trim_end();
        let mut stamps = Vec::new();
        let mut tag_only = false;
        while let Some(body) = rest.strip_prefix('[') {
            let Some(end) = body.find(']') else { break };
            let tag = &body[..end];
            match stamp(tag) {
                Some(ms) => stamps.push(ms),
                None => {
                    // Metadata tag ([ar:], [ti:], [offset:]); not part of the text.
                    if let Some(v) = tag.strip_prefix("offset:") {
                        offset = v.trim().parse().unwrap_or(0);
                    }
                    tag_only = true;
                }
            }
            rest = &body[end + 1..];
        }
        if stamps.is_empty() {
            if !tag_only {
                plain.push(Line { at_ms: None, text: raw.trim().to_string() });
            }
            continue;
        }
        for ms in stamps {
            timed.push(Line { at_ms: Some(ms), text: rest.trim().to_string() });
        }
    }
    if timed.is_empty() {
        // Trim blank lines at both ends of plain lyrics.
        while plain.first().is_some_and(|l| l.text.is_empty()) {
            plain.remove(0);
        }
        while plain.last().is_some_and(|l| l.text.is_empty()) {
            plain.pop();
        }
        return Lyrics { lines: plain, timed: false, provider: None };
    }
    // A positive LRC offset shows lyrics earlier.
    for l in &mut timed {
        l.at_ms = l.at_ms.map(|t| (t as i64 - offset).max(0) as u64);
    }
    timed.sort_by_key(|l| l.at_ms);
    Lyrics { lines: timed, timed: true, provider: None }
}

/// Parse the JSON form: `MediaContainer.Lyrics[].Line[]` with `startOffset` (ms) and
/// `Span[].text`. `None` when the body is not that shape.
fn parse_json(body: &str) -> Option<Lyrics> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let mc = v.get("MediaContainer")?;
    let lyrics = mc.get("Lyrics")?.as_array()?;
    let mut lines = Vec::new();
    for l in lyrics.iter().flat_map(|x| x.get("Line").and_then(|l| l.as_array()).into_iter().flatten()) {
        let text: String = l
            .get("Span")
            .and_then(|s| s.as_array())
            .map(|spans| spans.iter().filter_map(|s| s.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""))
            .unwrap_or_default();
        let at = l.get("startOffset").and_then(|o| o.as_u64().or_else(|| o.as_str().and_then(|s| s.parse().ok())));
        lines.push(Line { at_ms: at, text: text.trim().to_string() });
    }
    let timed = !lines.is_empty() && lines.iter().all(|l| l.at_ms.is_some());
    if timed {
        lines.sort_by_key(|l| l.at_ms);
    } else {
        lines.iter_mut().for_each(|l| l.at_ms = None);
    }
    let provider = lyrics.first().and_then(|x| x.get("provider")).and_then(|p| p.as_str()).map(str::to_string);
    Some(Lyrics { lines, timed, provider })
}

/// Parse whatever the server returned for a lyrics stream.
pub fn parse(body: &[u8]) -> Lyrics {
    let text = String::from_utf8_lossy(body);
    if text.trim_start().starts_with('{') {
        if let Some(l) = parse_json(&text) {
            return l;
        }
    }
    parse_text(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_with_repeats_offset_and_tags() {
        let l = parse_text("[ar:Someone]\n[offset:+500]\n[00:01.00]One\n[00:03.50][00:10]Chorus\n[00:05.2]Two\n");
        assert!(l.timed);
        let got: Vec<(u64, &str)> = l.lines.iter().map(|x| (x.at_ms.unwrap(), x.text.as_str())).collect();
        assert_eq!(got, vec![(500, "One"), (3000, "Chorus"), (4700, "Two"), (9500, "Chorus")]);
    }

    #[test]
    fn current_line_follows_the_position() {
        let l = parse_text("[00:01.00]a\n[00:02.00]b\n[00:03.00]c");
        assert_eq!(l.current(500), None, "before the first line");
        assert_eq!(l.current(1000), Some(0));
        assert_eq!(l.current(2999), Some(1));
        assert_eq!(l.current(60_000), Some(2));
    }

    #[test]
    fn plain_text_is_unsynced() {
        let l = parse_text("\n\nFirst line\n\nSecond verse\n\n");
        assert!(!l.timed);
        assert_eq!(l.lines.len(), 3);
        assert_eq!(l.lines[0].text, "First line");
        assert_eq!(l.current(10_000), None);
    }

    #[test]
    fn json_lines_and_bom() {
        let body = br#"{"MediaContainer":{"Lyrics":[{"provider":"LyricFind","Line":[
            {"startOffset":1200,"Span":[{"text":"Hello "},{"text":"there"}]},
            {"startOffset":"3400","Span":[{"text":"Again"}]}]}]}}"#;
        let l = parse(body);
        assert!(l.timed);
        assert_eq!(l.provider.as_deref(), Some("LyricFind"));
        assert_eq!(l.lines[0], Line { at_ms: Some(1200), text: "Hello there".into() });
        assert_eq!(l.lines[1].at_ms, Some(3400));
        let l = parse("\u{feff}[00:00.10]x".as_bytes());
        assert_eq!(l.lines[0].at_ms, Some(100));
    }

    #[test]
    fn bad_stamps_are_text() {
        let l = parse_text("[intro]\nJust words [not a stamp]");
        assert!(!l.timed);
        assert_eq!(l.lines, vec![Line { at_ms: None, text: "Just words [not a stamp]".into() }]);
    }
}
