use nucleo_matcher::{Matcher, Utf32Str};
use pinyin::ToPinyinMulti;

use super::model::AppEntry;

/// Compute the full pinyin (syllables concatenated, no tones) and the initials
/// (first letters) for a Chinese / mixed string. Non-han characters are appended
/// verbatim (lowercased) so english names still match.
///
/// For heteronym (multi-pronunciation) characters, *every* reading is included
/// so the launcher matches regardless of which pronunciation the user types.
pub fn pinyin_fields(input: &str) -> (String, String) {
    let mut full = String::with_capacity(input.len() + 8);
    let mut abbr = String::with_capacity(input.len());

    for ch in input.chars() {
        if let Some(multi) = ch.to_pinyin_multi() {
            let mut seen_plain = std::collections::BTreeSet::new();
            let mut seen_abbr = std::collections::BTreeSet::new();
            for py in multi {
                let plain = py.plain();
                let letter = py.first_letter();
                if seen_plain.insert(plain) {
                    full.push_str(plain);
                }
                if seen_abbr.insert(letter) {
                    abbr.push_str(letter);
                }
            }
        } else {
            full.push(ch.to_ascii_lowercase());
            abbr.push(ch.to_ascii_lowercase());
        }
    }
    (full, abbr)
}

/// Convert a string into a `Utf32Str`. Non-ASCII input is materialized into
/// `buf`. Uses `hay_buf`/`query_buf` separately so the haystack and needle can
/// both be non-ASCII without aliasing each other's scratch memory.
fn to_utf32<'a>(s: &'a str, buf: &'a mut Vec<char>) -> Utf32Str<'a> {
    Utf32Str::new(s, buf)
}

/// Fuzzy-match a query against one candidate field. Returns the score or `None`.
fn field_score(
    matcher: &mut Matcher,
    field: &str,
    query: &str,
    field_buf: &mut Vec<char>,
    query_buf: &mut Vec<char>,
) -> Option<u16> {
    let field_utf = to_utf32(field, field_buf);
    let query_utf = to_utf32(query, query_buf);
    matcher.fuzzy_match(field_utf, query_utf)
}

/// Score a single entry against the query across its name, pinyin abbreviation
/// and full pinyin. Returns the best score among the three, or `None`.
fn score(
    entry: &AppEntry,
    matcher: &mut Matcher,
    query: &str,
    name_buf: &mut Vec<char>,
    abbr_buf: &mut Vec<char>,
    full_buf: &mut Vec<char>,
    query_buf: &mut Vec<char>,
) -> Option<u16> {
    let mut best = field_score(matcher, &entry.name, query, name_buf, query_buf);
    if !entry.pinyin_abbr.is_empty() {
        if let Some(s) = field_score(matcher, &entry.pinyin_abbr, query, abbr_buf, query_buf) {
            best = Some(best.map_or(s, |b| b.max(s)));
        }
    }
    if !entry.pinyin_full.is_empty() {
        if let Some(s) = field_score(matcher, &entry.pinyin_full, query, full_buf, query_buf) {
            best = Some(best.map_or(s, |b| b.max(s)));
        }
    }
    best
}

/// Rank `items` against `query`, returning the indexes best-match first.
/// An empty (or whitespace-only) query returns everything in the original order.
pub fn rank(
    items: &[AppEntry],
    query: &str,
    matcher: &mut Matcher,
    scratch: &mut MatcherScratch,
) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..items.len()).collect();
    }

    let mut results: Vec<(usize, u16)> = Vec::with_capacity(items.len());
    for (i, entry) in items.iter().enumerate() {
        if let Some(s) = score(
            entry,
            matcher,
            query,
            &mut scratch.name_buf,
            &mut scratch.abbr_buf,
            &mut scratch.full_buf,
            &mut scratch.query_buf,
        ) {
            results.push((i, s));
        }
    }

    results.sort_by(|a, b| b.1.cmp(&a.1));
    results.into_iter().map(|(i, _)| i).collect()
}

/// Scratch buffers reused across a batch of searches to avoid reallocations.
#[derive(Default)]
pub struct MatcherScratch {
    pub name_buf: Vec<char>,
    pub abbr_buf: Vec<char>,
    pub full_buf: Vec<char>,
    pub query_buf: Vec<char>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleo_matcher::{Config, Matcher};

    fn entry(id: &str, name: &str) -> AppEntry {
        let (pf, pa) = pinyin_fields(name);
        AppEntry {
            id: id.to_string(),
            name: name.to_string(),
            exec: String::new(),
            icon_path: None,
            hidden: false,
            pinyin_full: pf,
            pinyin_abbr: pa,
        }
    }

    fn matching(query: &str, names: &[&str]) -> Vec<String> {
        let apps: Vec<AppEntry> = names
            .iter()
            .enumerate()
            .map(|(i, n)| entry(&i.to_string(), n))
            .collect();
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let idxs = rank(&apps, query, &mut matcher, &mut scratch);
        idxs.into_iter().map(|i| apps[i].name.clone()).collect()
    }

    #[test]
    fn pinyin_fields_are_computed() {
        let (full, abbr) = pinyin_fields("网易云音乐");
        // Full pinyin includes the syllables of each character.
        assert!(full.contains("wangyiyun"), "full={full}");
        // Heteronym "乐" should retain its alternate "yue" reading.
        assert!(full.contains("yue"), "full={full}");
        // Abbreviation records the first letters (incl. multi-reading ones), so a
        // user typing "wyyyy" can still match.
        assert!(abbr.len() >= 5, "abbr={abbr}");
    }

    #[test]
    fn heteronym_abbr_matches_any_reading() {
        // "yue" should match even though the default reading of 乐 is "le".
        let got = matching("wyyyy", &["Firefox", "网易云音乐"]);
        assert_eq!(got[0], "网易云音乐");
    }

    #[test]
    fn empty_query_returns_all_in_order() {
        let got = matching("", &["Firefox", "Alacritty", "GIMP"]);
        assert_eq!(got, vec!["Firefox", "Alacritty", "GIMP"]);
    }

    #[test]
    fn matches_full_pinyin() {
        let got = matching("wangyiyun", &["Firefox", "网易云音乐"]);
        assert_eq!(got[0], "网易云音乐");
    }

    #[test]
    fn matches_latin_substring() {
        let got = matching("firef", &["Firefox", "Chromium"]);
        assert_eq!(got[0], "Firefox");
    }

    #[test]
    fn no_match_returns_empty() {
        let got = matching("zzzz", &["Firefox", "Alacritty"]);
        assert!(got.is_empty());
    }
}
