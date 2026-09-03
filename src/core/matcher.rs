use bincode::{Decode, Encode};
use nucleo_matcher::{Matcher, Utf32Str};
use pinyin::ToPinyinMulti;

use super::history::HistoryRecord;
use super::model::AppEntry;

/// Where one character's pinyin lives inside the precomputed pinyin fields.
///
/// `abbr_start..abbr_end` / `full_start..full_end` are indices into
/// `pinyin_abbr` / `pinyin_full` (both pure ASCII, so char == byte offsets).
/// Heteronym characters own several disjoint ranges — one per reading — so any
/// hit inside them highlights the same underlying character.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Encode, Decode,
)]
pub struct FieldIndicesEntry {
    pub name_idx: usize,
    pub abbr_start: usize,
    pub abbr_end: usize,
    pub full_start: usize,
    pub full_end: usize,
}

/// The full reverse map for one entry, sorted by pinyin offset so the entry
/// covering a match can be found with a binary search.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Encode, Decode,
)]
pub struct FieldIndices {
    /// Sorted by `abbr_start` (and, for equal starts, `full_start`).
    pub entries: Vec<FieldIndicesEntry>,
}

impl FieldIndices {
    /// Character index in `name` owning the abbreviation character at `idx`.
    pub fn name_idx_for_abbr(&self, idx: usize) -> Option<usize> {
        self.lookup(idx, true)
    }

    /// Character index in `name` owning the full-pinyin character at `idx`.
    pub fn name_idx_for_full(&self, idx: usize) -> Option<usize> {
        self.lookup(idx, false)
    }

    fn lookup(&self, idx: usize, abbr: bool) -> Option<usize> {
        let start = |e: &FieldIndicesEntry| if abbr { e.abbr_start } else { e.full_start };
        let end = |e: &FieldIndicesEntry| if abbr { e.abbr_end } else { e.full_end };
        // `entries` is sorted by start, so the first entry whose start is past
        // `idx` bounds the candidate range.
        let pos = self.entries.partition_point(|e| start(e) <= idx);
        let entry = pos.checked_sub(1).and_then(|p| self.entries.get(p))?;
        (start(entry) <= idx && idx < end(entry)).then_some(entry.name_idx)
    }
}

/// Compute the full pinyin (syllables concatenated, no tones) and the initials
/// (first letters) for a Chinese / mixed string, plus the reverse index map that
/// points every pinyin character back to the character it belongs to.
/// Non-han characters are appended verbatim (lowercased) so english names still
/// match.
///
/// For heteronym (multi-pronunciation) characters, *every* reading is included
/// so the launcher matches regardless of which pronunciation the user types.
pub fn pinyin_fields(input: &str) -> (String, String, FieldIndices) {
    let mut full = String::with_capacity(input.len() + 8);
    let mut abbr = String::with_capacity(input.len());
    let mut entries = Vec::with_capacity(input.len());

    let abbr_len = |s: &str| s.chars().count();

    for (name_idx, ch) in input.chars().enumerate() {
        let abbr_start = abbr_len(&abbr);
        let full_start = abbr_len(&full);
        let mut handled = false;

        if let Some(multi) = ch.to_pinyin_multi() {
            let mut seen_plain = std::collections::BTreeSet::new();
            let mut seen_abbr = std::collections::BTreeSet::new();
            for py in multi {
                let plain = py.plain();
                let letter = py.first_letter();
                if seen_plain.insert(plain) {
                    full.push_str(plain);
                    entries.push(FieldIndicesEntry {
                        name_idx,
                        abbr_start: abbr_start + seen_abbr.len(),
                        abbr_end: abbr_start + seen_abbr.len() + 1,
                        full_start: abbr_len(&full) - abbr_len(plain),
                        full_end: abbr_len(&full),
                    });
                    handled = true;
                }
                if seen_abbr.insert(letter) {
                    if let Some(initial) = letter.chars().next() {
                        abbr.push(initial);
                    }
                }
            }
        }

        if !handled {
            // Non-han character: it maps to itself in both fields.
            full.push(ch.to_ascii_lowercase());
            abbr.push(ch.to_ascii_lowercase());
            entries.push(FieldIndicesEntry {
                name_idx,
                abbr_start,
                abbr_end: abbr_len(&abbr),
                full_start,
                full_end: abbr_len(&full),
            });
        }
    }

    entries.sort_by_key(|e| (e.abbr_start, e.full_start));
    (full, abbr, FieldIndices { entries })
}

