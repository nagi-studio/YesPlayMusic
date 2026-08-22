//! The pixel-logo intro.
//!
//! The app icon itself, run through the same renderer that draws cover art.
//! Every intro style builds the same data: a sequence of frames, each a grid
//! of resolved sub-pixel colours plus optional note glyphs. One serializer
//! turns frames into raw ANSI for the startup screen; the settings page
//! renders the same frames through ratatui as an in-app preview, so the two
//! can never drift apart. Terminals that cannot carry it — no TTY, no UTF-8,
//! too few rows — get the one-line wordmark instead, and the caller never
//! has to ask which happened.

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use crossterm::event::{Event, KeyEventKind};
use ratatui::style::Color;

use crate::config::IntroStyle;
use crate::pixel::{self, CoverDetail, CoverPalette, PixelCell, PixelCover};
use crate::term::{Style, BOLD, CLEAR_SCREEN, DIM, HIDE_CURSOR, HOME, RESET, SHOW_CURSOR};

/// The shipped mark, so the intro can never drift from the app icon.
const MARK: &[u8] = include_bytes!("../../../images/logo.png");

/// Terminal cells are about twice as tall as they are wide, so a square mark
/// needs twice as many columns as rows.
const COLS: u16 = 36;
const ROWS: u16 = 18;
/// The mark in sub-pixels: each cell row carries two half-block rows.
const SUB: usize = 36;
/// Blank line, the mark, blank line, wordmark, version, and a resting line.
const CHROME_ROWS: u16 = 6;
/// The narrowest side margin the intro will accept before falling back.
const INDENT: usize = 4;

/// How far a lit sub-pixel is pulled toward white at full brightness.
const LIT: f32 = 150.0;

/// How to render the mark: its square size in sub-pixels plus the pixel-art
/// styling the destination surface uses, so the intro's final frame is the
/// same picture the idle dashboard will keep showing.
#[derive(Clone, Copy)]
pub(crate) struct MarkSpec {
    /// Sub-pixel side length; even, at least 12. Columns = side, rows = side/2.
    pub side: usize,
    pub palette_mode: CoverPalette,
    pub palette: &'static [(u8, u8, u8)],
    pub detail_scale: f32,
    /// What anti-aliased edges blend against. The dashboard blends its idle
    /// art with the theme background; the intro must match or the finished
    /// mark lands with a subtly different rim than the art it becomes.
    pub background: Color,
    /// How the destination surface quantizes a cell. The animation always
    /// works on half-block sub-pixels, but a settled cell is handed to this
    /// detail level so the finished mark is the dashboard's art exactly.
    pub detail: CoverDetail,
}

impl MarkSpec {
    /// The standalone look: full 36×18 cells in the icon's own colours,
    /// used by `ypm update` and anywhere no page follows the intro.
    pub(crate) fn plain() -> Self {
        Self {
            side: SUB,
            palette_mode: CoverPalette::Original,
            palette: &[],
            detail_scale: 1.0,
            // Reset keeps the icon's transparent margin as the terminal's
            // own background instead of painting a rectangle around it.
            background: Color::Reset,
            // A bare updater screen has no dashboard to land on.
            detail: CoverDetail::Half,
        }
    }
}

#[cfg(test)]
fn cover() -> Option<PixelCover> {
    cover_with(&MarkSpec::plain(), COLS, ROWS, CoverDetail::Half)
}

fn cover_with(spec: &MarkSpec, cols: u16, rows: u16, detail: CoverDetail) -> Option<PixelCover> {
    pixel::from_image_bytes(
        MARK,
        spec.palette_mode,
        spec.palette,
        spec.background,
        (cols, rows),
        spec.detail_scale,
        detail,
    )
    .ok()
}

/// The mark as a square sub-pixel grid; `None` is the transparent margin.
struct Mark {
    side: usize,
    px: Vec<Option<(u8, u8, u8)>>,
    /// The finished picture as the destination surface draws it. Settled
    /// cells render from here, so landing on the dashboard changes nothing.
    art: PixelCover,
}

fn mark_pixels(cover: &PixelCover) -> Vec<Option<(u8, u8, u8)>> {
    let w = usize::from(cover.width);
    let h = usize::from(cover.height) * 2;
    let mut px = vec![None; w * h];
    for row in 0..usize::from(cover.height) {
        for col in 0..w {
            let cell = cover.cells[row * w + col];
            // half_cell picks the glyph per cell: '▀' carries the upper
            // sub-pixel in fg, but a top-transparent cell becomes '▄' whose
            // fg is the LOWER sub-pixel. Reading both through '▀' semantics
            // shifts every top-edge pixel up a row — chewed rounded corners.
            let (top, bottom) = match cell.glyph {
                '▄' => (cell.bg, cell.fg),
                _ => (cell.fg, cell.bg),
            };
            if let Color::Rgb(r, g, b) = top {
                px[row * 2 * w + col] = Some((r, g, b));
            }
            if let Color::Rgb(r, g, b) = bottom {
                px[(row * 2 + 1) * w + col] = Some((r, g, b));
            }
        }
    }
    px
}

fn mark(spec: &MarkSpec) -> Option<Mark> {
    let side = spec.side.max(12) & !1;
    let (cols, rows) = (side as u16, (side / 2) as u16);
    // Half is the only lossless level for the animation grid: one cell
    // carries two sub-pixels and two colours, so nothing is approximated
    // and every style can shade a sub-pixel on its own.
    let mut px = mark_pixels(&cover_with(spec, cols, rows, CoverDetail::Half)?);
    // The render flattens the icon's transparent margin into the background
    // colour — indistinguishable from painted pixels, so on RGB themes every
    // animation would treat the whole square as ink and fly background-
    // coloured tiles around. The margin comes back as exactly the background
    // (render_sample passes it through untouched, no dither), while blended
    // anti-aliased edges always carry an offset — so colour equality IS the
    // transparency test. A real pixel that happens to match the background
    // is invisible either way; skipping its animation changes nothing.
    if let Color::Rgb(r, g, b) = spec.background {
        for pixel in &mut px {
            if *pixel == Some((r, g, b)) {
                *pixel = None;
            }
        }
    }
    Some(Mark {
        side,
        px,
        art: cover_with(spec, cols, rows, spec.detail)?,
    })
}

/// Distance of a sub-pixel row from the mark's centre line, which is what
/// the centre-out reveals expand along.
fn reach(rows: i32, y: usize) -> i32 {
    let row = (y / 2) as i32;
    let middle = rows / 2;
    if row < middle {
        middle - 1 - row
    } else {
        row - middle
    }
}

/// Whether any full-mark intro fits and the terminal can render it at all.
/// Styles that want extra flight room shrink it to fit instead of failing,
/// so the minimum is the same for every style.
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

/* ---------------- frame model ---------------- */

/// A note glyph floating over the grid, in whole-cell coordinates.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoteGlyph {
    pub col: u16,
    pub row: u16,
    pub glyph: char,
    pub rgb: (u8, u8, u8),
}

