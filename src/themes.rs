//! Built-in themes adapted from kimi-statusline and CCometixLine.

use crate::config::{
    themes_dir, AnsiColor, ColorConfig, Config, IconConfig, SegmentConfig, SegmentId, StyleConfig,
    StyleMode, TextStyleConfig,
};
use std::collections::BTreeMap;

pub const BUILTIN: [(&str, &str); 10] = [
    ("claude", "Claude: clean text with adaptive colors"),
    ("cometix", "Cometix: bold 16-color with Nerd Font icons"),
    ("default", "Default: 16-color with emoji icons"),
    ("minimal", "Minimal: symbols instead of emoji"),
    ("gruvbox", "Gruvbox colors"),
    ("nord", "Nord colors on backgrounds"),
    ("powerline-dark", "Dark powerline"),
    (
        "powerline-light",
        "Vivid powerline on saturated backgrounds",
    ),
    ("powerline-rose-pine", "Rosé Pine powerline"),
    ("powerline-tokyo-night", "Tokyo Night powerline"),
];

/// (plain, nerd font) icons per segment, CCometixLine's where they overlap.
fn icons(id: SegmentId, minimal: bool) -> (&'static str, &'static str) {
    use SegmentId::*;
    match (id, minimal) {
        (Mode, false) => ("🛡️", "\u{f0483}"),
        (Mode, true) => ("◆", "\u{f0483}"),
        (Cost, false) => ("💰", "\u{f155}"),
        (Cost, true) => ("$", "\u{f155}"),
        (Model, false) => ("🤖", "\u{e26d}"),
        (Model, true) => ("✽", "\u{f2d0}"),
        (OutputStyle, false) => ("✎", "\u{f040}"),
        (OutputStyle, true) => ("✎", "\u{f040}"),
        (Directory, false) => ("📁", "\u{f024b}"),
        (Directory, true) => ("▸", "\u{f024b}"),
        (Git, false) => ("🌿", "\u{f02a2}"),
        (Git, true) => ("※", "\u{f02a2}"),
        (Context, false) => ("🧠", "\u{f49b}"),
        (Context, true) => ("◑", "\u{f49b}"),
        (Usage, false) => ("📊", "\u{f0a9e}"),
        (Usage, true) => ("Σ", "\u{f0a9e}"),
        (Subagent, false) => ("🧩", "\u{f0bc5}"),
        (Subagent, true) => ("⊕", "\u{f0bc5}"),
        (Session, false) => ("⏱️", "\u{f19bb}"),
        (Session, true) => ("◷", "\u{f19bb}"),
        (Quota, false) => ("⏳", "\u{f051f}"),
        (Quota, true) => ("◔", "\u{f051f}"),
        (Changes, false) => ("±", "\u{f440}"),
        (Changes, true) => ("±", "\u{f440}"),
        (Tps, false) => ("⚡", "\u{f04c5}"),
        (Tps, true) => ("↯", "\u{f04c5}"),
    }
}

fn default_options(id: SegmentId) -> BTreeMap<String, toml::Value> {
    let mut o = BTreeMap::new();
    let mut put = |k: &str, v: toml::Value| {
        o.insert(k.to_string(), v);
    };
    match id {
        SegmentId::Directory => put("depth", 3.into()),
        SegmentId::Git => {
            put("status", true.into());
            put("pr", true.into());
            put("pr_link", true.into());
        }
        SegmentId::Context => {
            put("show_tokens", true.into());
            put("bar", false.into());
            put("colorful", true.into());
        }
        SegmentId::Usage => {
            put("colorful", true.into());
            put("show_cache", true.into());
        }
        SegmentId::Subagent => put("colorful", true.into()),
        SegmentId::Tps => {
            put("stale_secs", 300.into());
            put("hide_when_stale", false.into());
        }
        SegmentId::Quota => {
            put("show_5h", true.into());
            put("show_7d", true.into());
            put("show_spend", true.into());
            put("oauth_fallback", false.into());
            put("show_reset", true.into());
            put("bar", false.into());
            put("colorful", true.into());
            put("refresh_secs", 120.into());
        }
        _ => {}
    }
    o
}

struct Spec {
    id: SegmentId,
    enabled: bool,
    icon: Option<AnsiColor>,
    text: Option<AnsiColor>,
    bg: Option<AnsiColor>,
}

