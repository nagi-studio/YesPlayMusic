//! The pixel-logo intro.
//!
//! The app icon itself, run through the same renderer that draws cover art,
//! and revealed outward from its centre line: the leading row arrives lit and
//! settles into the artwork's own colours. Terminals that cannot carry it —
//! no TTY, no UTF-8, too few rows — get the one-line wordmark instead, and
//! the caller never has to ask which happened.

use std::io::Write;
use std::time::Duration;

use crossterm::event::{Event, KeyEventKind};
use ratatui::style::Color;

use crate::pixel::{self, CoverDetail, CoverPalette, PixelCover};
use crate::term::{Style, BOLD, CLEAR_SCREEN, DIM, HIDE_CURSOR, HOME, RESET, SHOW_CURSOR};

/// The shipped mark, so the intro can never drift from the app icon.
const MARK: &[u8] = include_bytes!("../../../images/logo.png");

/// Terminal cells are about twice as tall as they are wide, so a square mark
/// needs twice as many columns as rows.
const COLS: u16 = 36;
const ROWS: u16 = 18;
/// Blank line, the mark, blank line, wordmark, version, and a resting line.
const CHROME_ROWS: u16 = 6;
const INDENT: usize = 4;

const STEP: Duration = Duration::from_millis(55);
const SETTLE: Duration = Duration::from_millis(140);
const FLASH: Duration = Duration::from_millis(80);
const SIGN_OFF: Duration = Duration::from_millis(320);
const INPUT_POLL: Duration = Duration::from_millis(16);

/// How far the leading row and the flash pull the artwork toward white.
const LIT: u16 = 150;

fn cover() -> Option<PixelCover> {
    pixel::from_image_bytes(
        MARK,
        CoverPalette::Original,
        &[],
        // Reset keeps the icon's transparent margin as the terminal's own
        // background instead of painting a rectangle around the mark.
        Color::Reset,
        (COLS, ROWS),
        1.0,
        // Half is the only lossless detail level here: one cell carries two
        // sub-pixels and two colours, so nothing is approximated. Quad and
        // finer squeeze four or more sub-pixels into the same two colours,
        // which dithers a smooth gradient into visible noise.
        CoverDetail::Half,
    )
    .ok()
}

/// Distance from the centre line, which is what the reveal expands along.
fn distance(row: u16) -> u16 {
    let middle = ROWS / 2;
    if row < middle {
        middle - 1 - row
    } else {
        row - middle
    }
}

/// Whether the animation fits and the terminal can render it at all.
pub(crate) fn can_animate(style: Style) -> bool {
    if !style.animated || !style.unicode {
        return false;
    }
    crossterm::terminal::size()
        .map(|(cols, rows)| cols >= COLS + INDENT as u16 * 2 && rows >= ROWS + CHROME_ROWS)
        .unwrap_or(false)
}

/// The single line shown when the full mark cannot play.
pub(crate) fn wordmark(style: Style, version: &str) -> String {
    let mark = if style.unicode { "▶" } else { ">" };
    format!(
        "\n  {} {}  {}\n",
        style.paint(style.brand, mark),
        style.paint(BOLD, "ypm"),
        style.paint(DIM, version),
    )
}

