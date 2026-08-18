//! Terminal capability probing shared by everything ypm draws outside the
//! ratatui loop: the `ypm update` progress line and the pixel-logo intro.
//!
//! Every decoration degrades independently — no TTY drops the escapes, a
//! non-UTF-8 locale drops the block glyphs, and a terminal without truecolor
//! falls back to 256 colours and then to plain blue.

use std::io::IsTerminal;

pub(crate) const RESET: &str = "\x1b[0m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const DIM: &str = "\x1b[2m";
/// Default foreground: reads as the terminal's own "brightest" ink.
pub(crate) const BRIGHT: &str = "\x1b[39m";
pub(crate) const CLEAR_LINE: &str = "\r\x1b[K";
pub(crate) const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
pub(crate) const HOME: &str = "\x1b[H";
pub(crate) const HIDE_CURSOR: &str = "\x1b[?25l";
pub(crate) const SHOW_CURSOR: &str = "\x1b[?25h";

/// YesPlayMusic's `--color-primary`, #335eea.
const BRAND_RGB: &str = "\x1b[38;2;51;94;234m";
const BRAND_256: &str = "\x1b[38;5;63m";
const BRAND_16: &str = "\x1b[34m";

#[derive(Clone, Copy)]
pub(crate) struct Style {
    pub(crate) animated: bool,
    pub(crate) unicode: bool,
    /// Whether 24-bit colour is safe; artwork degrades to the xterm cube.
    pub(crate) truecolor: bool,
    pub(crate) brand: &'static str,
}

impl Style {
    pub(crate) fn detect() -> Self {
        let animated = std::io::stdout().is_terminal()
            && std::env::var("TERM")
                .map(|term| term != "dumb")
                .unwrap_or(true);
        let (truecolor, brand) = brand_colour();
        Self {
            animated,
            unicode: supports_unicode(),
            truecolor,
            brand,
        }
    }

    /// Wraps `text` in `colour`, or returns it bare when escapes would only
    /// end up in a log file.
    pub(crate) fn paint(self, colour: &str, text: &str) -> String {
        if self.animated {
            format!("{colour}{text}{RESET}")
        } else {
            text.to_owned()
        }
    }
}

fn supports_unicode() -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if locale.contains("utf-8") || locale.contains("utf8") {
        return true;
    }
    matches!(
        std::env::var("TERM_PROGRAM").unwrap_or_default().as_str(),
        "Apple_Terminal" | "iTerm.app" | "vscode" | "WezTerm" | "ghostty"
    )
}

fn brand_colour() -> (bool, &'static str) {
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    if colorterm == "truecolor" || colorterm == "24bit" {
        (true, BRAND_RGB)
    } else if std::env::var("TERM")
        .unwrap_or_default()
        .contains("256color")
    {
        (false, BRAND_256)
    } else {
        (false, BRAND_16)
    }
}