fn c16(c: u8) -> Option<AnsiColor> {
    Some(AnsiColor::Color16 { c16: c })
}
fn c256(c: u8) -> Option<AnsiColor> {
    Some(AnsiColor::Color256 { c256: c })
}
fn rgb(r: u8, g: u8, b: u8) -> Option<AnsiColor> {
    Some(AnsiColor::Rgb { r, g, b })
}
fn tok(name: &str) -> Option<AnsiColor> {
    Some(AnsiColor::Named(name.into()))
}

/// Default enablement, shared by every preset.
fn enabled(id: SegmentId, _claude: bool) -> bool {
    !matches!(
        id,
        SegmentId::Session | SegmentId::OutputStyle | SegmentId::Changes
    )
}

fn build(
    name: &str,
    mode: StyleMode,
    separator: &str,
    bold: bool,
    minimal_icons: bool,
    specs: Vec<Spec>,
) -> Config {
    let segments = specs
        .into_iter()
        .map(|s| {
            let (plain, nerd) = icons(s.id, minimal_icons);
            let mut options = default_options(s.id);
            if s.bg.is_some() {
                // fixed ↑/↓ and context colors fight a segment background;
                // let the segment's own text color carry them
                if options.contains_key("colorful") {
                    options.insert("colorful".into(), false.into());
                }
                if s.id == SegmentId::Context {
                    options.insert("colorful".into(), false.into());
                }
            }
            SegmentConfig {
                id: s.id,
                enabled: s.enabled,
                icon: IconConfig {
                    plain: plain.into(),
                    nerd_font: nerd.into(),
                },
                colors: ColorConfig {
                    icon: s.icon,
                    text: s.text,
                    background: s.bg,
                },
                styles: TextStyleConfig { text_bold: bold },
                options,
            }
        })
        .collect();
    Config {
        theme: name.into(),
        style: StyleConfig {
            mode,
            separator: separator.into(),
            separator_color: None,
            palette: String::new(),
            width: 0,
        },
        segments,
    }
}

/// fg-only preset: one (icon, text) pair per segment in SegmentId::ALL order.
fn fg_theme(
    name: &str,
    mode: StyleMode,
    separator: &str,
    bold: bool,
    minimal: bool,
    colors: [(Option<AnsiColor>, Option<AnsiColor>); 13],
) -> Config {
    let specs = SegmentId::ALL
        .into_iter()
        .zip(colors)
        .map(|(id, (icon, text))| Spec {
            id,
            enabled: enabled(id, false),
            icon,
            text,
            bg: None,
        })
        .collect();
    build(name, mode, separator, bold, minimal, specs)
}

type Triple = (u8, u8, u8);

/// powerline preset: one (fg, bg) pair per segment. Backgrounds alternate
/// between two shades along the default enabled order so each arrow shows.
fn pl_theme(name: &str, colors: [(Triple, Triple); 13]) -> Config {
    let specs = SegmentId::ALL
        .into_iter()
        .zip(colors)
        .map(|(id, ((fr, fg, fb), (br, bg, bb)))| Spec {
            id,
            enabled: enabled(id, false),
            icon: rgb(fr, fg, fb),
            text: rgb(fr, fg, fb),
            bg: rgb(br, bg, bb),
        })
        .collect();
    build(name, StyleMode::Powerline, "\u{e0b0}", false, false, specs)
}

fn claude() -> Config {
    // Text-first default, with palette colors shared by the configurator.
    let mut cfg = build(
        "claude",
        StyleMode::Plain,
        "  ",
        false,
        false,
        SegmentId::ALL
            .into_iter()
            .map(|id| Spec {
                id,
                enabled: enabled(id, true),
                icon: tok("text_muted"),
                text: match id {
                    SegmentId::OutputStyle => tok("primary"),
                    SegmentId::Directory | SegmentId::Git => tok("text_dim"),
                    SegmentId::Session => tok("text_muted"),
                    _ => None,
                },
                bg: None,
            })
            .collect(),
    );
    for s in &mut cfg.segments {
        s.icon.plain = match s.id {
            // a divider in front of the usage half of the line
            SegmentId::Usage => "│".into(),
            _ => String::new(),
        };
    }
    cfg
}