/// One fully-resolved animation frame.
pub(crate) struct IntroFrame {
    pub dur: Duration,
    /// Wordmark opacity, 0..1.
    pub wm: f32,
    /// Row-major sub-pixel colours, `sub_w × sub_h`; `None` stays background.
    pub grid: Vec<Option<(u8, u8, u8)>>,
    pub notes: Vec<NoteGlyph>,
}

pub(crate) struct IntroSequence {
    /// Viewport width in columns (= sub-pixels).
    pub sub_w: usize,
    /// Viewport height in sub-pixels; always even.
    pub sub_h: usize,
    /// The mark's square side in sub-pixels, centred in the viewport.
    pub mark_side: usize,
    pub frames: Vec<IntroFrame>,
    /// The finished picture as the destination surface draws it, and the
    /// cell it starts at. Cells that already hold their final colours are
    /// composed from this instead of half blocks.
    art: PixelCover,
    art_at: (usize, usize),
}

impl IntroSequence {
    pub(crate) fn total(&self) -> Duration {
        self.frames.iter().map(|frame| frame.dur).sum()
    }
}

/// What one terminal cell of a frame resolves to.
pub(crate) enum ComposedCell {
    Empty,
    Pixels {
        top: Option<(u8, u8, u8)>,
        bottom: Option<(u8, u8, u8)>,
    },
    /// A settled cell, drawn by the destination's own pixel renderer.
    Art {
        glyph: char,
        fg: Color,
        bg: Color,
    },
    Note {
        glyph: char,
        rgb: (u8, u8, u8),
        under: Option<(u8, u8, u8)>,
    },
}

type Rgb8 = (u8, u8, u8);

/// Per-frame cell composer shared by the ANSI printer and the preview.
pub(crate) struct FrameCells<'a> {
    frame: &'a IntroFrame,
    sub_w: usize,
    notes: HashMap<(u16, u16), (char, Rgb8)>,
    art: &'a PixelCover,
    art_at: (usize, usize),
    /// The last frame's grid: a cell matching it here is finished.
    finished: &'a [Option<Rgb8>],
}

impl<'a> FrameCells<'a> {
    pub(crate) fn new(seq: &'a IntroSequence, frame: &'a IntroFrame) -> Self {
        let mut notes = HashMap::new();
        // Later notes draw over earlier ones, matching insertion order.
        for note in &frame.notes {
            notes.insert((note.col, note.row), (note.glyph, note.rgb));
        }
        Self {
            frame,
            sub_w: seq.sub_w,
            notes,
            art: &seq.art,
            art_at: seq.art_at,
            finished: seq.frames.last().map_or(&[], |last| last.grid.as_slice()),
        }
    }

    pub(crate) fn cell(&self, col: usize, row: usize) -> ComposedCell {
        let top = self.frame.grid[row * 2 * self.sub_w + col];
        let bottom = self.frame.grid[(row * 2 + 1) * self.sub_w + col];
        if let Some(&(glyph, rgb)) = self.notes.get(&(col as u16, row as u16)) {
            return ComposedCell::Note {
                glyph,
                rgb,
                under: bottom,
            };
        }
        if top.is_none() && bottom.is_none() {
            // A settled-empty cell can still carry faint art: the margin
            // mask decides on the Half grid while the destination samples
            // its own detail grid, and a hair-above-background corner the
            // two disagree on (float rounding is even platform-dependent)
            // would otherwise flip in on the handover frame. Deferring to
            // the art everywhere the animation is done makes the final
            // frame the art by construction.
            if let Some(cell) = self.settled(col, row, None, None) {
                return ComposedCell::Art {
                    glyph: cell.glyph,
                    fg: cell.fg,
                    bg: cell.bg,
                };
            }
            return ComposedCell::Empty;
        }
        // Hand a finished cell to the renderer the destination surface uses.
        // Below half blocks that picks a different glyph for the same
        // colours, and deferring the switch to the handover would snap the
        // whole mark into sharper focus in a single frame.
        if let Some(cell) = self.settled(col, row, top, bottom) {
            return ComposedCell::Art {
                glyph: cell.glyph,
                fg: cell.fg,
                bg: cell.bg,
            };
        }
        ComposedCell::Pixels { top, bottom }
    }

    /// The destination's own cell, once this one holds its final colours.
    fn settled(
        &self,
        col: usize,
        row: usize,
        top: Option<Rgb8>,
        bottom: Option<Rgb8>,
    ) -> Option<PixelCell> {
        let upper = row * 2 * self.sub_w + col;
        if *self.finished.get(upper)? != top || *self.finished.get(upper + self.sub_w)? != bottom {
            return None;
        }
        let art_col = col.checked_sub(self.art_at.0)?;
        let art_row = row.checked_sub(self.art_at.1)?;
        let width = usize::from(self.art.width);
        if art_col >= width || art_row >= usize::from(self.art.height) {
            return None;
        }
        self.art.cells.get(art_row * width + art_col).copied()
    }
}

#[derive(Clone, Copy)]
struct CellState {
    mix: f32,
    lit: f32,
}

const ON: CellState = CellState { mix: 1.0, lit: 0.0 };

/// Resolve one sub-pixel: fade from `bg`, then pull toward white.
fn shade(base: (u8, u8, u8), bg: (u8, u8, u8), state: CellState) -> (u8, u8, u8) {
    let mix = state.mix.clamp(0.0, 1.0);
    let lift = state.lit.clamp(0.0, 1.0) * LIT;
    let channel = |b: u8, g: u8| -> u8 {
        (f32::from(g) + (f32::from(b) - f32::from(g)) * mix + lift)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (
        channel(base.0, bg.0),
        channel(base.1, bg.1),
        channel(base.2, bg.2),
    )
}

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

/// Build a square frame from a per-sub-pixel state function.
fn frame_from<F>(mark: &Mark, bg: (u8, u8, u8), dur: u64, wm: f32, state: F) -> IntroFrame
where
    F: Fn(usize, usize) -> Option<CellState>,
{
    let side = mark.side;
    let mut grid = vec![None; side * side];
    for y in 0..side {
        for x in 0..side {
            if let Some(base) = mark.px[y * side + x] {
                if let Some(cell) = state(x, y) {
                    if cell.mix > 0.02 {
                        grid[y * side + x] = Some(shade(base, bg, cell));
                    }
                }
            }
        }
    }
    IntroFrame {
        dur: ms(dur),
        wm,
        grid,
        notes: Vec::new(),
    }
}

fn full(mark: &Mark, bg: (u8, u8, u8), dur: u64, wm: f32) -> IntroFrame {
    frame_from(mark, bg, dur, wm, |_, _| Some(ON))
}

fn seq_square(mark: &Mark, frames: Vec<IntroFrame>) -> IntroSequence {
    IntroSequence {
        sub_w: mark.side,
        sub_h: mark.side,
        mark_side: mark.side,
        frames,
        art: mark.art.clone(),
        art_at: (0, 0),
    }
}

/* ---------------- deterministic helpers ---------------- */

/// The demo page's LCG, kept deterministic so every launch looks the same.
fn chash(n: u32) -> f32 {
    let mut s = n.wrapping_add(13).wrapping_mul(374_761_393);
    s = (s ^ (s >> 13)).wrapping_mul(1_274_126_177);
    ((s >> 8) & 0x00FF_FFFF) as f32 / 16_777_216.0
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
    }
}

fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn reveal_cell(rows: i32, y: usize, settled: i32, active: i32, glow: f32) -> Option<CellState> {
    let d = reach(rows, y);
    if d == active {
        Some(CellState { mix: 1.0, lit: 1.0 })
    } else if glow > 0.0 && d == active - 1 {
        Some(CellState {
            mix: 1.0,
            lit: glow,
        })
    } else if d <= settled {
        Some(ON)
    } else {
        None
    }
}

/* ---------------- the twelve styles ---------------- */

fn classic(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let rows = (mark.side / 2) as i32;
    let mut frames = Vec::new();
    for r in 0..=rows / 2 {
        frames.push(frame_from(mark, bg, 55, 0.0, |_, y| {
            reveal_cell(rows, y, r - 1, r, 0.0)
        }));
    }
    frames.push(full(mark, bg, 140, 0.0));
    for flash in [1.0, 0.0, 1.0] {
        frames.push(frame_from(mark, bg, 80, 0.0, |_, _| {
            Some(CellState {
                mix: 1.0,
                lit: flash,
            })
        }));
    }
    frames.push(full(mark, bg, 900, 1.0));
    seq_square(mark, frames)
}

fn pulse(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let rows = (mark.side / 2) as i32;
    let half = rows / 2;
    let mut frames = Vec::new();
    for r in 0..=half {
        let dur = (38.0 + (r as f32 / half as f32).powf(1.6) * 85.0) as u64;
        frames.push(frame_from(mark, bg, dur, 0.0, |_, y| {
            reveal_cell(rows, y, r - 1, r, 0.35)
        }));
    }
    let decay = [0.6, 0.38, 0.2, 0.08, 0.0];
    let wm_ramp = [0.0, 0.25, 0.52, 0.78, 1.0];
    for (lit, wm) in decay.into_iter().zip(wm_ramp) {
        frames.push(frame_from(mark, bg, 42, wm, move |_, _| {
            Some(CellState { mix: 1.0, lit })
        }));
    }
    frames.push(full(mark, bg, 800, 1.0));
    seq_square(mark, frames)
}

fn scan(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let rows = (mark.side / 2) as i32;
    let beam = |b: i32| {
        move |_: usize, y: usize| -> Option<CellState> {
            let cr = (y / 2) as i32;
            if cr > b {
                return None;
            }
            let lit = match b - cr {
                0 => 0.93,
                1 => 0.45,
                2 => 0.16,
                _ => 0.0,
            };
            Some(CellState { mix: 1.0, lit })
        }
    };
    let mut frames = Vec::new();
    for b in 0..rows {
        frames.push(frame_from(mark, bg, 38, 0.0, beam(b)));
    }
    frames.push(frame_from(mark, bg, 45, 0.0, beam(rows + 1)));
    frames.push(full(mark, bg, 45, 0.0));
    for wm in [0.3, 0.62, 0.86, 1.0] {
        frames.push(full(mark, bg, 55, wm));
    }
    frames.push(full(mark, bg, 750, 1.0));
    seq_square(mark, frames)
}

fn ripple(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let centre = (mark.side as f32 - 1.0) / 2.0;
    let dist = |x: usize, y: usize| (x as f32 - centre).hypot(y as f32 - centre);
    let dmax = centre.hypot(centre) + 0.5;
    let mut frames = Vec::new();
    let steps = 16;
    for f in 1..=steps {
        let t = f as f32 / steps as f32;
        let radius = dmax * (1.0 - (1.0 - t).powf(2.2));
        frames.push(frame_from(mark, bg, 45, 0.0, move |x, y| {
            let d = dist(x, y);
            if d > radius {
                return None;
            }
            let lit = (1.0 - (d - radius).abs() / 2.4).max(0.0) * 0.95;
            Some(CellState { mix: 1.0, lit })
        }));
    }
    for (residual, wm) in [(0.35, 0.0), (0.12, 0.4)] {
        frames.push(frame_from(mark, bg, 45, wm, move |x, y| {
            let lit = (1.0 - (dmax - dist(x, y)) / 2.4).max(0.0) * residual;
            Some(CellState { mix: 1.0, lit })
        }));
    }
    for wm in [0.75, 1.0] {
        frames.push(full(mark, bg, 55, wm));
    }
    frames.push(full(mark, bg, 750, 1.0));
    seq_square(mark, frames)
}

fn dissolve(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let side = mark.side;
    let mut thresholds = vec![0.0f32; side * side];
    for (index, slot) in thresholds.iter_mut().enumerate() {
        let (x, y) = ((index % side) as u32, (index / side) as u32);
        let mut s = (x + 1)
            .wrapping_mul(374_761_393)
            .wrapping_add((y + 1).wrapping_mul(668_265_263));
        s = (s ^ (s >> 13)).wrapping_mul(1_274_126_177);
        *slot = ((s >> 8) & 0x00FF_FFFF) as f32 / 16_777_216.0;
    }
    let mut frames = Vec::new();
    let steps = 24;
    for f in 1..=steps {
        let p = ease_in_out(f as f32 / steps as f32);
        let thresholds = &thresholds;
        frames.push(frame_from(mark, bg, 33, 0.0, move |x, y| {
            let h = thresholds[y * side + x];
            if h >= p {
                return None;
            }
            let lit = (1.0 - (p - h) * 5.5).clamp(0.0, 1.0) * 0.9;
            Some(CellState { mix: 1.0, lit })
        }));
    }
    frames.push(frame_from(mark, bg, 40, 0.0, |_, _| {
        Some(CellState {
            mix: 1.0,
            lit: 0.12,
        })
    }));
    frames.push(full(mark, bg, 40, 0.4));
    for wm in [0.75, 1.0] {
        frames.push(full(mark, bg, 55, wm));
    }
    frames.push(full(mark, bg, 750, 1.0));
    seq_square(mark, frames)
}

fn breath(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let mut frames = Vec::new();
    let steps = 16;
    for f in 1..=steps {
        let mix = ease_out_cubic(f as f32 / steps as f32);
        let wm = ((mix - 0.45) / 0.55).clamp(0.0, 1.0);
        frames.push(frame_from(mark, bg, 42, wm, move |_, _| {
            Some(CellState { mix, lit: 0.0 })
        }));
    }
    for lit in [0.22, 0.11, 0.04, 0.0] {
        frames.push(frame_from(mark, bg, 45, 1.0, move |_, _| {
            Some(CellState { mix: 1.0, lit })
        }));
    }
    frames.push(full(mark, bg, 750, 1.0));
    seq_square(mark, frames)
}

