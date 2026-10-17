use std::{collections::BTreeMap, env, fs, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: String,
    pub surface: String,
    pub raised: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
    pub selection: String,
}

fn hex(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

impl Theme {
    pub fn from_palette(text: &str) -> Self {
        let values: BTreeMap<String, toml::Value> = toml::from_str(text).unwrap_or_default();
        let color = |key: &str, fallback: &str| {
            values
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|v| hex(v))
                .unwrap_or(fallback)
                .to_owned()
        };
        Self {
            background: color("background", "#101218"),
            surface: color("dark_background", "#0c0e13"),
            raised: color("lighter_background", "#1b1e28"),
            foreground: color("foreground", "#e3e5ed"),
            muted: color("muted", "#9a9fac"),
            accent: color("accent", "#a1a7f4"),
            selection: color("selection", "#292e46"),
        }
    }

    pub fn load() -> Self {
        if let Some(path) = env::var_os("TIDAL_THEME_FILE") {
            if let Ok(text) = fs::read_to_string(path) {
                return Self::from_palette(&text);
            }
        }
        let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
        let state = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/state"));
        let config = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        for current in [
            state.join("omarchy/current"),
            config.join("omarchy/current"),
        ] {
            if let Ok(palette) = fs::read_to_string(current.join("theme/colors.toml")) {
                return Self::from_palette(&palette);
            }
        }
        Self::from_palette("")
    }

    pub fn css(&self) -> String {
        let base = include_str!("../ui/style.css");
        format!(
            "@define-color tidal_bg {}; @define-color tidal_surface {}; @define-color tidal_raised {}; @define-color tidal_fg {}; @define-color tidal_muted {}; @define-color tidal_accent {}; @define-color tidal_selection {};\n{}",
            self.background,
            self.surface,
            self.raised,
            self.foreground,
            self.muted,
            self.accent,
            self.selection,
            base
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_uses_palette_and_rejects_css_injection() {
        let t = Theme::from_palette(
            "background = '#123456'\naccent = 'red; }'\nforeground = '#abcdef'",
        );
        assert_eq!(t.background, "#123456");
        assert_eq!(t.foreground, "#abcdef");
        assert_eq!(t.accent, "#a1a7f4");
    }
    #[test]
    fn malformed_palette_has_readable_defaults() {
        let t = Theme::from_palette("not = [toml");
        assert_eq!(t.background, "#101218");
        assert!(t.css().contains("@define-color tidal_fg"));
    }
}