pub fn builtin(name: &str) -> Option<Config> {
    // order: mode cost model output_style directory git context usage subagent session quota changes
    Some(match name {
        "claude" => claude(),
        "cometix" | "default" => {
            let pair = |c| (c16(c), c16(c));
            let mut cfg = fg_theme(
                name,
                if name == "cometix" {
                    StyleMode::NerdFont
                } else {
                    StyleMode::Plain
                },
                " | ",
                name == "cometix",
                false,
                [
                    pair(11),
                    pair(5),
                    pair(14),
                    pair(12),
                    (c16(11), c16(10)),
                    pair(12),
                    pair(13),
                    pair(14),
                    pair(6),
                    pair(2),
                    pair(3),
                    pair(10),
                    pair(10),
                ],
            );
            if name == "default" {
                cfg.style.mode = StyleMode::Plain;
            }
            cfg
        }
        "minimal" => {
            let pair = |c| (c16(c), c16(c));
            fg_theme(
                name,
                StyleMode::Plain,
                " │ ",
                false,
                true,
                [
                    pair(11),
                    pair(5),
                    pair(14),
                    pair(12),
                    (c16(11), c16(10)),
                    pair(12),
                    pair(13),
                    pair(14),
                    pair(6),
                    pair(2),
                    pair(3),
                    pair(10),
                    pair(10),
                ],
            )
        }
        "gruvbox" => {
            let pair = |c| (c256(c), c256(c));
            fg_theme(
                name,
                StyleMode::NerdFont,
                " | ",
                true,
                false,
                [
                    pair(167),
                    pair(223),
                    pair(208),
                    pair(214),
                    pair(142),
                    pair(109),
                    pair(175),
                    pair(214),
                    pair(108),
                    pair(142),
                    pair(214),
                    pair(108),
                    pair(108),
                ],
            )
        }
        "nord" => {
            let fg = (46, 52, 64);
            let specs = SegmentId::ALL
                .into_iter()
                .zip([
                    (220, 140, 146),
                    (218, 150, 128),
                    (136, 192, 208),
                    (129, 161, 193),
                    (163, 190, 140),
                    (129, 161, 193),
                    (192, 158, 186),
                    (235, 203, 139),
                    (143, 188, 187),
                    (163, 190, 140),
                    (235, 203, 139),
                    (136, 192, 208),
                    (136, 192, 208),
                ])
                .map(|(id, (r, g, b))| Spec {
                    id,
                    enabled: enabled(id, false),
                    icon: rgb(fg.0, fg.1, fg.2),
                    text: rgb(fg.0, fg.1, fg.2),
                    bg: rgb(r, g, b),
                })
                .collect();
            build(name, StyleMode::Powerline, "\u{e0b0}", false, false, specs)
        }
        "powerline-dark" => pl_theme(
            name,
            [
                ((255, 255, 255), (150, 50, 60)),
                ((229, 192, 123), (40, 42, 48)),
                ((255, 255, 255), (62, 66, 76)),
                ((198, 160, 246), (40, 42, 48)),
                ((171, 178, 191), (40, 42, 48)),
                ((152, 195, 121), (62, 66, 76)),
                ((209, 213, 219), (40, 42, 48)),
                ((125, 190, 245), (62, 66, 76)),
                ((198, 120, 221), (40, 42, 48)),
                ((171, 178, 191), (62, 66, 76)),
                ((229, 192, 123), (62, 66, 76)),
                ((152, 195, 121), (40, 42, 48)),
                ((86, 182, 194), (40, 42, 48)),
            ],
        ),
        "powerline-light" => pl_theme(
            name,
            [
                ((255, 255, 255), (200, 35, 51)),
                ((255, 255, 255), (111, 66, 193)),
                ((0, 0, 0), (135, 206, 235)),
                ((255, 255, 255), (15, 118, 110)),
                ((255, 255, 255), (194, 65, 12)),
                ((255, 255, 255), (3, 105, 161)),
                ((255, 255, 255), (75, 85, 99)),
                ((255, 255, 255), (21, 128, 61)),
                ((0, 0, 0), (250, 204, 21)),
                ((255, 255, 255), (15, 118, 110)),
                ((0, 0, 0), (251, 146, 60)),
                ((255, 255, 255), (21, 128, 61)),
                ((255, 255, 255), (29, 78, 216)),
            ],
        ),
        "powerline-rose-pine" => pl_theme(
            name,
            [
                ((245, 150, 178), (64, 61, 82)),
                ((246, 193, 119), (38, 35, 58)),
                ((235, 188, 186), (64, 61, 82)),
                ((156, 207, 216), (38, 35, 58)),
                ((196, 167, 231), (38, 35, 58)),
                ((156, 207, 216), (64, 61, 82)),
                ((224, 222, 244), (38, 35, 58)),
                ((246, 193, 119), (64, 61, 82)),
                ((235, 188, 186), (38, 35, 58)),
                ((156, 207, 216), (64, 61, 82)),
                ((246, 193, 119), (64, 61, 82)),
                ((156, 207, 216), (38, 35, 58)),
                ((156, 207, 216), (38, 35, 58)),
            ],
        ),
        "powerline-tokyo-night" => pl_theme(
            name,
            [
                ((255, 158, 178), (65, 72, 104)),
                ((255, 158, 100), (41, 46, 66)),
                ((252, 167, 234), (65, 72, 104)),
                ((125, 207, 255), (41, 46, 66)),
                ((130, 170, 255), (41, 46, 66)),
                ((195, 232, 141), (65, 72, 104)),
                ((192, 202, 245), (41, 46, 66)),
                ((232, 190, 128), (65, 72, 104)),
                ((187, 154, 247), (41, 46, 66)),
                ((158, 206, 106), (65, 72, 104)),
                ((232, 190, 128), (65, 72, 104)),
                ((125, 207, 255), (41, 46, 66)),
                ((125, 207, 255), (41, 46, 66)),
            ],
        ),
        _ => return None,
    })
}