fn spectrum(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let side = mark.side;
    let mut phase = vec![0.0f32; side];
    for (col, slot) in phase.iter_mut().enumerate() {
        *slot = chash(col as u32) * 0.34;
    }
    let mut frames = Vec::new();
    let steps = 22;
    for f in 1..=steps {
        let t = f as f32 / steps as f32;
        let phase = &phase;
        frames.push(frame_from(mark, bg, 38, 0.0, move |x, y| {
            let p = ((t - phase[x]) / 0.66).clamp(0.0, 1.0);
            if p <= 0.0 {
                return None;
            }
            let top = side as f32 - side as f32 * ease_out_back(p);
            if (y as f32) < top {
                return None;
            }
            let lit = if p < 1.0 && (y as f32 - top) < 2.0 {
                0.85 * (1.0 - p * 0.5)
            } else {
                0.0
            };
            Some(CellState { mix: 1.0, lit })
        }));
    }
    for wm in [0.4, 0.75, 1.0] {
        frames.push(full(mark, bg, 55, wm));
    }
    frames.push(full(mark, bg, 750, 1.0));
    seq_square(mark, frames)
}

fn vinyl(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let centre = (mark.side as f32 - 1.0) / 2.0;
    let angle = |x: usize, y: usize| -> f32 {
        let a = (x as f32 - centre).atan2(-(y as f32 - centre));
        if a < 0.0 {
            a + std::f32::consts::TAU
        } else {
            a
        }
    };
    let mut frames = Vec::new();
    let steps = 20;
    for f in 1..=steps {
        let theta = std::f32::consts::TAU * ease_in_out(f as f32 / steps as f32) + 0.001;
        frames.push(frame_from(mark, bg, 40, 0.0, move |x, y| {
            let a = angle(x, y);
            if a > theta {
                return None;
            }
            let lit = (1.0 - (theta - a) / 0.55).max(0.0) * 0.9;
            Some(CellState { mix: 1.0, lit })
        }));
    }
    frames.push(frame_from(mark, bg, 45, 0.4, |_, _| {
        Some(CellState { mix: 1.0, lit: 0.1 })
    }));
    for wm in [0.75, 1.0] {
        frames.push(full(mark, bg, 55, wm));
    }
    frames.push(full(mark, bg, 750, 1.0));
    seq_square(mark, frames)
}

fn shimmer(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let mut frames = Vec::new();
    for f in 1..=6 {
        let mix = 0.75 * ease_out_cubic(f as f32 / 6.0);
        frames.push(frame_from(mark, bg, 36, 0.0, move |_, _| {
            Some(CellState { mix, lit: 0.0 })
        }));
    }
    let edge = mark.side as f32 - 1.0;
    let span = edge + edge * 0.7 + 12.0;
    let steps = 12;
    for g in 1..=steps {
        let p = -6.0 + span * ease_in_out(g as f32 / steps as f32);
        let wm = ((g as f32 - 6.0) / 6.0).clamp(0.0, 1.0);
        frames.push(frame_from(mark, bg, 38, wm, move |x, y| {
            let u = x as f32 + y as f32 * 0.7;
            let mix = 0.75 + 0.25 * ((p - u) / 6.0).clamp(0.0, 1.0);
            let lit = (1.0 - (u - p).abs() / 5.0).max(0.0).powf(1.6) * 0.8;
            Some(CellState { mix, lit })
        }));
    }
    frames.push(full(mark, bg, 800, 1.0));
    seq_square(mark, frames)
}

fn rain(mark: &Mark, bg: (u8, u8, u8)) -> IntroSequence {
    let side = mark.side;
    let mut delay = vec![0.0f32; side];
    for (col, slot) in delay.iter_mut().enumerate() {
        *slot = chash(col as u32 * 7 + 3) * 8.0;
    }
    let mut frames = Vec::new();
    // Long enough for the slowest column to finish its fall.
    let steps = (8.0 + side as f32 / 2.6).ceil() as i32 + 1;
    for f in 1..=steps {
        let delay = &delay;
        frames.push(frame_from(mark, bg, 33, 0.0, move |x, y| {
            let head = (f as f32 - delay[x]) * 2.6;
            if head < y as f32 {
                return None;
            }
            let d = head - y as f32;
            let lit = if d < 1.4 {
                0.9
            } else if d < 4.0 {
                0.3 * (1.0 - (d - 1.4) / 2.6)
            } else {
                0.0
            };
            Some(CellState { mix: 1.0, lit })
        }));
    }
    for wm in [0.4, 0.75, 1.0] {
        frames.push(full(mark, bg, 55, wm));
    }
    frames.push(full(mark, bg, 750, 1.0));
    seq_square(mark, frames)
}

fn zoom(mark: &Mark, bg: (u8, u8, u8), spec: &MarkSpec) -> IntroSequence {
    let full_side = mark.side;
    let mut frames = Vec::new();
    // Each step is a genuine re-render of the mark at a smaller cell count,
    // exactly what the terminal build does with pixel::from_image_bytes.
    let steps: [(usize, u64); 6] = [(6, 42), (10, 42), (14, 46), (20, 52), (26, 62), (30, 80)];
    for (side, dur) in steps {
        if side >= full_side {
            continue;
        }
        let Some(small) = cover_with(spec, side as u16, (side / 2) as u16, CoverDetail::Half)
        else {
            continue;
        };
        let px = mark_pixels(&small);
        let offset = (full_side - side) / 2;
        let mut grid = vec![None; full_side * full_side];
        for y in 0..side {
            for x in 0..side {
                grid[(y + offset) * full_side + (x + offset)] = px[y * side + x];
            }
        }
        frames.push(IntroFrame {
            dur: ms(dur),
            wm: 0.0,
            grid,
            notes: Vec::new(),
        });
    }
    frames.push(frame_from(mark, bg, 45, 0.0, |_, _| {
        Some(CellState {
            mix: 1.0,
            lit: 0.22,
        })
    }));
    frames.push(frame_from(mark, bg, 45, 0.4, |_, _| {
        Some(CellState {
            mix: 1.0,
            lit: 0.09,
        })
    }));
    frames.push(full(mark, bg, 50, 0.75));
    frames.push(full(mark, bg, 800, 1.0));
    seq_square(mark, frames)
}

/* ---------------- notes: the jigsaw finale ---------------- */

const TILE: usize = 6;
const NOTE_GLYPHS: [char; 4] = ['♪', '♫', '♩', '♬'];

struct Note {
    tx: f32,
    ty: f32,
    sx: f32,
    sy: f32,
    amp: f32,
    phz: f32,
    f0: i32,
    fl: i32,
    arrive: i32,
    glyph: char,
    rgb: (u8, u8, u8),
}

fn note_pos(note: &Note, f: i32) -> (f32, f32, f32) {
    let p = (f - note.f0) as f32 / note.fl as f32;
    let e = ease_out_cubic(p);
    let x = note.sx
        + (note.tx - note.sx) * e
        + (p * std::f32::consts::PI * 2.2 + note.phz).sin() * (1.0 - p) * note.amp;
    let y = note.sy + (note.ty - note.sy) * e;
    (p, x, y)
}

