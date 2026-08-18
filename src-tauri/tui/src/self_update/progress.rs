//! Terminal presentation for `ypm update`.
//!
//! Shares the visual language of the Shiori CLI installer — braille spinner,
//! block bar with a bright comet head cooling into the brand colour — but the
//! download phase is determinate because GitHub sends a Content-Length. Every
//! decoration degrades: no TTY prints plain lines, no UTF-8 locale falls back
//! to ASCII cells, no truecolor drops to 256 colours or to plain blue.

use std::future::Future;
use std::io::Write;
use std::time::{Duration, Instant};

use crate::term::{Style, BOLD, BRIGHT, CLEAR_LINE, DIM, RESET};

const BAR_WIDTH: usize = 24;
/// Cells behind the head that stay lit, mirroring the installer's trail.
const TRAIL: usize = 6;
const FRAME: Duration = Duration::from_millis(80);
const REDRAW_INTERVAL: Duration = Duration::from_millis(50);

fn spinner(style: Style, step: usize) -> &'static str {
    if style.unicode {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[step % FRAMES.len()]
    } else {
        const FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
        FRAMES[step % FRAMES.len()]
    }
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * KIB;
    if bytes as f64 >= MIB {
        format!("{:.1} MB", bytes as f64 / MIB)
    } else {
        format!("{:.0} KB", (bytes as f64 / KIB).ceil())
    }
}

/// A single in-place progress line. Naming a different phase settles the
/// previous one into a checkmark row, so callers only describe *what* is
/// happening and never manage the cursor.
pub(crate) struct Reporter {
    style: Style,
    step: usize,
    started: Instant,
    last_draw: Option<Instant>,
    phase: Option<String>,
    detail: String,
}

impl Reporter {
    pub(crate) fn new() -> Self {
        Self {
            style: Style::detect(),
            step: 0,
            started: Instant::now(),
            last_draw: None,
            phase: None,
            detail: String::new(),
        }
    }

    pub(crate) fn style(&self) -> Style {
        self.style
    }

    pub(crate) fn header(&self, current: &str, next: &str) {
        let arrow = if self.style.unicode { "→" } else { "->" };
        println!(
            "\n  {}  {}  {arrow}  {}\n",
            self.style.paint(BOLD, "ypm"),
            self.style.paint(DIM, current),
            self.style
                .paint(&format!("{BOLD}{}", self.style.brand), next),
        );
    }

    /// Reports the live phase. `progress` carries `(downloaded, total)` when
    /// a byte counter exists; `None` keeps the indeterminate comet moving.
    pub(crate) fn phase(&mut self, label: &str, progress: Option<(u64, Option<u64>)>) {
        if self.phase.as_deref() != Some(label) {
            self.settle();
            self.phase = Some(label.to_owned());
            self.step = 0;
            self.started = Instant::now();
            self.last_draw = None;
            self.detail.clear();
            if !self.style.animated {
                println!("  {label} …");
            }
        } else if self
            .last_draw
            .is_some_and(|last| last.elapsed() < REDRAW_INTERVAL)
        {
            return;
        }
        if let Some((_, Some(total))) = progress {
            self.detail = human_bytes(total);
        }
        if self.style.animated {
            self.step += 1;
            self.draw(label, progress);
        }
    }

    /// Closes the live phase with a checkmark row. A no-op when idle.
    pub(crate) fn settle(&mut self) {
        let Some(label) = self.phase.take() else {
            return;
        };
        let detail = std::mem::take(&mut self.detail);
        self.mark(&label, &detail);
    }

    /// A checkmark row for something that took no measurable time.
    pub(crate) fn mark(&mut self, label: &str, detail: &str) {
        let tick = if self.style.unicode { "✓" } else { "ok" };
        let prefix = if self.style.animated { CLEAR_LINE } else { "" };
        println!(
            "{prefix}  {} {label}{}",
            self.style.paint(self.style.brand, tick),
            if detail.is_empty() {
                String::new()
            } else {
                format!("  {}", self.style.paint(DIM, detail))
            }
        );
    }

    pub(crate) fn tail(&self, message: &str) {
        println!("\n  {}\n", self.style.paint(DIM, message));
    }

    /// Drops the live line without a checkmark, so an error starts clean.
    pub(crate) fn abort(&mut self) {
        if self.phase.take().is_some() && self.style.animated {
            print!("{CLEAR_LINE}");
            let _ = std::io::stdout().flush();
        }
    }