/// One run of characters for the UI to render (matched runs get the highlight
/// colour and a heavier weight).
pub struct TextSpanData {
    pub text: String,
    pub is_match: bool,
}

/// Stack-allocated match bitmap used by [`build_highlight_spans`]. App / binary
/// names in the launcher never exceed this in practice, so a fixed array avoids
/// per-keystroke heap traffic (a `HashSet` was visible in the keystroke budget).
const MAX_HIGHLIGHT_CHARS: usize = 256;

/// Split `name` into consecutive runs of matched / unmatched characters.
/// `matched_indices` are character indices into `name`.
pub fn build_highlight_spans(name: &str, matched_indices: &[usize]) -> Vec<TextSpanData> {
    if matched_indices.is_empty() {
        return vec![TextSpanData {
            text: name.to_string(),
            is_match: false,
        }];
    }

    let mut is_match_map = [false; MAX_HIGHLIGHT_CHARS];
    for &idx in matched_indices {
        if idx < MAX_HIGHLIGHT_CHARS {
            is_match_map[idx] = true;
        }
    }

    let mut spans: Vec<TextSpanData> = Vec::with_capacity(4);
    for (idx, ch) in name.chars().enumerate() {
        let is_match = idx < MAX_HIGHLIGHT_CHARS && is_match_map[idx];
        match spans.last_mut() {
            Some(span) if span.is_match == is_match => span.text.push(ch),
            _ => spans.push(TextSpanData {
                text: ch.to_string(),
                is_match,
            }),
        }
    }
    spans
}

/// Character indices of `name` that `query` matched, for the highlight layer.
///
/// Mirrors [`score`]: the name itself is tried first, then the pinyin
/// abbreviation and finally the full pinyin, and the winning field's indices are
/// mapped back to the characters that produced them.
pub fn highlight_indices(
    entry: &AppEntry,
    query: &str,
    matcher: &mut Matcher,
    scratch: &mut MatcherScratch,
) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let mut indices: Vec<u32> = Vec::new();
    let (name_buf, abbr_buf, full_buf, query_buf) = scratch.buffers_mut();

    // 1. The name itself (latin names, or typing the characters verbatim).
    let name_utf = to_utf32(&entry.name, name_buf);
    let query_utf = to_utf32(query, query_buf);
    if matcher
        .fuzzy_indices(name_utf, query_utf, &mut indices)
        .is_some()
    {
        return dedup_sorted(indices.into_iter().map(|i| i as usize));
    }

    // 2. Pinyin initials: one abbreviation character per name character.
    if !entry.pinyin_abbr.is_empty() {
        let abbr_utf = to_utf32(&entry.pinyin_abbr, abbr_buf);
        let query_utf = to_utf32(query, query_buf);
        if matcher
            .fuzzy_indices(abbr_utf, query_utf, &mut indices)
            .is_some()
        {
            // nucleo returns indices in ascending order, and the abbr map
            // preserves that ordering, so we can dedup while collecting.
            let mut out = Vec::with_capacity(indices.len());
            let mut last = usize::MAX;
            for i in indices {
                if let Some(name_idx) = entry.pinyin_indices.name_idx_for_abbr(i as usize) {
                    if name_idx != last {
                        out.push(name_idx);
                        last = name_idx;
                    }
                }
            }
            return out;
        }
    }

    // 3. Full pinyin: a hit anywhere in a syllable highlights the character it
    //    belongs to ("wangyi" → 网,易). Multiple syllables of the same character
    //    can match (heteronym), so dedup after mapping.
    if !entry.pinyin_full.is_empty() {
        let full_utf = to_utf32(&entry.pinyin_full, full_buf);
        let query_utf = to_utf32(query, query_buf);
        if matcher
            .fuzzy_indices(full_utf, query_utf, &mut indices)
            .is_some()
        {
            let mut out = Vec::with_capacity(indices.len());
            let mut last = usize::MAX;
            for i in indices {
                if let Some(name_idx) = entry.pinyin_indices.name_idx_for_full(i as usize) {
                    if name_idx != last {
                        out.push(name_idx);
                        last = name_idx;
                    }
                }
            }
            return out;
        }
    }

    Vec::new()
}