/// Plays the intro and polls terminal input between frames so any key can cut
/// it short. Returns `false` when the terminal cannot carry the animation.
pub(crate) async fn play_interactive(style: Style, version: &str) -> std::io::Result<bool> {
    if !can_animate(style) {
        return Ok(false);
    }
    let Some(cover) = cover() else {
        return Ok(false);
    };
    let _raw_mode = RawModeGuard::enter()?;
    let mut terminal = IntroTerminal::enter()?;

    // The mark grows out of its centre line in both directions; the lit
    // leading row is the edge still arriving, the rows behind it have settled.
    let mut cut_short = false;
    for radius in 0..=(ROWS / 2) {
        if poll_keypress()? {
            cut_short = true;
            break;
        }
        draw(style, &cover, radius as i32 - 1, radius as i32, false, None);
        if wait_for_input(STEP).await? {
            cut_short = true;
            break;
        }
    }

    let full = ROWS as i32;
    draw(style, &cover, full, -1, false, None);
    if !cut_short && !poll_keypress()? {
        if wait_for_input(SETTLE).await? {
            cut_short = true;
        } else {
            for flash in [true, false, true] {
                draw(style, &cover, full, -1, flash, None);
                if wait_for_input(FLASH).await? {
                    cut_short = true;
                    break;
                }
            }
        }
    }

    draw(style, &cover, full, -1, false, Some(version));
    if !cut_short && !poll_keypress()? {
        let _ = wait_for_input(SIGN_OFF).await?;
    }
    terminal.restore()?;
    Ok(true)
}

fn poll_keypress() -> std::io::Result<bool> {
    while crossterm::event::poll(Duration::ZERO)? {
        match intro_input(crossterm::event::read()?) {
            Some(IntroInput::Skip) => return Ok(true),
            Some(IntroInput::Interrupt) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "intro interrupted",
                ));
            }
            None => {}
        }
    }
    Ok(false)
}

