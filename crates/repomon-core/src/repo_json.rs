//! Reads a repository's own `repo.json` (<https://github.com/repos-json/repos-json>).
//!
//! A repository committing this file says what it is called and what colour it is, so the same
//! answer travels to every machine that clones it. Everything here is pure: no filesystem, no
//! clock, no network — `parse` takes the file's bytes and `nearest_accent` takes a hex string.

use serde::Deserialize;

/// What repomon takes from a `repo.json`. Deliberately narrower than the specification:
/// facts about the repository, not one person's arrangement of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoJson {
    /// Display name. Applied to `Repo::label`, never to `Repo::name`, because `name` is identity:
    /// notes directories, MCP lookups and worktree paths are all derived from it.
    pub name: Option<String>,
    pub description: Option<String>,
    /// Brand colour resolved to one of the eight `--pane-accent-N` tokens, 1-8.
    pub accent: Option<u8>,
}

#[derive(Deserialize)]
struct RawRepoJson {
    name: Option<String>,
    description: Option<String>,
    color: Option<RawColor>,
}

/// `"color": "#7c3aed"` is shorthand for `{ "primary": "#7c3aed" }` (specification section 6).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawColor {
    Scalar(String),
    Roles { primary: Option<String> },
}

/// The eight `--pane-accent-N` tokens as HSL, mirroring `apps/desktop/src/index.css`. A supplied
/// hex is mapped to the nearest of these rather than used directly: the tokens were chosen to stay
/// legible across all six themes and an arbitrary brand colour was not.
const ACCENT_HSL: [(f64, f64, f64); 8] = [
    (18.0, 0.84, 0.61),
    (196.0, 0.70, 0.48),
    (268.0, 0.60, 0.62),
    (346.0, 0.74, 0.62),
    (42.0, 0.90, 0.50),
    (96.0, 0.55, 0.44),
    (176.0, 0.60, 0.42),
    (226.0, 0.70, 0.60),
];