/// Notes rise like bubbles, each owning one jigsaw piece; the finished mark
/// gets a single factory-fresh gleam.
fn notes(mark: &Mark, bg: (u8, u8, u8), pad_x: usize, pad_y: usize) -> IntroSequence {
    let sub = mark.side;
    let sub_w = sub + pad_x * 2;
    let sub_h = sub + pad_y * 2;
    let tiles_axis = sub.div_ceil(TILE);

    // One note per tile that has any ink, coloured by the tile's first
    // inked pixel; edge tiles may run smaller when the mark's side is not
    // a multiple of the tile size.
    let mut tiles = Vec::new();
    for ty in 0..tiles_axis {
        for tx in 0..tiles_axis {
            let found = (ty * TILE..((ty + 1) * TILE).min(sub)).find_map(|y| {
                (tx * TILE..((tx + 1) * TILE).min(sub)).find_map(|x| mark.px[y * sub + x])
            });
            if let Some(rgb) = found {
                tiles.push((tx, ty, rgb));
            }
        }
    }
    tiles.sort_by(|a, b| {
        chash((a.0 * 31 + a.1 * 57) as u32).total_cmp(&chash((b.0 * 31 + b.1 * 57) as u32))
    });

    let mut notes = Vec::new();
    let mut chord = 0i32;
    for (k, &(tx, ty, rgb)) in tiles.iter().enumerate() {
        let k32 = k as u32;
        let f0 = (chash(k32 * 577 + 3) * 18.0) as i32;
        let fl = 15 + (chash(k32 * 271 + 5) * 8.0) as i32;
        let col_x = (tx * TILE + TILE / 2) as f32;
        let note = Note {
            tx: col_x,
            ty: (ty * TILE + TILE / 2) as f32 / 2.0,
            sx: col_x + (chash(k32 * 911 + 7) - 0.5) * 26.0,
            // Rising entry: notes start just under the mark's bottom row.
            sy: (sub / 2) as f32 + 4.0 + chash(k32 * 131 + 11) * 4.0,
            amp: 2.0 + chash(k32 * 419 + 29) * 2.2,
            phz: chash(k32 * 613 + 41) * std::f32::consts::TAU,
            f0,
            fl,
            arrive: f0 + fl,
            glyph: NOTE_GLYPHS[k % NOTE_GLYPHS.len()],
            rgb,
        };
        chord = chord.max(note.arrive);
        notes.push(note);
    }

    // Jigsaw seams: every interior edge grows a 2-sub-pixel knob to one
    // side, so each piece owns its tabs and lacks its blanks. Knobs that
    // would poke past a ragged edge tile are simply skipped.
    let mut owner = vec![0usize; sub * sub];
    for (index, slot) in owner.iter_mut().enumerate() {
        let (x, y) = (index % sub, index / sub);
        *slot = (y / TILE) * tiles_axis + x / TILE;
    }
    for ty in 0..tiles_axis {
        for tx in 0..tiles_axis - 1 {
            let to_right = chash((tx * 97 + ty * 53 + 5) as u32) > 0.5;
            let xe = if to_right {
                (tx + 1) * TILE
            } else {
                (tx + 1) * TILE - 1
            };
            let id = ty * tiles_axis + if to_right { tx } else { tx + 1 };
            let mid = ty * TILE + 2;
            if mid + 1 < sub {
                owner[mid * sub + xe] = id;
                owner[(mid + 1) * sub + xe] = id;
            }
        }
    }
    for ty in 0..tiles_axis - 1 {
        for tx in 0..tiles_axis {
            let to_down = chash((tx * 71 + ty * 89 + 9) as u32) > 0.5;
            let ye = if to_down {
                (ty + 1) * TILE
            } else {
                (ty + 1) * TILE - 1
            };
            let id = (if to_down { ty } else { ty + 1 }) * tiles_axis + tx;
            let mid = tx * TILE + 2;
            if mid + 1 < sub {
                owner[ye * sub + mid] = id;
                owner[ye * sub + mid + 1] = id;
            }
        }
    }
    let mut arrive_by_tile = vec![chord; tiles_axis * tiles_axis];
    for (k, &(tx, ty, _)) in tiles.iter().enumerate() {
        arrive_by_tile[ty * tiles_axis + tx] = notes[k].arrive;
    }
    let reveal: Vec<i32> = owner.iter().map(|&id| arrive_by_tile[id]).collect();

    let gleam_start = chord + 3;
    let gleam_len = 9;
    let edge = sub as f32 - 1.0;
    let span = edge + edge * 0.7 + 10.0;
    let mut frames = Vec::new();
    for f in 0..=(gleam_start + gleam_len + 8) {
        let wm = if f > gleam_start + 4 {
            ((f - gleam_start - 4) as f32 / 6.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut grid = vec![None; sub_w * sub_h];
        for y in 0..sub {
            for x in 0..sub {
                let Some(base) = mark.px[y * sub + x] else {
                    continue;
                };
                let arrived = reveal[y * sub + x];
                if f < arrived {
                    continue;
                }
                let age = f - arrived;
                // A piece snaps in bright for one beat — the puzzle click.
                let mut lit: f32 = match age {
                    0 => 0.75,
                    1 => 0.32,
                    _ => 0.0,
                };
                if f < chord {
                    // The jigsaw contour glows wherever the neighbouring
                    // piece is still missing, so waiting notches read as
                    // shapes.
                    let missing = (x > 0 && reveal[y * sub + x - 1] > f)
                        || (x + 1 < sub && reveal[y * sub + x + 1] > f)
                        || (y > 0 && reveal[(y - 1) * sub + x] > f)
                        || (y + 1 < sub && reveal[(y + 1) * sub + x] > f);
                    if missing {
                        lit = lit.max(0.3);
                    }
                }
                if f >= gleam_start && f <= gleam_start + gleam_len {
                    // Factory-fresh gleam: one diagonal highlight sweep.
                    let p = -5.0 + span * ease_in_out((f - gleam_start) as f32 / gleam_len as f32);
                    let u = x as f32 + y as f32 * 0.7;
                    lit = lit.max((1.0 - (u - p).abs() / 4.0).max(0.0).powf(1.5) * 0.95);
                }
                grid[(y + pad_y) * sub_w + (x + pad_x)] =
                    Some(shade(base, bg, CellState { mix: 1.0, lit }));
            }
        }
        let mut glyphs = Vec::new();
        for note in &notes {
            if f < note.f0 || f >= note.arrive {
                continue;
            }
            // Fading afterimages at the previous two quantized cells.
            for (back, alpha) in [(2i32, 0.16f32), (1, 0.38), (0, 1.0)] {
                let pf = f - back;
                if pf <= note.f0 {
                    continue;
                }
                let (p, x, y) = note_pos(note, pf);
                let col = x.round() as i32 + pad_x as i32;
                let row = y.round() as i32 + (pad_y / 2) as i32;
                if col < 0 || col >= sub_w as i32 || row < 0 || row >= (sub_h / 2) as i32 {
                    continue;
                }
                let lift = 90.0 * (1.0 - p) + 45.0;
                let bright = |c: u8| (f32::from(c) + lift).min(255.0);
                let blend =
                    |c: f32, g: u8| (f32::from(g) + (c - f32::from(g)) * alpha).round() as u8;
                glyphs.push(NoteGlyph {
                    col: col as u16,
                    row: row as u16,
                    glyph: note.glyph,
                    rgb: (
                        blend(bright(note.rgb.0), bg.0),
                        blend(bright(note.rgb.1), bg.1),
                        blend(bright(note.rgb.2), bg.2),
                    ),
                });
            }
        }
        frames.push(IntroFrame {
            dur: ms(42),
            wm,
            grid,
            notes: glyphs,
        });
    }
    // The resting frame: the finished mark, nothing else.
    let mut grid = vec![None; sub_w * sub_h];
    for y in 0..sub {
        for x in 0..sub {
            grid[(y + pad_y) * sub_w + (x + pad_x)] = mark.px[y * sub + x];
        }
    }
    frames.push(IntroFrame {
        dur: ms(900),
        wm: 1.0,
        grid,
        notes: Vec::new(),
    });
    IntroSequence {
        sub_w,
        sub_h,
        mark_side: sub,
        frames,
        art: mark.art.clone(),
        // `pad_y` counts sub-pixels and notes_pads keeps it even, so the
        // mark starts on a cell boundary rather than straddling one.
        art_at: (pad_x, pad_y / 2),
    }
}

/* ---------------- sequence entry points ---------------- */

/// Flight padding for the notes style, shrunk to whatever the terminal has.
pub(crate) fn notes_pads(cols: u16, rows: u16) -> (usize, usize) {
    let pad_x = usize::from(cols.saturating_sub(COLS + INDENT as u16 * 2) / 2).min(10);
    let pad_rows = usize::from(rows.saturating_sub(ROWS + CHROME_ROWS) / 2).min(5);
    (pad_x, pad_rows * 2)
}

/// Build the full frame sequence for a style. `bg` is what fades resolve
/// against — the theme background in-app, black on the raw updater screen
/// where the real background is unknown. `spec` chooses the mark's size and
/// pixel styling so the intro can land exactly on the idle dashboard's art.
pub(crate) fn sequence(
    intro: IntroStyle,
    bg: (u8, u8, u8),
    pads: (usize, usize),
    spec: &MarkSpec,
) -> Option<IntroSequence> {
    let mark = mark(spec)?;
    Some(match intro {
        IntroStyle::Off => return None,
        IntroStyle::Classic => classic(&mark, bg),
        IntroStyle::Pulse => pulse(&mark, bg),
        IntroStyle::Scan => scan(&mark, bg),
        IntroStyle::Ripple => ripple(&mark, bg),
        IntroStyle::Dissolve => dissolve(&mark, bg),
        IntroStyle::Breath => breath(&mark, bg),
        IntroStyle::Spectrum => spectrum(&mark, bg),
        IntroStyle::Vinyl => vinyl(&mark, bg),
        IntroStyle::Shimmer => shimmer(&mark, bg),
        IntroStyle::Rain => rain(&mark, bg),
        IntroStyle::Zoom => zoom(&mark, bg, spec),
        IntroStyle::Notes => notes(&mark, bg, pads.0, pads.1),
    })
}

/* ---------------- startup playback ---------------- */

/// Plays the intro and polls terminal input between frames so any key can cut
/// it short. Returns `false` when the terminal cannot carry the animation.
pub(crate) async fn play_interactive(
    style: Style,
    version: &str,
    intro: IntroStyle,
) -> std::io::Result<bool> {
    if intro == IntroStyle::Off || !can_animate(style) {
        return Ok(false);
    }
    let (cols, rows) = crossterm::terminal::size()?;
    let Some(seq) = sequence(intro, (0, 0, 0), notes_pads(cols, rows), &MarkSpec::plain()) else {
        return Ok(false);
    };
    let placed = layout(cols, rows, seq.sub_w as u16, (seq.sub_h / 2) as u16);
    let _raw_mode = RawModeGuard::enter()?;
    let mut terminal = IntroTerminal::enter()?;

    let mut cut_short = false;
    for frame in &seq.frames {
        if poll_keypress()? {
            cut_short = true;
            break;
        }
        print!("{}", render_ansi(style, &seq, frame, placed, version));
        let _ = std::io::stdout().flush();
        if wait_for_input(frame.dur).await? {
            cut_short = true;
            break;
        }
    }
    if cut_short {
        // A skip still lands on the finished mark, not a half-drawn frame.
        if let Some(last) = seq.frames.last() {
            print!("{}", render_ansi(style, &seq, last, placed, version));
            let _ = std::io::stdout().flush();
        }
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
    const INPUT_POLL: Duration = Duration::from_millis(16);
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

/* ---------------- ANSI serialization ---------------- */

/// Where the intro block sits: centred in the terminal, the same region the
/// main page is about to occupy, rather than pinned to the top-left corner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Layout {
    left: usize,
    top: u16,
}

fn layout(cols: u16, rows: u16, view_cols: u16, view_rows: u16) -> Layout {
    Layout {
        left: usize::from(cols.saturating_sub(view_cols) / 2),
        top: rows.saturating_sub(view_rows + CHROME_ROWS) / 2,
    }
}

/// Clear to end of line, then a full CRLF: the intro plays in raw mode, where
/// OPOST is off and a bare LF moves down without returning the carriage —
/// each row would start where the previous one ended and staircase off the
/// right edge of the screen.
const EOL: &str = "\x1b[K\r\n";

fn render_ansi(
    style: Style,
    seq: &IntroSequence,
    frame: &IntroFrame,
    placed: Layout,
    version: &str,
) -> String {
    let cells = FrameCells::new(seq, frame);
    let pad = " ".repeat(placed.left);
    let mut out = String::from(HOME);
    for _ in 0..placed.top {
        out.push_str(EOL);
    }
    out.push_str(EOL);
    let rgb = |value: Option<(u8, u8, u8)>| match value {
        Some((r, g, b)) => Color::Rgb(r, g, b),
        None => Color::Reset,
    };
    for row in 0..seq.sub_h / 2 {
        out.push_str(&pad);
        for col in 0..seq.sub_w {
            match cells.cell(col, row) {
                ComposedCell::Empty => {
                    out.push_str(&colour(style, Color::Reset, true));
                    out.push_str(&colour(style, Color::Reset, false));
                    out.push(' ');
                }
                ComposedCell::Pixels { top, bottom } => {
                    let (glyph, fg, bg) = match (top, bottom) {
                        (Some(_), _) => ('▀', rgb(top), rgb(bottom)),
                        (None, Some(_)) => ('▄', rgb(bottom), Color::Reset),
                        (None, None) => unreachable!("empty cells are ComposedCell::Empty"),
                    };
                    out.push_str(&colour(style, fg, true));
                    out.push_str(&colour(style, bg, false));
                    out.push(glyph);
                }
                ComposedCell::Art { glyph, fg, bg } => {
                    out.push_str(&colour(style, fg, true));
                    out.push_str(&colour(style, bg, false));
                    out.push(glyph);
                }
                ComposedCell::Note {
                    glyph,
                    rgb: fg,
                    under,
                } => {
                    out.push_str(&colour(style, Color::Rgb(fg.0, fg.1, fg.2), true));
                    out.push_str(&colour(style, rgb(under), false));
                    out.push(glyph);
                    // Note glyphs are ambiguous-width: some terminals draw
                    // them two columns wide, which would shove the rest of
                    // the row sideways. Re-anchor the cursor to the next
                    // cell's absolute column so either width lands right.
                    out.push_str(&format!("\x1b[{}G", placed.left + col + 2));
                }
            }
        }
        out.push_str(RESET);
        out.push_str(EOL);
    }
    out.push_str(EOL);
    // The wordmark sits under the mark's own centre, not the screen's.
    let centre = placed.left + seq.sub_w / 2;
    if frame.wm < 0.15 {
        out.push_str(EOL);
        out.push_str(EOL);
    } else {
        // A three-step fade: DIM first, then the full brand colours.
        let name = if frame.wm < 0.6 {
            style.paint(DIM, "ypm")
        } else {
            style.paint(&format!("{BOLD}{}", style.brand), "ypm")
        };
        out.push_str(&format!("{}{}{EOL}", " ".repeat(centre - 1), name));
        out.push_str(&format!(
            "{}{}{EOL}",
            " ".repeat(centre.saturating_sub(version.chars().count() / 2)),
            style.paint(DIM, version)
        ));
    }
    out
}

/// `Color::Reset` is the terminal's own default, which is what the icon's
/// transparent margin must stay — never a painted rectangle.
fn colour(style: Style, color: Color, foreground: bool) -> String {
    let (base, reset) = if foreground { (38, 39) } else { (48, 49) };
    match color {
        Color::Rgb(red, green, blue) => {
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

    fn test_style() -> Style {
        Style {
            animated: true,
            unicode: true,
            truecolor: true,
            brand: "\x1b[38;2;51;94;234m",
        }
    }

    fn playable_styles() -> impl Iterator<Item = IntroStyle> {
        IntroStyle::ALL
            .into_iter()
            .filter(|style| *style != IntroStyle::Off)
    }

    #[test]
    fn the_shipped_icon_renders_at_the_intro_size() {
        let cover = cover().expect("the embedded logo must decode");
        assert_eq!((cover.width, cover.height), (COLS, ROWS));
        assert_eq!(cover.cells.len(), usize::from(COLS) * usize::from(ROWS));
        // A mark that came out entirely transparent would animate as a blank
        // screen; the icon has to actually put ink on the middle row.
        let mark = mark(&MarkSpec::plain()).expect("the embedded logo must decode");
        let middle = SUB / 2 * SUB;
        assert!(mark.px[middle..middle + SUB].iter().any(Option::is_some));
    }

    #[test]
    fn lower_half_cells_keep_their_pixel_in_the_lower_row() {
        // The icon's top edge produces '▄' cells (upper transparent, lower
        // inked) whose fg is the LOWER sub-pixel. Mapping them through '▀'
        // semantics shifted those pixels up a row and chewed the corners.
        let cover = cover().expect("the embedded logo must decode");
        let px = mark_pixels(&cover);
        let w = usize::from(cover.width);
        let mut lower_only_cells = 0;
        for row in 0..usize::from(cover.height) {
            for col in 0..w {
                let cell = cover.cells[row * w + col];
                if cell.glyph == '▄' {
                    lower_only_cells += 1;
                    assert!(px[row * 2 * w + col].is_none(), "({col},{row}) top");
                    assert!(
                        px[(row * 2 + 1) * w + col].is_some(),
                        "({col},{row}) bottom"
                    );
                }
            }
        }
        // The guard must actually see such cells, or it guards nothing.
        assert!(lower_only_cells > 0, "icon has no '▄' edge cells to check");
    }

    #[test]
    fn every_row_is_reachable_from_the_centre_line() {
        // A row the reveal can never light would stay blank for the whole
        // animation, leaving a hole in the mark — at every mark size the
        // dashboard can ask for.
        for side in [12usize, 24, 28, 36] {
            let rows = (side / 2) as i32;
            for y in 0..side {
                let d = reach(rows, y);
                assert!((0..rows / 2).contains(&d), "side {side} sub-row {y}");
            }
        }
    }

    #[test]
    fn the_transparent_margin_stays_the_terminals_own_background() {
        let style = test_style();
        assert_eq!(colour(style, Color::Reset, true), "\x1b[39m");
        assert_eq!(colour(style, Color::Reset, false), "\x1b[49m");
    }

    #[test]
    fn lighting_a_pixel_moves_it_toward_white_and_stops_there() {
        let lit = CellState { mix: 1.0, lit: 1.0 };
        assert_eq!(shade((255, 255, 255), (0, 0, 0), lit), (255, 255, 255));
        assert_eq!(shade((0, 0, 0), (0, 0, 0), lit).0, LIT as u8);
        // A fully faded pixel sits on the background it fades from.
        let hidden = CellState { mix: 0.0, lit: 0.0 };
        assert_eq!(shade((200, 100, 50), (10, 20, 30), hidden), (10, 20, 30));
    }

    #[test]
    fn every_style_builds_frames_and_ends_on_the_finished_mark() {
        let mark = mark(&MarkSpec::plain()).expect("the embedded logo must decode");
        for intro in playable_styles() {
            let seq = sequence(intro, (0, 0, 0), (10, 10), &MarkSpec::plain())
                .expect("sequence must build");
            assert!(seq.frames.len() > 3, "{intro:?} has too few frames");
            assert!(
                seq.total() > Duration::from_millis(500),
                "{intro:?} too short"
            );
            let last = seq.frames.last().expect("at least one frame");
            assert!(last.wm >= 1.0, "{intro:?} must end with the wordmark");
            assert!(last.notes.is_empty(), "{intro:?} must not end mid-flight");
            let pad_x = (seq.sub_w - SUB) / 2;
            let pad_y = (seq.sub_h - SUB) / 2;
            for y in 0..SUB {
                for x in 0..SUB {
                    let expected = mark.px[y * SUB + x];
                    let got = last.grid[(y + pad_y) * seq.sub_w + (x + pad_x)];
                    assert_eq!(got, expected, "{intro:?} final pixel ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn frames_return_the_carriage_because_raw_mode_turns_off_output_translation() {
        let style = test_style();
        for intro in playable_styles() {
            let seq = sequence(intro, (0, 0, 0), (10, 10), &MarkSpec::plain())
                .expect("sequence must build");
            let placed = layout(150, 45, seq.sub_w as u16, (seq.sub_h / 2) as u16);
            for (index, frame) in seq.frames.iter().enumerate() {
                let text = render_ansi(style, &seq, frame, placed, "v0.9.3");
                let bytes = text.as_bytes();
                let newlines = bytes.iter().filter(|&&byte| byte == b'\n').count();
                // The guard must actually see line breaks, or it guards
                // nothing.
                assert!(newlines > seq.sub_h / 2, "{intro:?} frame {index}");
                for (offset, &byte) in bytes.iter().enumerate() {
                    if byte == b'\n' {
                        assert_eq!(
                            bytes.get(offset.wrapping_sub(1)),
                            Some(&b'\r'),
                            "{intro:?} frame {index}: bare LF at byte {offset}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_mark_is_centred_where_the_main_page_will_draw() {
        // A roomy terminal centres the block both ways.
        assert_eq!(layout(150, 40, 36, 18), Layout { left: 57, top: 8 });
        // The smallest terminal can_animate accepts still leaves the minimum
        // side margin and starts at the top instead of underflowing.
        assert_eq!(
            layout(COLS + INDENT as u16 * 2, ROWS + CHROME_ROWS, COLS, ROWS),
            Layout {
                left: INDENT,
                top: 0
            }
        );
        // A terminal smaller than the mark must not panic on subtraction.
        assert_eq!(layout(10, 5, 36, 18), Layout { left: 0, top: 0 });
    }

    #[test]
    fn rgb_backgrounds_do_not_turn_the_transparent_margin_into_ink() {
        // The icon has rounded corners; on an RGB theme the flattened corner
        // pixels come back exactly background-coloured. Treating them as ink
        // would make every style animate the full square — flying background
        // tiles, glowing corners under the gleam pass.
        let bg = (20u8, 16u8, 19u8);
        let spec = MarkSpec {
            side: 28,
            palette_mode: CoverPalette::Original,
            palette: &[],
            detail_scale: 1.0,
            background: Color::Rgb(bg.0, bg.1, bg.2),
            detail: CoverDetail::Half,
        };
        let mark = mark(&spec).expect("mark must render");
        assert_eq!(mark.px[0], None, "top-left corner must stay transparent");
        assert_eq!(
            mark.px[mark.side - 1],
            None,
            "top-right corner must stay transparent"
        );
        // And the icon itself must still be there.
        assert!(
            mark.px.iter().filter(|px| px.is_some()).count() > mark.side * mark.side / 2,
            "the mask must not eat the icon"
        );
    }

    #[test]
    fn the_intro_lands_on_the_idle_dashboards_own_art() {
        // The launch intro anchors on the idle art rect and must finish as
        // exactly the picture the dashboard keeps showing: same renderer,
        // same size, same palette, same background blend for soft edges.
        let bg = (20u8, 16u8, 19u8);
        let spec = MarkSpec {
            side: 28,
            palette_mode: CoverPalette::Original,
            palette: &[],
            detail_scale: 1.0,
            background: Color::Rgb(bg.0, bg.1, bg.2),
            detail: CoverDetail::Half,
        };
        let seq = sequence(IntroStyle::Notes, bg, (6, 6), &spec).expect("sequence must build");
        let last = seq.frames.last().expect("at least one frame");
        let idle = pixel::from_image_bytes(
            MARK,
            spec.palette_mode,
            spec.palette,
            spec.background,
            (28, 14),
            spec.detail_scale,
            CoverDetail::Half,
        )
        .expect("idle art must render");
        let idle_px = mark_pixels(&idle);
        for y in 0..28 {
            for x in 0..28 {
                // The intro masks the icon's transparent margin to None while
                // the idle art paints it in the background colour — the same
                // pixels on screen, so compare what the viewer sees.
                assert_eq!(
                    last.grid[(y + 6) * seq.sub_w + (x + 6)].unwrap_or(bg),
                    idle_px[y * 28 + x].unwrap_or(bg),
                    "pixel ({x},{y}) differs from the idle art"
                );
            }
        }
    }

    #[test]
    fn the_finished_mark_is_the_idle_art_at_every_cover_detail() {
        // Sub-pixel equality only proves the animation grid is right; what
        // the terminal shows is the composed cell. The dashboard quantizes
        // its idle art at the configured detail, so anything finer than half
        // blocks picks different glyphs for the same colours — composing the
        // last frame any other way snaps the whole mark into sharper focus
        // on the frame the intro hands over.
        let bg = (20u8, 16u8, 19u8);
        for detail in [
            CoverDetail::Half,
            CoverDetail::Quad,
            CoverDetail::Sextant,
            CoverDetail::Octant,
        ] {
            let spec = MarkSpec {
                side: 28,
                palette_mode: CoverPalette::Original,
                palette: &[],
                detail_scale: 1.0,
                background: Color::Rgb(bg.0, bg.1, bg.2),
                detail,
            };
            let seq = sequence(IntroStyle::Notes, bg, (6, 6), &spec).expect("sequence must build");
            let last = seq.frames.last().expect("at least one frame");
            let idle = pixel::from_image_bytes(
                MARK,
                spec.palette_mode,
                spec.palette,
                spec.background,
                (28, 14),
                spec.detail_scale,
                detail,
            )
            .expect("idle art must render");
            let cells = FrameCells::new(&seq, last);
            for row in 0..14 {
                for col in 0..28 {
                    let want = idle.cells[row * 28 + col];
                    // The notes style pads by (6, 6) sub-pixels: 6 columns
                    // and 3 cell rows. Every cell of the settled mark —
                    // masked margin included — must come from the art, so
                    // no per-platform quantization can split the two.
                    match cells.cell(col + 6, row + 3) {
                        ComposedCell::Art { glyph, fg, bg } => assert_eq!(
                            (glyph, fg, bg),
                            (want.glyph, want.fg, want.bg),
                            "{detail} cell ({col},{row}) differs from the idle art"
                        ),
                        _ => panic!("{detail} cell ({col},{row}) never settled onto the idle art"),
                    }
                }
            }
        }
    }

    #[test]
    fn notes_pads_shrink_to_the_terminal_instead_of_failing() {
        assert_eq!(notes_pads(150, 45), (10, 10));
        // The minimum playable terminal leaves no flight room but still runs.
        assert_eq!(
            notes_pads(COLS + INDENT as u16 * 2, ROWS + CHROME_ROWS),
            (0, 0)
        );
        let seq = sequence(IntroStyle::Notes, (0, 0, 0), (0, 0), &MarkSpec::plain())
            .expect("sequence must build");
        assert_eq!((seq.sub_w, seq.sub_h), (SUB, SUB));
    }

    #[test]
    fn every_jigsaw_piece_is_claimed_so_the_puzzle_always_completes() {
        let seq = sequence(IntroStyle::Notes, (0, 0, 0), (10, 10), &MarkSpec::plain())
            .expect("sequence must build");
        let mark = mark(&MarkSpec::plain()).expect("the embedded logo must decode");
        // Two frames before the resting frame the gleam has passed and every
        // piece must already be placed: no solid pixel may still be missing.
        let settled = &seq.frames[seq.frames.len() - 2];
        for y in 0..SUB {
            for x in 0..SUB {
                if mark.px[y * SUB + x].is_some() {
                    assert!(
                        settled.grid[(y + 10) * seq.sub_w + (x + 10)].is_some(),
                        "pixel ({x},{y}) never got its piece"
                    );
                }
            }
        }
    }

    #[test]
    fn note_glyphs_stay_inside_the_viewport() {
        let seq = sequence(IntroStyle::Notes, (0, 0, 0), (10, 10), &MarkSpec::plain())
            .expect("sequence must build");
        for frame in &seq.frames {
            for note in &frame.notes {
                assert!((note.col as usize) < seq.sub_w);
                assert!((note.row as usize) < seq.sub_h / 2);
            }
        }
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
