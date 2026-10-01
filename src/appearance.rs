//! Status line model aliases and independent dark/light/custom palettes.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Models {
    /// alias / provider model / display name -> display name
    pub names: HashMap<String, String>,
}

impl Models {
    pub fn load() -> Self {
        // Optional exact model-id aliases, separate from Claude's own settings.
        std::fs::read_to_string(crate::config::config_dir().join("models.toml"))
            .ok()
            .and_then(|s| toml::from_str::<std::collections::HashMap<String, String>>(&s).ok())
            .map(|names| Self { names })
            .unwrap_or_default()
    }

    /// Display name for any spelling, falling back to the last path segment
    /// (provider/model -> model).
    pub fn display(&self, model: &str) -> String {
        self.names
            .get(model)
            .cloned()
            .unwrap_or_else(|| model_label(model))
    }
}

fn model_label(model: &str) -> String {
    let model = model.rsplit('/').next().unwrap_or(model);
    let Some(id) = model.strip_prefix("claude-") else {
        return model.to_string();
    };
    let base = id.split('[').next().unwrap_or(id);
    let parts: Vec<_> = base.split('-').collect();
    let family = parts.iter().find_map(|part| match *part {
        "sonnet" => Some("Sonnet"),
        "opus" => Some("Opus"),
        "haiku" => Some("Haiku"),
        _ => None,
    });
    let Some(family) = family else {
        return model.to_string();
    };
    let version = parts
        .iter()
        .filter(|p| !p.is_empty() && p.len() <= 2 && p.bytes().all(|b| b.is_ascii_digit()))
        .copied()
        .collect::<Vec<_>>()
        .join(".");
    let suffix = if id.contains("[1m]") { " 1M" } else { "" };
    if version.is_empty() {
        format!("{family}{suffix}")
    } else {
        format!("{family} {version}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_versions_ignore_release_dates_and_preserve_unknown_providers() {
        assert_eq!(model_label("claude-3-5-sonnet-20241022"), "Sonnet 3.5");
        assert_eq!(model_label("claude-opus-4-6[1m]"), "Opus 4.6 1M");
        assert_eq!(model_label("provider/custom-model"), "custom-model");
        assert_eq!(model_label("Opus 4.6"), "Opus 4.6");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(hex: &str) -> Option<Rgb> {
        let h = hex.strip_prefix('#')?;
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Rgb(p(0)?, p(2)?, p(4)?))
    }
}

/// The ColorPalette tokens the footer uses (upstream src/tui/theme/colors.ts).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub text: Rgb,
    pub primary: Rgb,
    pub accent: Rgb,
    pub text_dim: Rgb,
    pub text_muted: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub error: Rgb,
}

pub const DARK: Palette = Palette {
    text: Rgb(0xE0, 0xE0, 0xE0),
    primary: Rgb(0x4F, 0xA8, 0xFF),
    accent: Rgb(0x5B, 0xC0, 0xBE),
    text_dim: Rgb(0x88, 0x88, 0x88),
    text_muted: Rgb(0x6B, 0x6B, 0x6B),
    success: Rgb(0x4E, 0xC8, 0x7E),
    warning: Rgb(0xE8, 0xA8, 0x38),
    error: Rgb(0xE8, 0x54, 0x54),
};

pub const LIGHT: Palette = Palette {
    text: Rgb(0x1A, 0x1A, 0x1A),
    primary: Rgb(0x15, 0x65, 0xC0),
    accent: Rgb(0x00, 0x83, 0x8F),
    text_dim: Rgb(0x45, 0x45, 0x45),
    text_muted: Rgb(0x5F, 0x5F, 0x5F),
    success: Rgb(0x0E, 0x7A, 0x38),
    warning: Rgb(0x92, 0x66, 0x0A),
    error: Rgb(0xB9, 0x1C, 0x1C),
};

impl Palette {
    /// Look a token up by name; both `text_dim` and `textDim` spellings work.
    pub fn token(&self, name: &str) -> Option<Rgb> {
        Some(match name {
            "text" => self.text,
            "primary" => self.primary,
            "accent" => self.accent,
            "text_dim" | "textDim" => self.text_dim,
            "text_muted" | "textMuted" => self.text_muted,
            "success" => self.success,
            "warning" => self.warning,
            "error" => self.error,
            _ => return None,
        })
    }
}

/// Resolve a built-in palette or a JSON file in cc-statusline/palettes/.
pub fn palette(override_theme: Option<&str>) -> Palette {
    let theme = override_theme.map(str::to_string);
    match theme.as_deref() {
        None | Some("auto") | Some("dark") => DARK,
        Some("light") => LIGHT,
        Some(name) => custom_palette(name).unwrap_or(DARK),
    }
}

fn custom_palette(name: &str) -> Option<Palette> {
    let path = crate::config::config_dir()
        .join("palettes")
        .join(format!("{name}.json"));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let mut p = if v.get("base").and_then(|b| b.as_str()) == Some("light") {
        LIGHT
    } else {
        DARK
    };
    if let Some(colors) = v.get("colors").and_then(|c| c.as_object()) {
        let pick = |k: &str| colors.get(k).and_then(|c| c.as_str()).and_then(Rgb::parse);
        for (key, slot) in [
            ("text", &mut p.text),
            ("primary", &mut p.primary),
            ("accent", &mut p.accent),
            ("textDim", &mut p.text_dim),
            ("textMuted", &mut p.text_muted),
            ("success", &mut p.success),
            ("warning", &mut p.warning),
            ("error", &mut p.error),
        ] {
            if let Some(c) = pick(key) {
                *slot = c;
            }
        }
    }
    Some(p)
}