/// Parse a `repo.json`. Returns `None` when the bytes are not an object, and leaves individual
/// fields `None` when they are absent or unusable, so one bad field never discards the rest.
pub fn parse(text: &str) -> Option<RepoJson> {
    let raw: RawRepoJson = serde_json::from_str(text).ok()?;
    let color = match raw.color {
        Some(RawColor::Scalar(s)) => Some(s),
        Some(RawColor::Roles { primary }) => primary,
        None => None,
    };
    Some(RepoJson {
        name: raw.name.and_then(non_empty),
        description: raw.description.and_then(non_empty),
        accent: color.as_deref().and_then(nearest_accent),
    })
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Map `#rgb` or `#rrggbb` to the nearest accent token, 1-8. Any other syntax yields `None`:
/// the specification admits no other colour form, and guessing at one would make two consumers
/// disagree about the same file.
pub fn nearest_accent(hex: &str) -> Option<u8> {
    let (r, g, b) = parse_hex(hex)?;
    let mut best = (f64::MAX, 0usize);
    for (i, &(h, s, l)) in ACCENT_HSL.iter().enumerate() {
        let d = redmean_sq((r, g, b), hsl_to_rgb(h, s, l));
        if d < best.0 {
            best = (d, i);
        }
    }
    Some(best.1 as u8 + 1)
}

fn parse_hex(hex: &str) -> Option<(f64, f64, f64)> {
    let h = hex.strip_prefix('#')?;
    let byte = |s: &str| u8::from_str_radix(s, 16).ok().map(f64::from);
    match h.len() {
        3 => {
            let d: Vec<char> = h.chars().collect();
            let dup = |c: char| byte(&format!("{c}{c}"));
            Some((dup(d[0])?, dup(d[1])?, dup(d[2])?))
        }
        6 => Some((byte(&h[0..2])?, byte(&h[2..4])?, byte(&h[4..6])?)),
        _ => None,
    }
}

fn hsl_to_rgb(h_deg: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = h_deg / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

/// "Redmean" distance: a cheap approximation of perceived colour difference that, unlike plain RGB
/// distance, does not put a grey next to a saturated hue. Squared, because only the order matters.
fn redmean_sq(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    let rbar = (a.0 + b.0) / 2.0;
    let (dr, dg, db) = (a.0 - b.0, a.1 - b.1, a.2 - b.2);
    (2.0 + rbar / 256.0) * dr * dr + 4.0 * dg * dg + (2.0 + (255.0 - rbar) / 256.0) * db * db
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_field_repomon_uses() {
        let j = parse(r##"{"name":"Acme Platform","description":"The thing","color":"#0f766e"}"##)
            .expect("parses");
        assert_eq!(j.name.as_deref(), Some("Acme Platform"));
        assert_eq!(j.description.as_deref(), Some("The thing"));
        assert_eq!(j.accent, Some(7)); // teal
    }

    #[test]
    fn scalar_colour_is_shorthand_for_primary() {
        let scalar = parse(r##"{"color":"#1d4ed8"}"##).unwrap();
        let roles = parse(r##"{"color":{"primary":"#1d4ed8","accent":"#f59e0b"}}"##).unwrap();
        assert_eq!(scalar.accent, roles.accent);
        assert_eq!(scalar.accent, Some(8)); // blue, not the amber accent role
    }

    #[test]
    fn empty_file_and_unknown_fields_are_not_errors() {
        assert_eq!(parse("{}"), Some(RepoJson::default()));
        let j =
            parse(r##"{"homepage":"https://example.com","projects":[],"name":"Keep"}"##).unwrap();
        assert_eq!(j.name.as_deref(), Some("Keep"));
    }

    #[test]
    fn a_field_that_cannot_be_used_does_not_discard_the_others() {
        let j = parse(r##"{"name":"Kept","color":"rebeccapurple"}"##).unwrap();
        assert_eq!(j.name.as_deref(), Some("Kept"));
        assert_eq!(j.accent, None);
    }

    #[test]
    fn blank_strings_are_absent_not_present_and_empty() {
        let j = parse(r##"{"name":"   ","description":""}"##).unwrap();
        assert_eq!(j.name, None);
        assert_eq!(j.description, None);
    }

    #[test]
    fn not_an_object_is_none() {
        for bad in ["", "[]", "null", "\"x\"", "{", "not json"] {
            assert_eq!(parse(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn three_digit_hex_expands_like_css() {
        assert_eq!(nearest_accent("#0f766e"), nearest_accent("#076"));
    }

    #[test]
    fn every_token_maps_to_itself() {
        // The token colours as hex, so a file that already uses the palette is not moved.
        let tokens = [
            ("#f0793d", 1),
            ("#25a0d0", 2),
            ("#9a63dd", 3),
            ("#e8496e", 4),
            ("#f2a70d", 5),
            ("#6aae32", 6),
            ("#2bab9f", 7),
            ("#5578e6", 8),
        ];
        for (hex, want) in tokens {
            assert_eq!(
                nearest_accent(hex),
                Some(want),
                "{hex} should stay on {want}"
            );
        }
    }

    #[test]
    fn a_colour_is_always_resolved_to_a_token_in_range() {
        for hex in ["#000", "#fff", "#808080", "#ff0000", "#00ff00", "#0000ff"] {
            let a = nearest_accent(hex).unwrap_or_else(|| panic!("{hex} unresolved"));
            assert!((1..=8).contains(&a), "{hex} gave {a}");
        }
    }

    #[test]
    fn malformed_colours_are_rejected_rather_than_guessed() {
        for bad in [
            "",
            "#",
            "#12",
            "#12345",
            "#1234567",
            "0f766e",
            "#gggggg",
            "rgb(1,2,3)",
        ] {
            assert_eq!(nearest_accent(bad), None, "{bad:?} should be rejected");
        }
    }
}
