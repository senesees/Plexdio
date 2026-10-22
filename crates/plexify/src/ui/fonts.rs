//! System-font fallback for characters the bundled egui fonts do not cover (CJK titles,
//! symbols, emoji). Installed as a `FontProvider`, so a font file is only read the first
//! time a character of that script actually appears — normal Latin-only libraries never
//! load anything.

use egui::{FallbackRequest, FontData, FontInsert};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Pretend every bundled font missed, and look in the OS's font directory.
pub fn system_provider() -> impl Fn(&FallbackRequest<'_>) -> Option<FontInsert> + Send + Sync {
    |req| {
        let c = req.cluster.chars().next()?;
        for path in candidates_for(c) {
            if let Some(data) = load(&path) {
                let name = format!("system:{}", path.rsplit(['/', '\\']).next().unwrap_or(&path));
                return Some(FontInsert { name, data, families: Vec::new() });
            }
        }
        None
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF   // Hangul Jamo
        | 0x2E80..=0x2FFF // CJK radicals, Kangxi
        | 0x3000..=0x31FF // CJK punctuation, kana, bopomofo, hangul compat
        | 0x3400..=0x4DBF // CJK ext A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compat ideographs
        | 0xFF00..=0xFFEF // halfwidth/fullwidth forms
        | 0x20000..=0x2FA1F)
}

fn is_symbol(c: char) -> bool {
    matches!(c as u32,
        0x2190..=0x21FF | 0x2300..=0x23FF | 0x25A0..=0x25FF | 0x2600..=0x27BF
        | 0x1F300..=0x1FAFF | 0xFE00..=0xFE0F)
}

#[cfg(windows)]
fn font_dir() -> String {
    std::env::var("WINDIR").map(|w| format!("{w}\\Fonts")).unwrap_or_else(|_| "C:\\Windows\\Fonts".into())
}

#[cfg(windows)]
fn candidates_for(c: char) -> Vec<String> {
    let dir = font_dir();
    let files: &[&str] = if is_cjk(c) {
        &["msyh.ttc", "YuGothM.ttc", "msgothic.ttc", "simsun.ttc", "malgun.ttf", "meiryo.ttc"]
    } else if is_symbol(c) {
        &["seguiemj.ttf", "seguisym.ttf", "arial.ttf", "segoeui.ttf"]
    } else {
        &["segoeui.ttf", "arial.ttf", "tahoma.ttf", "seguisym.ttf"]
    };
    files.iter().map(|f| format!("{dir}\\{f}")).collect()
}

#[cfg(not(windows))]
fn candidates_for(c: char) -> Vec<String> {
    let files: &[&str] = if is_cjk(c) {
        &[
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
        ]
    } else if is_symbol(c) {
        &["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf"]
    } else {
        &["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf"]
    };
    files.iter().map(|f| f.to_string()).collect()
}

/// Reading a CJK font file costs a few ms, so do it once per path.
fn load(path: &str) -> Option<FontData> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<FontData>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(path.to_string())
        .or_insert_with(|| std::fs::read(path).ok().map(FontData::from_owned))
        .clone()
}