/// A saved theme file first (so users can override presets), then built-ins,
/// then `claude`.
pub fn get(name: &str) -> Config {
    load_file(name)
        .or_else(|| builtin(name))
        .unwrap_or_else(|| builtin("claude").expect("claude theme"))
}

fn load_file(name: &str) -> Option<Config> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let text = std::fs::read_to_string(themes_dir().join(format!("{name}.toml"))).ok()?;
    let mut cfg: Config = toml::from_str(&text).ok()?;
    cfg.theme = name.into();
    cfg.add_missing_segments();
    Some(cfg)
}

pub fn save(name: &str, cfg: &Config) -> Result<std::path::PathBuf, String> {
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return Err(format!("invalid theme name {name:?}"));
    }
    let mut cfg = cfg.clone();
    cfg.theme = name.into();
    let path = themes_dir().join(format!("{name}.toml"));
    cfg.write_to(&path)?;
    Ok(path)
}

/// Built-ins followed by saved themes.
pub fn list() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(themes_dir()) {
        let mut extra: Vec<String> = rd
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".toml")
                    .map(str::to_string)
            })
            .filter(|n| !names.contains(n))
            .collect();
        extra.sort();
        names.extend(extra);
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_with_the_removed_lang_key_still_load() {
        let cfg: Config = toml::from_str("[style]\nlang = \"zh\"\nwidth = 90\n").unwrap();
        assert_eq!(cfg.style.width, 90);
    }

    #[test]
    fn old_configs_gain_new_segments_disabled() {
        let mut cfg = builtin("nord").unwrap();
        cfg.segments.retain(|s| s.id != SegmentId::Changes);
        let enabled_before: Vec<_> = cfg
            .segments
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.id)
            .collect();
        cfg.add_missing_segments();
        let changes = cfg.segment(SegmentId::Changes).unwrap();
        assert!(!changes.enabled);
        // styled like the nord preset (powerline background)
        assert!(changes.colors.background.is_some());
        assert_eq!(cfg.segments.last().unwrap().id, SegmentId::Changes);
        let enabled_after: Vec<_> = cfg
            .segments
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.id)
            .collect();
        assert_eq!(enabled_before, enabled_after);
    }

    #[test]
    fn every_builtin_round_trips_through_toml() {
        for (name, _) in BUILTIN {
            let cfg = builtin(name).unwrap();
            assert_eq!(cfg.segments.len(), SegmentId::ALL.len(), "{name}");
            let text = toml::to_string_pretty(&cfg).unwrap();
            let back: Config = toml::from_str(&text).unwrap();
            assert_eq!(back, cfg, "{name}");
        }
    }
}
