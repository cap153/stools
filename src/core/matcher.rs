use nucleo_matcher::{Matcher, Utf32Str};
use pinyin::ToPinyinMulti;

use super::history::HistoryRecord;
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
/// If `history` is provided, recently-used items get a boost and empty queries
/// sort by recency.
pub fn rank(
    items: &[AppEntry],
    query: &str,
    matcher: &mut Matcher,
    scratch: &mut MatcherScratch,
    history: Option<&std::collections::HashMap<String, HistoryRecord>>,
) -> Vec<usize> {
    let query = query.trim();

    if query.is_empty() {
        // Strict three-tier ordering: history (most recent first) → desktop apps
        // → binaries, preserving original (scan) order within each tier.
        let mut idxs: Vec<usize> = (0..items.len()).collect();
        idxs.sort_by(|&a, &b| {
            let ea = &items[a];
            let eb = &items[b];

            let ha = history.and_then(|h| h.get(&ea.id)).map_or(0u64, |r| r.last_used);
            let hb = history.and_then(|h| h.get(&eb.id)).map_or(0u64, |r| r.last_used);

            // Tier 1: entries with history sort by recency (newest first).
            match (ha > 0, hb > 0) {
                (true, true) => return hb.cmp(&ha),
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                (false, false) => {}
            }

            // Tier 2: desktop apps sort before binaries.
            match (&ea.kind, &eb.kind) {
                (crate::core::model::EntryKind::Desktop, crate::core::model::EntryKind::Binary) => {
                    return std::cmp::Ordering::Less;
                }
                (crate::core::model::EntryKind::Binary, crate::core::model::EntryKind::Desktop) => {
                    return std::cmp::Ordering::Greater;
                }
                _ => {}
            }

            // Tier 3: stable within the same kind (original scan order).
            a.cmp(&b)
        });
        return idxs;
    }

    let mut results: Vec<(usize, u16)> = Vec::with_capacity(items.len());
    for (i, entry) in items.iter().enumerate() {
        if let Some(mut s) = score(
            entry,
            matcher,
            query,
            &mut scratch.name_buf,
            &mut scratch.abbr_buf,
            &mut scratch.full_buf,
            &mut scratch.query_buf,
        ) {
            // Desktop apps get a base priority so GUI software ranks above CLI
            // tools of comparable match quality (without drowning out a strong
            // exact CLI match).
            if entry.kind == crate::core::model::EntryKind::Desktop {
                s = s.saturating_add(150);
            }
            // History boost: frequently-used items climb higher.
            if let Some(hist) = history {
                if let Some(h) = hist.get(&entry.id) {
                    let boost = (h.count.min(20) * 15) as u16;
                    s = s.saturating_add(boost);
                }
            }
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
            kind: crate::core::model::EntryKind::Desktop,
            subtitle: None,
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
        let idxs = rank(&apps, query, &mut matcher, &mut scratch, None);
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

    #[test]
    fn empty_query_orders_desktop_before_binary() {
        // Mixed kinds, no history: desktop apps must come before binaries,
        // each in original scan order within its tier.
        let mut apps: Vec<AppEntry> = ["Btop", "Zed"].iter().enumerate()
            .map(|(i, n)| entry(&i.to_string(), n))
            .collect();
        let (f1, a1) = pinyin_fields("vimdot");
        let (f2, a2) = pinyin_fields("true");
        apps.push(AppEntry {
            id: "bin:/usr/bin/vimdot".into(),
            name: "vimdot".into(),
            exec: "/usr/bin/vimdot".into(),
            icon_path: None,
            hidden: false,
            pinyin_full: f1,
            pinyin_abbr: a1,
            kind: crate::core::model::EntryKind::Binary,
            subtitle: None,
        });
        apps.push(AppEntry {
            id: "bin:/usr/bin/true".into(),
            name: "true".into(),
            exec: "/usr/bin/true".into(),
            icon_path: None,
            hidden: false,
            pinyin_full: f2,
            pinyin_abbr: a2,
            kind: crate::core::model::EntryKind::Binary,
            subtitle: None,
        });

        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let idxs = rank(&apps, "", &mut matcher, &mut scratch, None);
        let kinds: Vec<_> = idxs.iter().map(|&i| &apps[i].kind).collect();
        assert_eq!(kinds[0], &crate::core::model::EntryKind::Desktop);
        assert_eq!(kinds[1], &crate::core::model::EntryKind::Desktop);
        assert_eq!(kinds[2], &crate::core::model::EntryKind::Binary);
        assert_eq!(kinds[3], &crate::core::model::EntryKind::Binary);
        // Names within each tier keep scan order (steam/sorted): Btop,Zed then vimdot,true
        let names: Vec<_> = idxs.into_iter().map(|i| apps[i].name.clone()).collect();
        assert_eq!(names, vec!["Btop", "Zed", "vimdot", "true"]);
    }

    #[test]
    fn empty_query_sorts_most_recent_first() {
        let apps: Vec<AppEntry> = ["Firefox", "Alacritty", "GIMP"]
            .iter()
            .enumerate()
            .map(|(i, n)| entry(&i.to_string(), n))
            .collect();
        let mut hist = std::collections::HashMap::new();
        hist.insert("1".into(), HistoryRecord { last_used: 300, count: 1 }); // Alacritty newest
        hist.insert("0".into(), HistoryRecord { last_used: 100, count: 1 }); // Firefox older
        // GIMP has no history → should sort after any-with-history, before/after by stability.

        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let idxs = rank(&apps, "", &mut matcher, &mut scratch, Some(&hist));
        let names: Vec<String> = idxs.into_iter().map(|i| apps[i].name.clone()).collect();
        // Most recent (Alacritty) first, then the older history entry, then the rest.
        assert_eq!(names[0], "Alacritty");
        assert_eq!(names[1], "Firefox");
    }

    #[test]
    fn history_boost_promotes_frequent_item() {
        // Both match query "a". Alacritty is used a lot → should rank above Firefox
        // despite Firefox scoring better on the raw fuzzy match.
        let apps: Vec<AppEntry> = ["Firefox", "Alacritty"].iter().enumerate()
            .map(|(i, n)| entry(&i.to_string(), n))
            .collect();
        let mut hist = std::collections::HashMap::new();
        hist.insert("1".into(), HistoryRecord { last_used: 10, count: 10 }); // Alacritty heavy use

        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let idxs = rank(&apps, "a", &mut matcher, &mut scratch, Some(&hist));
        let names: Vec<String> = idxs.into_iter().map(|i| apps[i].name.clone()).collect();
        assert_eq!(names[0], "Alacritty", "got {names:?}");
    }
}