async fn wait_for_input(duration: Duration) -> std::io::Result<bool> {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        if poll_keypress()? {
            return Ok(true);
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        tokio::time::sleep((deadline - now).min(INPUT_POLL)).await;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntroInput {
    Skip,
    Interrupt,
}

fn intro_input(event: Event) -> Option<IntroInput> {
    let Event::Key(key) = event else { return None };
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.code == crossterm::event::KeyCode::Char('c')
        && key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        Some(IntroInput::Interrupt)
    } else {
        Some(IntroInput::Skip)
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

struct IntroTerminal {
    active: bool,
}

impl IntroTerminal {
    fn enter() -> std::io::Result<Self> {
        let terminal = Self { active: true };
        print!("{HIDE_CURSOR}{CLEAR_SCREEN}");
        std::io::stdout().flush()?;
        Ok(terminal)
    }

    fn restore(&mut self) -> std::io::Result<()> {
        if self.active {
            print!("{SHOW_CURSOR}");
            std::io::stdout().flush()?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for IntroTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn draw(
    style: Style,
    cover: &PixelCover,
    settled: i32,
    active: i32,
    flash: bool,
    version: Option<&str>,
) {
    let pad = " ".repeat(INDENT);
    let mut frame = String::from(HOME);
    frame.push_str("\x1b[K\n");
    for row in 0..ROWS {
        let reach = distance(row) as i32;
        let lit = flash || reach == active;
        if !lit && reach > settled {
            frame.push_str("\x1b[K\n");
            continue;
        }
        frame.push_str(&pad);
        for col in 0..COLS {
            let cell = cover.cells[usize::from(row) * usize::from(cover.width) + usize::from(col)];
            frame.push_str(&paint(style, cell.fg, cell.bg, lit));
            frame.push(cell.glyph);
        }
        frame.push_str(RESET);
        frame.push_str("\x1b[K\n");
    }
    frame.push_str("\x1b[K\n");
    // The wordmark sits under the mark's own centre, not the screen's.
    match version {
        Some(version) => {
            let centre = INDENT + usize::from(COLS) / 2;
            frame.push_str(&format!(
                "{}{}\x1b[K\n",
                " ".repeat(centre - 1),
                style.paint(&format!("{BOLD}{}", style.brand), "ypm")
            ));
            frame.push_str(&format!(
                "{}{}\x1b[K\n",
                " ".repeat(centre.saturating_sub(version.chars().count() / 2)),
                style.paint(DIM, version)
            ));
        }
        None => frame.push_str("\x1b[K\n\x1b[K\n"),
    }
    print!("{frame}");
    let _ = std::io::stdout().flush();
}

fn paint(style: Style, fg: Color, bg: Color, lit: bool) -> String {
    let mut escape = String::new();
    escape.push_str(&colour(style, fg, lit, true));
    escape.push_str(&colour(style, bg, lit, false));
    escape
}

/// `Color::Reset` is the terminal's own default, which is what the icon's
/// transparent margin must stay — never a painted rectangle.
fn colour(style: Style, color: Color, lit: bool, foreground: bool) -> String {
    let (base, reset) = if foreground { (38, 39) } else { (48, 49) };
    match color {
        Color::Rgb(red, green, blue) => {
            let (red, green, blue) = if lit {
                brighten(red, green, blue)
            } else {
                (red, green, blue)
            };
            if style.truecolor {
                format!("\x1b[{base};2;{red};{green};{blue}m")
            } else {
                format!("\x1b[{base};5;{}m", xterm256(red, green, blue))
            }
        }
        Color::Indexed(index) => format!("\x1b[{base};5;{index}m"),
        _ => format!("\x1b[{reset}m"),
    }
}

fn brighten(red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    let mix = |channel: u8| (u16::from(channel) + LIT).min(255) as u8;
    (mix(red), mix(green), mix(blue))
}

/// Standard xterm cube plus grey ramp, for terminals without truecolor.
fn xterm256(red: u8, green: u8, blue: u8) -> u8 {
    if red == green && green == blue {
        return match red {
            0..=7 => 16,
            248..=255 => 231,
            grey => 232 + (u16::from(grey) - 8) as u8 / 10,
        };
    }
    let level = |channel: u8| (u16::from(channel) * 5 / 255) as u8;
    16 + 36 * level(red) + 6 * level(green) + level(blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_icon_renders_at_the_intro_size() {
        let cover = cover().expect("the embedded logo must decode");
        assert_eq!((cover.width, cover.height), (COLS, ROWS));
        assert_eq!(cover.cells.len(), usize::from(COLS) * usize::from(ROWS));
        // A mark that came out entirely transparent would animate as a blank
        // screen; the icon has to actually put ink on the middle row.
        let middle = usize::from(ROWS / 2) * usize::from(COLS);
        assert!(cover.cells[middle..middle + usize::from(COLS)]
            .iter()
            .any(|cell| matches!(cell.fg, Color::Rgb(..)) || matches!(cell.bg, Color::Rgb(..))));
    }

    #[test]
    fn every_row_is_reachable_from_the_centre_line() {
        // A row the reveal can never light would stay blank for the whole
        // animation, leaving a hole in the mark.
        for row in 0..ROWS {
            assert!(distance(row) < ROWS / 2, "row {row}");
        }
    }

    #[test]
    fn the_transparent_margin_stays_the_terminals_own_background() {
        let style = Style {
            animated: true,
            unicode: true,
            truecolor: true,
            brand: "\x1b[38;2;51;94;234m",
        };
        assert_eq!(colour(style, Color::Reset, false, true), "\x1b[39m");
        assert_eq!(colour(style, Color::Reset, false, false), "\x1b[49m");
    }

    #[test]
    fn lighting_a_pixel_moves_it_toward_white_and_stops_there() {
        assert_eq!(brighten(255, 255, 255), (255, 255, 255));
        let (red, ..) = brighten(0, 0, 0);
        assert_eq!(red, LIT as u8);
    }

    #[test]
    fn regular_keys_skip_ctrl_c_interrupts_and_non_keys_do_nothing() {
        assert_eq!(
            intro_input(Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('x'),
                crossterm::event::KeyModifiers::NONE,
            ))),
            Some(IntroInput::Skip)
        );
        assert_eq!(
            intro_input(Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('c'),
                crossterm::event::KeyModifiers::CONTROL,
            ))),
            Some(IntroInput::Interrupt)
        );
        assert_eq!(intro_input(Event::Resize(80, 24)), None);
    }
}
