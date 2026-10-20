//! Palette infrastructure: portable theme identities resolve to semantic colors here.

use ku_session::app::{App, AppConfig, AppError, ThemeCatalog, ThemeDescriptor};
use ratatui::style::Color;
use ratatui_themes::ThemeName;

/// The complete installed palette catalog, including Ku's signature colors.
///
/// # Errors
/// Rejects invalid or duplicate identities in the installed palette library.
pub fn catalog() -> Result<ThemeCatalog, AppError> {
    let mut entries = vec![ThemeDescriptor {
        id: "ku".into(),
        label: "Ku".into(),
    }];
    entries.extend(ThemeName::all().iter().map(|name| ThemeDescriptor {
        id: name.slug().into(),
        label: name.display_name().into(),
    }));
    ThemeCatalog::try_from(entries)
}

/// Installs palette metadata and the selected identity in the immutable genesis.
///
/// # Errors
/// Rejects a theme outside the installed catalog.
pub fn configure(config: AppConfig, theme: &str) -> Result<AppConfig, AppError> {
    config.with_themes(catalog()?, theme)
}

/// Parses a command-line choice to its canonical portable identity.
///
/// # Errors
/// Returns the available identities when the name is unknown.
pub fn parse_name(name: &str) -> Result<String, String> {
    if name == "ku" {
        return Ok(name.into());
    }
    name.parse::<ThemeName>()
        .map(|theme| theme.slug().into())
        .map_err(|_| {
            let choices = std::iter::once("ku")
                .chain(ThemeName::all().iter().map(|theme| theme.slug()))
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown theme {name:?}; choose {choices}")
        })
}

/// Semantic presentation roles shared by every Ku widget.
/// No mutable global or terminal state is needed to resolve or use a palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Accent for active controls and highlights.
    pub accent: Color,
    /// Deemphasized text and secondary metadata.
    pub muted: Color,
    /// Highlight for speculative solver placements.
    pub guess: Color,
    /// Background for errors and invalid state.
    pub alert: Color,
    /// Background for panels and tables.
    pub table: Color,
    /// Background for cells sharing the selected row, column, or box.
    pub peer: Color,
    /// Board and panel separator color.
    pub grid: Color,
    /// Primary text and given-digit color.
    pub ink: Color,
    /// Brand title color.
    pub brand: Color,
    /// Entered value color.
    pub value: Color,
    /// Focused widget text color.
    pub focus: Color,
    /// Completed puzzle and successful result color.
    pub success: Color,
    /// Selected cell background.
    pub selection_bg: Color,
    /// Selected cell foreground.
    pub selection_fg: Color,
    /// Text drawn on the alert background.
    pub error_fg: Color,
}

impl Default for Palette {
    fn default() -> Self {
        // Like the site, resting content is monochrome and interaction reveals
        // the sole accent. Reset lets the terminal own its background and text.
        // Exact identity navy from garrettladley.com/favicon.svg (#2a334f),
        // distinct from the site's CSS link accent.
        let navy = Color::Rgb(0x2a, 0x33, 0x4f);
        Self {
            accent: navy,
            muted: Color::DarkGray,
            guess: Color::Reset,
            alert: Color::Gray,
            table: Color::Reset,
            peer: Color::Reset,
            grid: Color::DarkGray,
            ink: Color::Reset,
            brand: Color::Reset,
            value: Color::Reset,
            focus: Color::Reset,
            success: Color::Reset,
            selection_bg: navy,
            selection_fg: Color::Rgb(245, 244, 239),
            error_fg: Color::Black,
        }
    }
}

impl Palette {
    /// Resolves the session's identity; unknown external palettes use Ku colors.
    /// Replay can still understand such identities without the renderer installed.
    pub fn for_app(app: &App) -> Self {
        Self::for_id(&app.theme().id)
    }

    /// Resolves a palette by its stable identity.
    ///
    /// # Examples
    ///
    /// ```
    /// use ku_cli::tui::theme::{Palette, parse_name};
    ///
    /// assert_eq!(parse_name("ku")?, "ku");
    /// assert!(parse_name("unknown-theme").is_err());
    /// assert_eq!(Palette::for_id("unknown-theme"), Palette::default());
    /// # Ok::<(), String>(())
    /// ```
    pub fn for_id(id: &str) -> Self {
        let Ok(name) = id.parse::<ThemeName>() else {
            return Self::default();
        };
        let colors = name.palette();
        Self {
            accent: colors.accent,
            muted: colors.muted,
            guess: colors.warning,
            alert: colors.error,
            table: colors.bg,
            peer: colors.selection,
            grid: colors.muted,
            ink: colors.fg,
            brand: colors.accent,
            value: colors.accent,
            focus: colors.accent,
            success: colors.success,
            selection_bg: colors.accent,
            selection_fg: contrast_ink(colors.accent),
            error_fg: contrast_ink(colors.error),
        }
    }
}

// Choose the higher-contrast text for saturated selection/error backgrounds,
// independent of whether the overall palette is light or dark.
fn contrast_ink(background: Color) -> Color {
    let Color::Rgb(r, g, b) = background else {
        return Color::Black;
    };
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
    if luminance > 0.179 {
        Color::Rgb(0, 0, 0)
    } else {
        Color::Rgb(255, 255, 255)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            panic!("signature theme must use explicit RGB colors");
        };
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    fn contrast(foreground: Color, background: Color) -> f64 {
        let foreground = luminance(foreground);
        let background = luminance(background);
        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
    }

    #[test]
    fn signature_default_inherits_the_terminal_and_reserves_blue_for_interaction() {
        let palette = Palette::default();
        assert_eq!(palette.table, Color::Reset);
        for color in [
            palette.ink,
            palette.brand,
            palette.value,
            palette.focus,
            palette.guess,
            palette.success,
        ] {
            assert_eq!(color, Color::Reset);
        }
        assert!(contrast(palette.selection_fg, palette.selection_bg) >= 4.5);
        assert_eq!(palette.selection_bg, Color::Rgb(42, 51, 79));
        assert_eq!(palette.accent, palette.selection_bg);
        assert_eq!(palette.error_fg, Color::Black);
        assert_ne!(palette.alert, palette.accent);
        assert_eq!(Palette::for_id("ku"), palette);
    }

    #[test]
    fn every_installed_palette_resolves_and_parses_to_its_canonical_id() {
        for theme in ThemeName::all() {
            assert_eq!(parse_name(theme.slug()).unwrap(), theme.slug());
            assert_ne!(Palette::for_id(theme.slug()), Palette::default());
        }
        assert_eq!(
            catalog().unwrap().as_ref().len(),
            ThemeName::all().len() + 1
        );
        assert!(
            parse_name("missing")
                .unwrap_err()
                .contains("catppuccin-mocha")
        );
    }

    #[test]
    fn selection_and_error_text_choose_contrast_for_each_background() {
        assert_eq!(contrast_ink(Color::Rgb(255, 255, 255)), Color::Rgb(0, 0, 0));
        assert_eq!(contrast_ink(Color::Rgb(0, 0, 0)), Color::Rgb(255, 255, 255));
        for theme in ThemeName::all() {
            let palette = Palette::for_id(theme.slug());
            assert_ne!(palette.selection_bg, palette.selection_fg);
            assert_ne!(palette.alert, palette.error_fg);
            assert_ne!(palette.table, palette.ink);
        }
    }
}