/// `nucleo` returns indices in ascending order, so a single-pass dedup is
/// cheaper than a `HashSet` here.
fn dedup_sorted(it: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut last = usize::MAX;
    for v in it {
        if v != last {
            out.push(v);
            last = v;
        }
    }
    out
}

/// Convert a string into a `Utf32Str`. Non-ASCII input is materialized into
/// `buf`. Uses `hay_buf`/`query_buf` separately so the haystack and needle can
/// both be non-ASCII without aliasing each other's scratch memory.
fn to_utf32<'a>(s: &'a str, buf: &'a mut Vec<char>) -> Utf32Str<'a> {
    Utf32Str::new(s, buf)
}

/// Fuzzy-match a query against one candidate field. Returns the score or `None`.
///
/// `query` must already be converted: the caller hoists that conversion out of
/// the entry loop (it was rebuilt up to 3× per entry before, which dominated the
/// keystroke budget on large app lists).
fn field_score(
    matcher: &mut Matcher,
    field: &str,
    query: Utf32Str<'_>,
    field_buf: &mut Vec<char>,
) -> Option<u16> {
    let field_utf = to_utf32(field, field_buf);
    matcher.fuzzy_match(field_utf, query)
}

/// Score a single entry against the query across its name, pinyin abbreviation
/// and full pinyin. Returns the best score among the three, or `None`.
fn score(
    entry: &AppEntry,
    matcher: &mut Matcher,
    query: Utf32Str<'_>,
    name_buf: &mut Vec<char>,
    abbr_buf: &mut Vec<char>,
    full_buf: &mut Vec<char>,
) -> Option<u16> {
    let mut best = field_score(matcher, &entry.name, query, name_buf);
    if !entry.pinyin_abbr.is_empty() {
        if let Some(s) = field_score(matcher, &entry.pinyin_abbr, query, abbr_buf) {
            best = Some(best.map_or(s, |b| b.max(s)));
        }
    }
    if !entry.pinyin_full.is_empty() {
        if let Some(s) = field_score(matcher, &entry.pinyin_full, query, full_buf) {
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
        // Alias entries (secondary-language `.desktop` names) are excluded so the
        // first screen shows only the primary name, not "关机" and "Power Off" side
        // by side. They still participate once a query is typed (below).
        let mut idxs: Vec<usize> = (0..items.len())
            .filter(|&i| !items[i].is_alias)
            .collect();
        idxs.sort_by(|&a, &b| {
            let ea = &items[a];
            let eb = &items[b];

            let ha = history
                .and_then(|h| h.get(&ea.id))
                .map_or(0u64, |r| r.last_used);
            let hb = history
                .and_then(|h| h.get(&eb.id))
                .map_or(0u64, |r| r.last_used);

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

    // The query is encoded exactly once for the whole scan instead of once per
    // field comparison (3 × items.len() times before).
    let query_utf = to_utf32(query, &mut scratch.query_buf);

    let mut results: Vec<(usize, u16)> = Vec::with_capacity(items.len());
    for (i, entry) in items.iter().enumerate() {
        if let Some(mut s) = score(
            entry,
            matcher,
            query_utf,
            &mut scratch.name_buf,
            &mut scratch.abbr_buf,
            &mut scratch.full_buf,
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
///
/// Each field keeps its own buffer so a haystack and a non-ASCII needle never
/// alias, and so the query can be encoded once and shared (the resulting
/// `Utf32Str` is `Copy` and only borrows `query_buf`, which the `*_as_utf32`
/// helpers hand out one at a time).
#[derive(Default)]
pub struct MatcherScratch {
    pub name_buf: Vec<char>,
    pub abbr_buf: Vec<char>,
    pub full_buf: Vec<char>,
    pub query_buf: Vec<char>,
}

impl MatcherScratch {
    /// Split the buffers into independent mutable borrows.
    ///
    /// Each `Utf32Str` only borrows the buffer it was encoded into, so — unlike
    /// `&mut self` helper methods — this lets a hoisted query stay alive while
    /// haystacks are encoded against the other buffers.
    pub fn buffers_mut(
        &mut self,
    ) -> (
        &mut Vec<char>,
        &mut Vec<char>,
        &mut Vec<char>,
        &mut Vec<char>,
    ) {
        (
            &mut self.name_buf,
            &mut self.abbr_buf,
            &mut self.full_buf,
            &mut self.query_buf,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleo_matcher::{Config, Matcher};

    fn entry(id: &str, name: &str) -> AppEntry {
        let (pf, pa, pi) = pinyin_fields(name);
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
            pinyin_indices: pi,
            is_alias: false,
        }
    }

    /// Character indices of `name` highlighted by `query`.
    fn highlighted(name: &str, query: &str) -> Vec<usize> {
        let e = entry("0", name);
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let mut idxs = highlight_indices(&e, query, &mut matcher, &mut scratch);
        idxs.sort_unstable();
        idxs
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

    /// Timing harness (not an assertion): `cargo test --release keystroke -- --nocapture`
    /// prints the per-keystroke cost of ranking a realistic (~3.6k entry) list.
    #[test]
    fn keystroke_budget() {
        let mut apps: Vec<AppEntry> = Vec::new();
        let Ok(rd) = std::fs::read_dir("/usr/bin") else {
            return;
        };
        for (i, e) in rd.flatten().enumerate().take(3000) {
            let name = e.file_name().to_string_lossy().into_owned();
            apps.push(entry(&format!("bin:{i}"), &name));
        }
        for (i, name) in [
            "网易云音乐",
            "腾讯会议",
            "Visual Studio Code",
            "MATE 顏色選擇區",
        ]
        .iter()
        .enumerate()
        {
            apps.push(entry(&format!("desktop:{i}"), name));
        }
        // Pad to the size of a real index (~3.6k entries).
        while apps.len() < 3600 {
            let i = apps.len();
            apps.push(entry(&format!("pad:{i}"), &format!("tool-number-{i}")));
        }
        if apps.is_empty() {
            return;
        }

        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();
        for query in ["f", "fire", "firef", "wyy", "wangyi", "qute"] {
            let start = std::time::Instant::now();
            let idxs = rank(&apps, query, &mut matcher, &mut scratch, None);
            let ranked = start.elapsed();
            let start = std::time::Instant::now();
            for &i in idxs.iter().take(30) {
                std::hint::black_box(highlight_indices(
                    &apps[i],
                    query,
                    &mut matcher,
                    &mut scratch,
                ));
            }
            let highlighted = start.elapsed();
            println!(
                "[bench] query={query:<7} n={} rank={:?} highlight30={:?}",
                apps.len(),
                ranked,
                highlighted
            );
        }
    }

    #[test]
    fn highlights_latin_substring() {
        // "qute" → the leading run of qutebrowser.
        assert_eq!(highlighted("qutebrowser", "qute"), vec![0, 1, 2, 3]);
    }

    #[test]
    fn highlights_pinyin_initials() {
        // "wyy" hits 网,易,云 → the original characters are highlighted.
        assert_eq!(highlighted("网易云音乐", "wyy"), vec![0, 1, 2]);
    }

    #[test]
    fn highlights_full_pinyin() {
        // "wangyi" spans two syllables → 网,易.
        assert_eq!(highlighted("网易云音乐", "wangyi"), vec![0, 1]);
    }

    #[test]
    fn highlights_typed_characters() {
        // Typing the characters themselves matches the name directly.
        assert_eq!(highlighted("网易云音乐", "音乐"), vec![3, 4]);
    }

    #[test]
    fn highlights_nothing_for_empty_query() {
        assert!(highlighted("网易云音乐", "").is_empty());
    }

    #[test]
    fn spans_merge_adjacent_runs() {
        let idx: Vec<usize> = vec![0, 1, 2];
        let spans = build_highlight_spans("网易云音乐", &idx);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "网易云");
        assert!(spans[0].is_match);
        assert_eq!(spans[1].text, "音乐");
        assert!(!spans[1].is_match);
    }

    #[test]
    fn spans_without_matches_are_a_single_run() {
        let spans = build_highlight_spans("Firefox", &[]);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "Firefox");
        assert!(!spans[0].is_match);
    }

    #[test]
    fn pinyin_fields_are_computed() {
        let (full, abbr, _) = pinyin_fields("网易云音乐");
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
    fn aliases_hidden_when_query_empty_but_matchable_when_typed() {
        let (pf, pa, pi) = pinyin_fields("关机");
        let primary = AppEntry {
            id: "d:poweroff.desktop".into(),
            name: "关机".into(),
            exec: "systemctl poweroff".into(),
            icon_path: None,
            hidden: false,
            pinyin_full: pf,
            pinyin_abbr: pa,
            kind: crate::core::model::EntryKind::Desktop,
            subtitle: None,
            pinyin_indices: pi,
            is_alias: false,
        };
        let (pf, pa, pi) = pinyin_fields("Power Off");
        let alias = AppEntry {
            id: "d:poweroff.desktop:alias".into(),
            name: "Power Off".into(),
            exec: "systemctl poweroff".into(),
            icon_path: None,
            hidden: false,
            pinyin_full: pf,
            pinyin_abbr: pa,
            kind: crate::core::model::EntryKind::Desktop,
            subtitle: None,
            pinyin_indices: pi,
            is_alias: true,
        };
        let apps = vec![primary, alias];

        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();

        // Empty query: only the primary shows (no "twins").
        let empty = rank(&apps, "", &mut matcher, &mut scratch, None);
        assert_eq!(empty.len(), 1);
        assert_eq!(apps[empty[0]].name, "关机");

        // Typing an English query surfaces the alias and highlights it.
        let en = rank(&apps, "power", &mut matcher, &mut scratch, None);
        let hit_alias = en.iter().any(|&i| apps[i].name == "Power Off");
        assert!(hit_alias, "alias not surfaced by english query");
    }

    #[test]
    fn empty_query_orders_desktop_before_binary() {
        // Mixed kinds, no history: desktop apps must come before binaries,
        // each in original scan order within its tier.
        let mut apps: Vec<AppEntry> = ["Btop", "Zed"]
            .iter()
            .enumerate()
            .map(|(i, n)| entry(&i.to_string(), n))
            .collect();
        let (f1, a1, i1) = pinyin_fields("vimdot");
        let (f2, a2, i2) = pinyin_fields("true");
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
            pinyin_indices: i1,
            is_alias: false,
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
            pinyin_indices: i2,
            is_alias: false,
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
        hist.insert(
            "1".into(),
            HistoryRecord {
                last_used: 300,
                count: 1,
            },
        ); // Alacritty newest
        hist.insert(
            "0".into(),
            HistoryRecord {
                last_used: 100,
                count: 1,
            },
        ); // Firefox older
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
        let apps: Vec<AppEntry> = ["Firefox", "Alacritty"]
            .iter()
            .enumerate()
            .map(|(i, n)| entry(&i.to_string(), n))
            .collect();
        let mut hist = std::collections::HashMap::new();
        hist.insert(
            "1".into(),
            HistoryRecord {
                last_used: 10,
                count: 10,
            },
        ); // Alacritty heavy use

        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let idxs = rank(&apps, "a", &mut matcher, &mut scratch, Some(&hist));
        let names: Vec<String> = idxs.into_iter().map(|i| apps[i].name.clone()).collect();
        assert_eq!(names[0], "Alacritty", "got {names:?}");
    }
}