    fn draw(&mut self, label: &str, progress: Option<(u64, Option<u64>)>) {
        self.last_draw = Some(Instant::now());
        let (bar, detail) = match progress {
            Some((done, Some(total))) if total > 0 => (
                self.filled_bar(done as f64 / total as f64),
                format!(
                    "{:>3}%  {} / {}{}",
                    (done * 100 / total).min(100),
                    human_bytes(done),
                    human_bytes(total),
                    self.rate(done),
                ),
            ),
            Some((done, _)) => (
                self.comet_bar(),
                format!("{}{}", human_bytes(done), self.rate(done)),
            ),
            None => (self.comet_bar(), String::new()),
        };
        print!(
            "{CLEAR_LINE}  {} {bar}  {label}  {}\r",
            self.style
                .paint(self.style.brand, spinner(self.style, self.step)),
            self.style.paint(DIM, &detail),
        );
        let _ = std::io::stdout().flush();
    }

    fn rate(&self, done: u64) -> String {
        let seconds = self.started.elapsed().as_secs_f64();
        if seconds < 0.4 || done == 0 {
            return String::new();
        }
        let separator = if self.style.unicode { " · " } else { "  " };
        format!(
            "{separator}{}/s",
            human_bytes((done as f64 / seconds) as u64)
        )
    }

    fn cells(&self) -> (&'static str, &'static str) {
        if self.style.unicode {
            ("█", "░")
        } else {
            ("#", "-")
        }
    }

    /// Age from the leading edge: the fill front stays bright white and the
    /// settled body cools into the brand colour.
    fn cell(&self, age: usize) -> String {
        let (full, _) = self.cells();
        match age {
            0 => format!("{BOLD}{BRIGHT}{full}{RESET}"),
            1..=2 => format!("{BOLD}{}{full}{RESET}", self.style.brand),
            _ => format!("{}{full}{RESET}", self.style.brand),
        }
    }

    fn empty_cell(&self) -> String {
        format!("{DIM}{}{RESET}", self.cells().1)
    }

    fn filled_bar(&self, ratio: f64) -> String {
        let filled = ((BAR_WIDTH as f64) * ratio.clamp(0.0, 1.0)).round() as usize;
        (0..BAR_WIDTH)
            .map(|index| match filled.checked_sub(index + 1) {
                Some(age) => self.cell(age),
                None => self.empty_cell(),
            })
            .collect()
    }

    fn comet_bar(&self) -> String {
        let head = self.step % (BAR_WIDTH + TRAIL);
        (0..BAR_WIDTH)
            .map(|index| match head.checked_sub(index) {
                Some(age) if age < TRAIL => self.cell(age),
                _ => self.empty_cell(),
            })
            .collect()
    }
}

/// Runs `future` while the comet keeps moving: phases with no byte counter
/// (release lookup, signature check) still read as alive.
pub(crate) async fn spin<T>(
    reporter: &mut Reporter,
    label: &str,
    future: impl Future<Output = T>,
) -> T {
    reporter.phase(label, None);
    let mut future = std::pin::pin!(future);
    let mut ticker = tokio::time::interval(FRAME);
    ticker.tick().await;
    loop {
        tokio::select! {
            output = &mut future => return output,
            _ = ticker.tick() => {
                reporter.last_draw = None;
                reporter.phase(label, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reporter() -> Reporter {
        Reporter {
            style: Style {
                animated: true,
                unicode: true,
                truecolor: true,
                brand: "\x1b[38;2;51;94;234m",
            },
            step: 0,
            started: Instant::now(),
            last_draw: None,
            phase: None,
            detail: String::new(),
        }
    }

    fn cell_count(bar: &str) -> usize {
        bar.matches('█').count() + bar.matches('░').count()
    }

    #[test]
    fn the_bar_keeps_its_width_at_every_ratio() {
        let reporter = reporter();
        for ratio in [0.0, 0.01, 0.5, 0.999, 1.0, 2.0, -1.0] {
            assert_eq!(
                cell_count(&reporter.filled_bar(ratio)),
                BAR_WIDTH,
                "{ratio}"
            );
        }
    }

    #[test]
    fn the_comet_never_runs_off_the_bar() {
        let mut reporter = reporter();
        for _ in 0..(BAR_WIDTH + TRAIL) * 3 {
            reporter.step += 1;
            assert_eq!(cell_count(&reporter.comet_bar()), BAR_WIDTH);
        }
    }

    #[test]
    fn naming_a_new_phase_settles_the_previous_one() {
        let mut reporter = reporter();
        reporter.phase("下载", Some((10, Some(100))));
        assert_eq!(reporter.detail, "1 KB");
        reporter.phase("校验", None);
        // The download's size is consumed by its checkmark row, so the new
        // phase cannot inherit a stale detail.
        assert_eq!(reporter.phase.as_deref(), Some("校验"));
        assert!(reporter.detail.is_empty());
    }

    #[test]
    fn byte_sizes_never_round_a_live_download_down_to_zero() {
        assert_eq!(human_bytes(512), "1 KB");
        assert_eq!(human_bytes(1024 * 700), "700 KB");
        assert_eq!(human_bytes(7_654_321), "7.3 MB");
    }
}
