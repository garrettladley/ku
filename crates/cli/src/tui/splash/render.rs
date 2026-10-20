//! A chamfered terminal wordmark using the terminal's own block typography.

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget},
};

use crate::tui::theme::Palette;

const SUBTITLE: &str = "SUDOKU WORKBENCH";
const HINT: &str = "any key to enter · opens automatically";
const LETTER_K: [&str; 7] = [
    " ▄▄     ▄▄",
    " ██   ▄█▀ ",
    " ██ ▄█▀   ",
    " ████▄    ",
    " ██ ▀██▄  ",
    " ██   ▀██▄",
    " ▀▀     ▀▀",
];
const LETTER_U: [&str; 7] = [
    "          ",
    "          ",
    " ▄▄    ▄▄ ",
    " ██    ██ ",
    " ██    ██ ",
    " ██▄▄▄▄██ ",
    "  ▀▀▀▀ ▀▀ ",
];

/// Draws the landing screen without consulting input, state, or the clock.
pub fn render(frame: &mut Frame) {
    render_with_palette(frame, Palette::default());
}

/// Draws the landing screen with an immutable semantic palette.
pub fn render_with_palette(frame: &mut Frame, palette: Palette) {
    frame.render_widget(Splash.with_palette(palette), frame.area());
}

/// A stateless, responsive Ku wordmark usable with any Ratatui backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct Splash;

impl Splash {
    /// Applies the palette used by the rest of the workbench.
    pub fn with_palette(self, palette: Palette) -> ThemedSplash {
        ThemedSplash { palette }
    }
}

impl Widget for Splash {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        self.with_palette(Palette::default()).render(area, buffer);
    }
}

/// A stateless landing widget with explicitly supplied presentation colors.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThemedSplash {
    palette: Palette,
}

impl Widget for ThemedSplash {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let palette = self.palette;
        Clear.render(area, buffer);
        Block::default()
            .style(Style::default().fg(palette.ink).bg(palette.table))
            .render(area, buffer);
        if area.is_empty() {
            return;
        }

        let lines = if area.width >= 40 && area.height >= 15 {
            full_wordmark(palette)
        } else {
            compact_wordmark(area, palette)
        };
        let height = u16::try_from(lines.len())
            .unwrap_or(area.height)
            .min(area.height);
        let content = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(height) / 2,
            area.width,
            height,
        );
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .render(content, buffer);
    }
}

fn full_wordmark(palette: Palette) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(12);
    for (letter_k, letter_u) in LETTER_K.into_iter().zip(LETTER_U) {
        lines.push(Line::from(vec![
            Span::styled(letter_k, Style::default().fg(palette.brand)),
            Span::raw("    "),
            Span::styled(letter_u, Style::default().fg(palette.brand)),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::styled(SUBTITLE, Style::default().fg(palette.ink)));
    lines.push(Line::default());
    lines.push(Line::default());
    lines.push(Line::styled(HINT, Style::default().fg(palette.muted)));
    lines
}

fn compact_wordmark(area: Rect, palette: Palette) -> Vec<Line<'static>> {
    let title = Line::styled(
        "Ku",
        Style::default()
            .fg(palette.brand)
            .add_modifier(Modifier::BOLD),
    );
    if area.height < 3 {
        return vec![title];
    }
    let subtitle = Line::styled(
        if area.width >= 15 { SUBTITLE } else { "SUDOKU" },
        Style::default().fg(palette.ink),
    );
    let hint = Line::styled(
        if area.width >= 37 {
            HINT
        } else if area.width >= 14 {
            "any key / auto"
        } else {
            "any key"
        },
        Style::default().fg(palette.muted),
    );
    if area.height >= 7 {
        vec![title, Line::default(), subtitle, Line::default(), hint]
    } else {
        vec![title, subtitle, hint]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(buffer: &Buffer) -> String {
        let area = buffer.area;
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn large_wordmark_centers_the_logo_and_keeps_the_entry_hint_visible() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 26));
        Splash.render(buffer.area, &mut buffer);
        let rendered = screen(&buffer);

        assert!(rendered.contains("██   ▄█▀"));
        assert!(rendered.contains("██▄▄▄▄██"));
        assert!(rendered.contains(SUBTITLE));
        assert!(rendered.contains(HINT));
        assert_eq!(buffer[(34, 7)].fg, Palette::default().brand);
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.bg == Palette::default().table)
        );
    }

    #[test]
    fn small_terminal_uses_readable_text_instead_of_clipping_the_large_logo() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 8));
        Splash.render(buffer.area, &mut buffer);
        let rendered = screen(&buffer);

        assert!(rendered.contains("Ku"));
        assert!(rendered.contains(SUBTITLE));
        assert!(rendered.contains("any key / auto"));
        assert!(!rendered.contains('█'));
    }

    #[test]
    fn selected_palette_covers_the_canvas_and_styles_large_and_compact_lettering() {
        let palette = Palette::for_id("catppuccin-latte");
        for (width, height, letter) in [(90, 26, (34, 7)), (24, 8, (11, 1))] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
            Splash
                .with_palette(palette)
                .render(buffer.area, &mut buffer);
            assert_eq!(buffer[letter].fg, palette.brand);
            assert!(buffer.content.iter().all(|cell| cell.bg == palette.table));
            assert!(screen(&buffer).contains(SUBTITLE));
        }
    }

    #[test]
    fn empty_and_tiny_viewports_and_nonzero_origins_are_supported() {
        for area in [
            Rect::new(0, 0, 0, 0),
            Rect::new(0, 0, 1, 1),
            Rect::new(4, 5, 40, 15),
        ] {
            let mut buffer = Buffer::empty(area);
            Splash.render(area, &mut buffer);
            if area.width >= 40 {
                assert!(screen(&buffer).contains(SUBTITLE));
            }
        }
    }
}
