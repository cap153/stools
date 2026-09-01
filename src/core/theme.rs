//! Theme injection: Fuzzel-style colours and the font family chain.

use slint::{Color, SharedString};

use crate::core::config::ThemeConfig;
use crate::launcher::LauncherWindow;

/// Parse a Fuzzel colour literal into a Slint colour.
///
/// Fuzzel uses `RRGGBBAA` (`282a36dd`); `#`-prefixed values and the CSS-ish
/// `RGB`, `RGBA` and `RRGGBB` forms are accepted too. Invalid input falls back
/// to `fallback` so a typo cannot make the window invisible.
pub fn parse_fuzzel_color(hex: &str, fallback: Color) -> Color {
    let clean = hex.trim().trim_start_matches('#');
    if !clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return fallback;
    }

    let nibble = |i: usize| -> Option<u8> {
        let c = clean.as_bytes().get(i)?;
        (*c as char).to_digit(16).map(|d| d as u8)
    };
    let byte = |i: usize| -> Option<u8> {
        let hi = nibble(i)?;
        let lo = nibble(i + 1)?;
        Some(hi << 4 | lo)
    };
    // Short form: each digit is duplicated (f -> ff).
    let short = |i: usize| -> Option<u8> { nibble(i).map(|d| d << 4 | d) };

    let rgba = match clean.len() {
        3 => (short(0), short(1), short(2), Some(255)),
        4 => (short(0), short(1), short(2), short(3)),
        6 => (byte(0), byte(2), byte(4), Some(255)),
        8 => (byte(0), byte(2), byte(4), byte(6)),
        _ => return fallback,
    };

    match rgba {
        (Some(r), Some(g), Some(b), Some(a)) => Color::from_argb_u8(a, r, g, b),
        _ => fallback,
    }
}

/// Pick the first configured font family that is actually installed.
///
/// Slint resolves `font-family` against a single family name, so a
/// comma-separated chain would match nothing; instead the list acts as a
/// priority order. Glyphs missing from the chosen family (e.g. CJK in a Latin
/// mono font) still go through the system font fallback.
pub fn resolve_font_family(families: &[String]) -> Option<String> {
    let mut candidates = families.iter().map(|f| f.trim()).filter(|f| !f.is_empty());
    let first = candidates.next()?.to_string();
    // A single entry needs no lookup: Slint falls back on its own if it is
    // missing, and we skip building a font collection during startup.
    let rest: Vec<&str> = candidates.collect();
    if rest.is_empty() {
        return Some(first);
    }

    let mut collection = fontique::Collection::new(fontique::CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    if collection.family_id(&first).is_some() {
        return Some(first);
    }
    for family in rest {
        if collection.family_id(family).is_some() {
            return Some(family.to_string());
        }
    }
    // Nothing installed: keep the primary choice so the config stays visible in
    // debug output rather than silently switching to the Slint default.
    Some(first)
}

/// Push the configured colours and font onto the window.
pub fn apply_theme(ui: &LauncherWindow, theme: &ThemeConfig) {
    ui.set_theme_bg(parse_fuzzel_color(&theme.background, ui.get_theme_bg()));
    ui.set_theme_text(parse_fuzzel_color(&theme.text, ui.get_theme_text()));
    ui.set_theme_match(parse_fuzzel_color(&theme.match_color, ui.get_theme_match()));
    ui.set_theme_selection_bg(parse_fuzzel_color(
        &theme.selection,
        ui.get_theme_selection_bg(),
    ));
    ui.set_theme_selection_text(parse_fuzzel_color(
        &theme.selection_text,
        ui.get_theme_selection_text(),
    ));
    ui.set_theme_border(parse_fuzzel_color(&theme.border, ui.get_theme_border()));

    if let Some(family) = resolve_font_family(&theme.font) {
        if std::env::var("STOOLS_DEBUG").is_ok() {
            eprintln!("[stools] font-family={family}");
        }
        ui.set_theme_font(SharedString::from(family));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallback() -> Color {
        Color::from_argb_u8(0xff, 0, 0, 0)
    }

    #[test]
    fn parses_fuzzel_rgba() {
        let c = parse_fuzzel_color("282a36dd", fallback());
        assert_eq!(
            (c.red(), c.green(), c.blue(), c.alpha()),
            (0x28, 0x2a, 0x36, 0xdd)
        );
    }

    #[test]
    fn parses_hash_prefix_and_short_forms() {
        let c = parse_fuzzel_color("#f8f8f2", fallback());
        assert_eq!(
            (c.red(), c.green(), c.blue(), c.alpha()),
            (0xf8, 0xf8, 0xf2, 0xff)
        );

        let c = parse_fuzzel_color("#f0a", fallback());
        assert_eq!(
            (c.red(), c.green(), c.blue(), c.alpha()),
            (0xff, 0x00, 0xaa, 0xff)
        );

        let c = parse_fuzzel_color("f0a8", fallback());
        assert_eq!(
            (c.red(), c.green(), c.blue(), c.alpha()),
            (0xff, 0x00, 0xaa, 0x88)
        );
    }

    #[test]
    fn invalid_colors_use_the_fallback() {
        for raw in ["", "nope", "12345", "282a36ddd", "#zzzzzz"] {
            assert_eq!(
                parse_fuzzel_color(raw, fallback()),
                fallback(),
                "input {raw:?}"
            );
        }
    }

    #[test]
    fn font_chain_keeps_the_primary_family() {
        assert_eq!(resolve_font_family(&[]), None);
        assert_eq!(
            resolve_font_family(&["  ".into(), "JetBrains Mono".into()]).as_deref(),
            Some("JetBrains Mono")
        );
        // A single entry is taken verbatim, installed or not.
        assert_eq!(
            resolve_font_family(&["Definitely Not Installed".into()]).as_deref(),
            Some("Definitely Not Installed")
        );
    }
}
