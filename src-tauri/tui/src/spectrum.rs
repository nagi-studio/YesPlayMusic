use std::f32::consts::PI;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use rodio::{ChannelCount, SampleRate, Source};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

pub const CAPTURE_SAMPLES: usize = 4096;
pub const FFT_SAMPLES: usize = 1024;
pub const SPECTRUM_BINS: usize = 66;
/// Paused audio below this level no longer leaves rounded half-cells visible.
const SILENCE_THRESHOLD: f32 = 0.01;

/// Single-producer sample ring. The audio thread only performs atomic stores.
pub struct SampleBuffer {
    slots: Box<[AtomicU32]>,
    sequence: AtomicU64,
}

impl Default for SampleBuffer {
    fn default() -> Self {
        Self {
            slots: (0..CAPTURE_SAMPLES)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect(),
            sequence: AtomicU64::new(0),
        }
    }
}

impl SampleBuffer {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn push(&self, sample: f32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        let index = sequence as usize % self.slots.len();
        self.slots[index].store(sample.to_bits(), Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Release);
    }

    pub fn clear(&self) {
        for slot in &self.slots {
            slot.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        self.sequence.store(0, Ordering::Release);
    }

    pub fn latest(&self, wanted: usize) -> Vec<f32> {
        for _ in 0..3 {
            let end = self.sequence.load(Ordering::Acquire);
            let count = wanted.min(end as usize).min(self.slots.len());
            let start = end.saturating_sub(count as u64);
            let samples = (start..end)
                .map(|sequence| {
                    let index = sequence as usize % self.slots.len();
                    f32::from_bits(self.slots[index].load(Ordering::Relaxed))
                })
                .collect::<Vec<_>>();
            let after = self.sequence.load(Ordering::Acquire);
            if after.wrapping_sub(end) <= (self.slots.len() - count) as u64 {
                return samples;
            }
        }
        Vec::new()
    }
}

/// Rodio adapter that forwards audio unchanged and taps complete channel frames.
pub struct SampleTap<S> {
    source: S,
    capture: Arc<SampleBuffer>,
    channel_cursor: u16,
    frame_sum: f32,
}

impl<S: Source> SampleTap<S> {
    pub fn new(source: S, capture: Arc<SampleBuffer>) -> Self {
        Self {
            source,
            capture,
            channel_cursor: 0,
            frame_sum: 0.0,
        }
    }
}

impl<S: Source> Iterator for SampleTap<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.source.next()?;
        let channels = self.source.channels().get();
        self.frame_sum += sample;
        self.channel_cursor += 1;
        if self.channel_cursor >= channels {
            self.capture.push(self.frame_sum / f32::from(channels));
            self.channel_cursor = 0;
            self.frame_sum = 0.0;
        }
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.source.size_hint()
    }
}

impl<S: Source> Source for SampleTap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.source.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.source.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.source.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.channel_cursor = 0;
        self.frame_sum = 0.0;
        self.source.try_seek(position)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpectrumKind {
    #[default]
    Blocks,
    Mirror,
    Led,
    Braille,
    Shade,
    Scope,
    Fire,
    Waterfall,
    Vu,
    Reflect,
}

impl SpectrumKind {
    pub const ALL: [Self; 10] = [
        Self::Blocks,
        Self::Mirror,
        Self::Led,
        Self::Braille,
        Self::Shade,
        Self::Scope,
        Self::Fire,
        Self::Waterfall,
        Self::Vu,
        Self::Reflect,
    ];
}

// Automatic gain: bars are normalized against a slowly decaying rolling
// peak so the loudest bin always reaches full height. A fixed mapping
// leaves typical music at 5-30% amplitude, which reads as a dead band no
// matter how many rows the spectrum gets.
const CEILING_FLOOR: f32 = 0.12;
const CEILING_DECAY: f32 = 0.995;

struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    work: Vec<Complex<f32>>,
    bins: [f32; SPECTRUM_BINS],
    ceiling: f32,
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        let mut planner = FftPlanner::new();
        Self {
            fft: planner.plan_fft_forward(FFT_SAMPLES),
            work: vec![Complex::default(); FFT_SAMPLES],
            bins: [0.0; SPECTRUM_BINS],
            ceiling: CEILING_FLOOR,
        }
    }
}

impl SpectrumAnalyzer {
    fn process(&mut self, samples: &[f32]) -> &[f32; SPECTRUM_BINS] {
        let input_start = samples.len().saturating_sub(FFT_SAMPLES);
        let input = &samples[input_start..];
        let pad = FFT_SAMPLES - input.len();
        for (index, value) in self.work.iter_mut().enumerate() {
            let sample = index
                .checked_sub(pad)
                .and_then(|source| input.get(source))
                .copied()
                .unwrap_or(0.0);
            let hann = 0.5 - 0.5 * (2.0 * PI * index as f32 / (FFT_SAMPLES - 1) as f32).cos();
            *value = Complex::new(sample * hann, 0.0);
        }
        self.fft.process(&mut self.work);

        let high = (FFT_SAMPLES / 2) as f32;
        let mut targets = [0.0_f32; SPECTRUM_BINS];
        for (index, target) in targets.iter_mut().enumerate() {
            let start = high
                .powf(index as f32 / SPECTRUM_BINS as f32)
                .floor()
                .max(1.0) as usize;
            let end = high
                .powf((index + 1) as f32 / SPECTRUM_BINS as f32)
                .ceil()
                .max((start + 1) as f32) as usize;
            let magnitude = self.work[start..end.min(FFT_SAMPLES / 2)]
                .iter()
                .map(|value| value.norm())
                .fold(0.0_f32, f32::max)
                / FFT_SAMPLES as f32;
            *target = (magnitude * 40.0).ln_1p() / 41.0_f32.ln();
        }
        let frame_peak = targets.iter().copied().fold(0.0_f32, f32::max);
        self.ceiling = (self.ceiling * CEILING_DECAY)
            .max(frame_peak)
            .max(CEILING_FLOOR);
        for (bin, target) in self.bins.iter_mut().zip(targets) {
            let normalized = (target / self.ceiling).clamp(0.0, 1.0);
            let coefficient = if normalized > *bin { 0.4 } else { 0.09 };
            *bin += (normalized - *bin) * coefficient;
        }
        &self.bins
    }
}

pub trait SpectrumStyle {
    fn set_terminal_background(&mut self, _background: Option<Color>) {}

    fn render(
        &mut self,
        bins: &[f32],
        samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    );
}

pub struct SpectrumView {
    analyzer: SpectrumAnalyzer,
    bins: [f32; SPECTRUM_BINS],
    samples: Vec<f32>,
    kind: SpectrumKind,
    style: Box<dyn SpectrumStyle>,
    glow: GhostBuffer,
    ticks: u64,
    settled: bool,
    terminal_background: Option<Color>,
}

impl SpectrumView {
    pub fn new(kind: SpectrumKind) -> Self {
        Self {
            analyzer: SpectrumAnalyzer::default(),
            bins: [0.0; SPECTRUM_BINS],
            samples: vec![0.0; FFT_SAMPLES],
            kind,
            style: style_for(kind),
            glow: GhostBuffer::default(),
            ticks: 0,
            settled: true,
            terminal_background: None,
        }
    }

    pub fn set_terminal_background(&mut self, background: Option<Color>) {
        self.terminal_background = background;
    }

    pub fn tick(&mut self, capture: &SampleBuffer, playing: bool, preview: bool) {
        self.ticks = self.ticks.wrapping_add(1);
        let real = capture.latest(FFT_SAMPLES);
        self.samples = if playing && !real.is_empty() {
            real
        } else if playing || preview {
            simulated_samples(self.ticks)
        } else {
            vec![0.0; FFT_SAMPLES]
        };
        self.bins = *self.analyzer.process(&self.samples);
        let settled =
            !playing && !preview && self.bins.iter().all(|value| *value < SILENCE_THRESHOLD);
        if settled && !self.settled {
            self.style = style_for(self.kind);
            self.glow.clear();
        }
        self.settled = settled;
    }

    pub fn render(
        &mut self,
        kind: SpectrumKind,
        glow: bool,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        if area.is_empty() {
            return;
        }
        if kind != self.kind {
            self.kind = kind;
            self.style = style_for(kind);
            self.glow.clear();
        }
        self.style.set_terminal_background(self.terminal_background);
        clear_area(buf, area, theme.bg);
        if self.settled {
            self.glow.clear();
            return;
        }
        self.style
            .render(&self.bins, &self.samples, area, buf, theme);
        if glow {
            self.glow.apply(area, buf, theme, self.terminal_background);
        } else {
            self.glow.clear();
        }
    }
}

fn simulated_samples(tick: u64) -> Vec<f32> {
    let elapsed = tick as f32 * 0.05;
    let beat_phase = (elapsed % 0.48) / 0.48;
    let pulse = (-6.0 * beat_phase).exp();
    let envelope = 0.22 + pulse * 0.78;
    (0..FFT_SAMPLES)
        .map(|index| {
            let time = elapsed + index as f32 / 44_100.0;
            envelope
                * ((2.0 * PI * 55.0 * time).sin() * 0.58
                    + (2.0 * PI * 110.0 * time).sin() * 0.27
                    + (2.0 * PI * 440.0 * time).sin() * 0.09)
        })
        .collect()
}

fn style_for(kind: SpectrumKind) -> Box<dyn SpectrumStyle> {
    match kind {
        SpectrumKind::Blocks => Box::new(Blocks),
        SpectrumKind::Mirror => Box::new(Mirror),
        SpectrumKind::Led => Box::new(Led::default()),
        SpectrumKind::Braille => Box::new(Braille),
        SpectrumKind::Shade => Box::new(Shade),
        SpectrumKind::Scope => Box::new(Scope),
        SpectrumKind::Fire => Box::new(Fire::default()),
        SpectrumKind::Waterfall => Box::new(Waterfall::default()),
        SpectrumKind::Vu => Box::new(Vu::default()),
        SpectrumKind::Reflect => Box::new(Reflect::default()),
    }
}

#[derive(Clone, Copy)]
struct Ramp([Color; 5]);

impl Ramp {
    fn new(theme: &Theme) -> Self {
        Self([
            theme.faint,
            theme.dim,
            theme.accent,
            theme.accent2,
            theme.fg,
        ])
    }

    fn at(self, intensity: f32) -> Color {
        let index = (intensity.clamp(0.0, 0.999) * self.0.len() as f32) as usize;
        self.0[index]
    }
}

fn mix_color(foreground: Color, background: Color, background_weight: f32) -> Color {
    match (foreground, background) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(br, bg, bb)) => {
            let weight = background_weight.clamp(0.0, 1.0);
            let mix = |front: u8, back: u8| {
                (f32::from(front) * (1.0 - weight) + f32::from(back) * weight).round() as u8
            };
            Color::Rgb(mix(fr, br), mix(fg, bg), mix(fb, bb))
        }
        (foreground, background) => {
            if background_weight >= 0.5 {
                background
            } else {
                foreground
            }
        }
    }
}

fn mix_with_background(
    foreground: Color,
    theme: &Theme,
    terminal_background: Option<Color>,
    background_weight: f32,
) -> Color {
    let Some(background) = theme.resolved_background(terminal_background) else {
        return theme.faint;
    };
    match (foreground, background) {
        (Color::Rgb(..), Color::Rgb(..)) => mix_color(foreground, background, background_weight),
        _ => theme.faint,
    }
}

fn clear_area(buf: &mut Buffer, area: Rect, background: Color) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(" ").set_bg(background);
            }
        }
    }
}

fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, symbol: &str, color: Color, bg: Color) {
    let position = (area.x.saturating_add(x), area.y.saturating_add(y));
    if x < area.width && y < area.height {
        if let Some(cell) = buf.cell_mut(position) {
            cell.set_symbol(symbol)
                .set_style(Style::new().fg(color).bg(bg));
        }
    }
}

fn bin_value(bins: &[f32], position: usize, count: usize) -> f32 {
    if bins.is_empty() || count == 0 {
        return 0.0;
    }
    let start = position * bins.len() / count;
    let end = ((position + 1) * bins.len())
        .div_ceil(count)
        .min(bins.len());
    let slice = &bins[start.min(bins.len() - 1)..end.max(start + 1).min(bins.len())];
    slice.iter().sum::<f32>() / slice.len() as f32
}

fn eighth_glyph(eighths: u16) -> &'static str {
    match eighths.min(8) {
        0 => " ",
        1 => "▁",
        2 => "▂",
        3 => "▃",
        4 => "▄",
        5 => "▅",
        6 => "▆",
        7 => "▇",
        _ => "█",
    }
}

fn shade_glyph(intensity: f32) -> &'static str {
    match (intensity.clamp(0.0, 0.999) * 4.0) as usize {
        0 => "░",
        1 => "▒",
        2 => "▓",
        _ => "█",
    }
}

const BRICK_GLYPH: &str = "▆";
const BRICK_WIDTH: u16 = 2;
const BRICK_STRIDE: u16 = 3;
const REFLECTION_BACKGROUND_WEIGHT: f32 = 0.62;

fn brick_count(width: u16) -> usize {
    usize::from(width.saturating_add(1) / BRICK_STRIDE).max(1)
}

fn brick_row_color(ramp: Ramp, from_bottom: u16, height: u16) -> Color {
    ramp.at((f32::from(from_bottom) + 1.0) / f32::from(height.max(1)))
}

fn draw_brick(buf: &mut Buffer, area: Rect, x: u16, y: u16, color: Color, background: Color) {
    for offset in 0..BRICK_WIDTH {
        put(buf, area, x + offset, y, BRICK_GLYPH, color, background);
    }
}

fn draw_brick_bar(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    value: f32,
    height: u16,
    theme: &Theme,
) -> u16 {
    let filled = (value.clamp(0.0, 1.0) * f32::from(height)).ceil() as u16;
    let ramp = Ramp::new(theme);
    for from_bottom in 0..filled.min(height) {
        draw_brick(
            buf,
            area,
            x,
            height - 1 - from_bottom,
            brick_row_color(ramp, from_bottom, height),
            theme.bg,
        );
    }
    filled.min(height)
}

fn reflection_color(main: Color, theme: &Theme, terminal_background: Option<Color>) -> Color {
    mix_with_background(
        main,
        theme,
        terminal_background,
        REFLECTION_BACKGROUND_WEIGHT,
    )
}

// Deliberately time-independent: hashing the frame counter made ~1 in 5
// reflection bricks blink every frame, which read as flicker instead of
// water. The ripple pattern is now a stable dither over (column, depth).
fn reflection_brick_visible(index: usize, distance: u16) -> bool {
    let mut hash = (index as u64)
        .wrapping_mul(0xbf58_476d_1ce4_e5b9)
        .wrapping_add(u64::from(distance).wrapping_mul(0x94d0_49bb_1331_11eb));
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    hash % 100 >= 18
}

fn draw_eighth_bar(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    width: u16,
    value: f32,
    height: u16,
    theme: &Theme,
) {
    let ramp = Ramp::new(theme);
    let eighths = (value.clamp(0.0, 1.0) * f32::from(height) * 8.0).round() as u16;
    for row in 0..height {
        let from_bottom = height - 1 - row;
        let fill = eighths.saturating_sub(from_bottom * 8).min(8);
        if fill == 0 {
            continue;
        }
        let color = ramp.at((f32::from(from_bottom) + f32::from(fill) / 8.0) / f32::from(height));
        for offset in 0..width {
            put(
                buf,
                area,
                x + offset,
                row,
                eighth_glyph(fill),
                color,
                theme.bg,
            );
        }
    }
}

struct Blocks;

impl SpectrumStyle for Blocks {
    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let count = usize::from((area.width + 1) / 3).max(1);
        for index in 0..count {
            draw_eighth_bar(
                buf,
                area,
                index as u16 * 3,
                2,
                bin_value(bins, index, count),
                area.height,
                theme,
            );
        }
    }
}

struct Mirror;

impl SpectrumStyle for Mirror {
    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let half = area.height / 2;
        if half == 0 {
            return;
        }
        let count = usize::from((area.width + 1) / 3).max(1);
        let ramp = Ramp::new(theme);
        for index in 0..count {
            let level = (bin_value(bins, index, count) * f32::from(half) * 2.0).round() as u16;
            for distance in 0..half {
                let remaining = level.saturating_sub(distance * 2).min(2);
                if remaining == 0 {
                    continue;
                }
                let top_symbol = if remaining == 1 { "▄" } else { "█" };
                let bottom_symbol = if remaining == 1 { "▀" } else { "█" };
                let color = ramp.at((f32::from(distance) + 1.0) / f32::from(half));
                for offset in 0..2 {
                    let x = index as u16 * 3 + offset;
                    put(
                        buf,
                        area,
                        x,
                        half - 1 - distance,
                        top_symbol,
                        color,
                        theme.bg,
                    );
                    put(
                        buf,
                        area,
                        x,
                        half + distance,
                        bottom_symbol,
                        color,
                        theme.bg,
                    );
                }
            }
        }
    }
}

#[derive(Default)]
struct Led {
    peaks: Vec<f32>,
    holds: Vec<u8>,
    terminal_background: Option<Color>,
}

impl SpectrumStyle for Led {
    fn set_terminal_background(&mut self, background: Option<Color>) {
        self.terminal_background = background;
    }

    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let count = brick_count(area.width);
        self.peaks.resize(count, 0.0);
        self.holds.resize(count, 0);
        let ramp = Ramp::new(theme);
        let afterglow = mix_with_background(ramp.at(0.25), theme, self.terminal_background, 0.68);
        for index in 0..count {
            let value = bin_value(bins, index, count);
            if value >= self.peaks[index] {
                self.peaks[index] = value;
                self.holds[index] = 8;
            } else if self.holds[index] > 0 {
                self.holds[index] -= 1;
            } else {
                self.peaks[index] = (self.peaks[index] - 0.0175).max(value);
            }
            let lit = (value * f32::from(area.height)).round() as u16;
            for distance in 0..area.height {
                let y = area.height - 1 - distance;
                let color = if distance < lit {
                    ramp.at((f32::from(distance) + 1.0) / f32::from(area.height))
                } else {
                    afterglow
                };
                draw_brick(buf, area, index as u16 * BRICK_STRIDE, y, color, theme.bg);
            }
            let peak_y = area
                .height
                .saturating_sub(1)
                .saturating_sub((self.peaks[index] * f32::from(area.height - 1)).round() as u16);
            for offset in 0..BRICK_WIDTH {
                put(
                    buf,
                    area,
                    index as u16 * BRICK_STRIDE + offset,
                    peak_y,
                    "▔",
                    ramp.at(1.0),
                    theme.bg,
                );
            }
        }
    }
}

struct Braille;

impl SpectrumStyle for Braille {
    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let columns = usize::from(area.width) * 2;
        let mut ridge = vec![0.0; columns];
        for (x, value) in ridge.iter_mut().enumerate() {
            let previous = bin_value(bins, x.saturating_sub(1), columns);
            let current = bin_value(bins, x, columns);
            let next = bin_value(bins, (x + 1).min(columns - 1), columns);
            *value = (previous + current * 2.0 + next) / 4.0;
        }
        let ramp = Ramp::new(theme);
        let mut canvas = BrailleCanvas::new(area);
        let pixel_height = area.height.saturating_mul(4);
        for (x, value) in ridge.into_iter().enumerate() {
            let height = (value * f32::from(area.height) * 4.0).round() as u16;
            for from_bottom in 0..height {
                let y = pixel_height.saturating_sub(1).saturating_sub(from_bottom);
                canvas.set_dot(
                    x as u16,
                    y,
                    ramp.at(f32::from(from_bottom) / (f32::from(area.height) * 4.0)),
                );
            }
        }
        canvas.flush(buf, theme.bg);
    }
}

struct Shade;

impl SpectrumStyle for Shade {
    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let ramp = Ramp::new(theme);
        for x in 0..area.width {
            let value = bin_value(bins, usize::from(x), usize::from(area.width));
            let height = (value * f32::from(area.height)).ceil() as u16;
            for distance in 0..height {
                let intensity = 1.0 - f32::from(distance) / f32::from(height.max(1));
                put(
                    buf,
                    area,
                    x,
                    area.height - 1 - distance,
                    shade_glyph(intensity),
                    ramp.at((f32::from(distance) + 1.0) / f32::from(area.height)),
                    theme.bg,
                );
            }
        }
    }
}

struct Scope;

impl SpectrumStyle for Scope {
    fn render(
        &mut self,
        _bins: &[f32],
        samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        if samples.is_empty() {
            return;
        }
        let mut canvas = BrailleCanvas::new(area);
        let pixels_w = area.width.saturating_mul(2);
        let pixels_h = area.height.saturating_mul(4);
        let middle = pixels_h.saturating_sub(1) as f32 / 2.0;
        let ramp = Ramp::new(theme);
        for x in 0..pixels_w {
            let index = usize::from(x) * (samples.len() - 1) / usize::from(pixels_w.max(1));
            let y = (middle - samples[index].clamp(-1.0, 1.0) * middle)
                .round()
                .clamp(0.0, f32::from(pixels_h.saturating_sub(1))) as u16;
            canvas.set_dot(x, y, ramp.at(samples[index].abs()));
            canvas.set_dot(x, (y + 1).min(pixels_h.saturating_sub(1)), ramp.at(0.75));
        }
        for x in (0..area.width).step_by(4) {
            put(buf, area, x, area.height / 2, "·", ramp.at(0.0), theme.bg);
        }
        canvas.flush(buf, theme.bg);
    }
}

#[derive(Default)]
struct Fire {
    width: u16,
    height: u16,
    heat: Vec<f32>,
    seed: u64,
}

impl SpectrumStyle for Fire {
    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        self.resize(area);
        if self.heat.is_empty() {
            return;
        }
        let width = usize::from(self.width);
        let height = usize::from(self.height);
        for x in 0..width {
            self.heat[(height - 1) * width + x] = bin_value(bins, x, width);
        }
        let mut next = self.heat.clone();
        for y in 1..height {
            for x in 0..width {
                let random = self.random();
                let decay = 0.06 + random * 0.14;
                let drift = match (self.random() * 3.0) as i32 {
                    0 => -1,
                    1 => 0,
                    _ => 1,
                };
                let target_x = (x as i32 + drift).clamp(0, width as i32 - 1) as usize;
                next[(y - 1) * width + target_x] = (self.heat[y * width + x] - decay).max(0.0);
            }
        }
        self.heat = next;
        let ramp = Ramp::new(theme);
        for y in 0..height {
            for x in 0..width {
                let heat = self.heat[y * width + x];
                if heat > 0.02 {
                    put(
                        buf,
                        area,
                        x as u16,
                        y as u16,
                        shade_glyph(heat),
                        ramp.at(heat),
                        theme.bg,
                    );
                }
            }
        }
    }
}

impl Fire {
    fn resize(&mut self, area: Rect) {
        if self.width != area.width || self.height != area.height {
            self.width = area.width;
            self.height = area.height;
            self.heat = vec![0.0; usize::from(area.width) * usize::from(area.height)];
            self.seed = 0x9e37_79b9_7f4a_7c15;
        }
    }

    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed as u32) as f32 / u32::MAX as f32
    }
}

#[derive(Default)]
struct Waterfall {
    history: Vec<Vec<f32>>,
    tick: bool,
    width: u16,
    height: u16,
}

impl SpectrumStyle for Waterfall {
    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        if self.width != area.width || self.height != area.height {
            self.width = area.width;
            self.height = area.height;
            self.history = vec![vec![0.0; usize::from(area.width)]; usize::from(area.height)];
        }
        self.tick = !self.tick;
        if self.tick && !self.history.is_empty() {
            self.history.rotate_right(1);
            for x in 0..usize::from(area.width) {
                self.history[0][x] = bin_value(bins, x, usize::from(area.width));
            }
        }
        let ramp = Ramp::new(theme);
        for (y, row) in self.history.iter().enumerate() {
            for (x, intensity) in row.iter().copied().enumerate() {
                if intensity > 0.02 {
                    put(
                        buf,
                        area,
                        x as u16,
                        y as u16,
                        shade_glyph(intensity),
                        ramp.at(intensity),
                        theme.bg,
                    );
                }
            }
        }
    }
}

#[derive(Default)]
struct Vu {
    levels: [f32; 2],
}

impl SpectrumStyle for Vu {
    fn render(
        &mut self,
        _bins: &[f32],
        samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let mut low_pass = 0.0_f32;
        let mut energy = [0.0_f32; 2];
        for sample in samples {
            low_pass += (*sample - low_pass) * 0.08;
            energy[0] += low_pass * low_pass;
            let high = *sample - low_pass;
            energy[1] += high * high;
        }
        let divisor = samples.len().max(1) as f32;
        for (index, energy) in energy.into_iter().enumerate() {
            let target = (energy / divisor).sqrt().mul_add(3.2, 0.0).clamp(0.0, 1.0);
            self.levels[index] += (target - self.levels[index]) * 0.12;
        }
        let left_width = area.width / 2;
        let regions = [
            Rect::new(area.x, area.y, left_width, area.height),
            Rect::new(
                area.x + left_width,
                area.y,
                area.width - left_width,
                area.height,
            ),
        ];
        for (index, region) in regions.into_iter().enumerate() {
            self.draw_meter(index, region, buf, theme);
        }
    }
}

impl Vu {
    fn draw_meter(&self, index: usize, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.width < 2 || area.height < 2 {
            return;
        }
        let local = Rect::new(0, 0, area.width, area.height);
        let mut canvas = BrailleCanvas::new(local);
        let width = f32::from(area.width.saturating_mul(2));
        let height = f32::from(area.height.saturating_mul(4));
        let center = (width / 2.0, height - 2.0);
        let radius = (width * 0.43, height * 0.72);
        let ramp = Ramp::new(theme);
        for mark in 0..9 {
            let ratio = mark as f32 / 8.0;
            let angle = PI + ratio * PI;
            let x = center.0 + angle.cos() * radius.0;
            let y = center.1 + angle.sin() * radius.1;
            let color = if mark >= 7 {
                ramp.at(1.0)
            } else {
                ramp.at(0.25)
            };
            canvas.set_dot(x.round() as u16, y.max(0.0).round() as u16, color);
        }
        let angle = PI + self.levels[index] * PI;
        let end = (
            center.0 + angle.cos() * radius.0 * 0.88,
            center.1 + angle.sin() * radius.1 * 0.88,
        );
        canvas.line(
            center.0.round() as i32,
            center.1.round() as i32,
            end.0.round() as i32,
            end.1.round() as i32,
            ramp.at(1.0),
        );
        canvas.flush_at(buf, area.x, area.y, theme.bg);
        put(
            buf,
            area,
            area.width / 2,
            area.height - 1,
            if index == 0 { "L" } else { "R" },
            ramp.at(0.25),
            theme.bg,
        );
    }
}

#[derive(Default)]
struct Reflect {
    terminal_background: Option<Color>,
}

impl SpectrumStyle for Reflect {
    fn set_terminal_background(&mut self, background: Option<Color>) {
        self.terminal_background = background;
    }

    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        // Bars get two thirds of the area, the reflection one third, so
        // the band reads as anchored to its bottom edge (the cover's).
        let line = area.height.saturating_mul(2) / 3;
        let ramp = Ramp::new(theme);
        for x in 0..area.width {
            put(buf, area, x, line, "─", theme.faint, theme.bg);
        }
        if line == 0 {
            return;
        }
        let count = brick_count(area.width);
        for index in 0..count {
            let value = bin_value(bins, index, count);
            let main_height =
                draw_brick_bar(buf, area, index as u16 * BRICK_STRIDE, value, line, theme);
            let reflected = main_height.div_ceil(2);
            for distance in 0..reflected.min(area.height.saturating_sub(line + 1)) {
                if reflection_brick_visible(index, distance) {
                    let source_row = (distance * 2).min(main_height.saturating_sub(1));
                    let color = reflection_color(
                        brick_row_color(ramp, source_row, line),
                        theme,
                        self.terminal_background,
                    );
                    draw_brick(
                        buf,
                        area,
                        index as u16 * BRICK_STRIDE,
                        line + 1 + distance,
                        color,
                        theme.bg,
                    );
                }
            }
        }
    }
}

struct BrailleCanvas {
    area: Rect,
    dots: Vec<u8>,
    colors: Vec<Option<Color>>,
}

impl BrailleCanvas {
    fn new(area: Rect) -> Self {
        let cells = usize::from(area.width) * usize::from(area.height);
        Self {
            area,
            dots: vec![0; cells],
            colors: vec![None; cells],
        }
    }

    fn set_dot(&mut self, x: u16, y: u16, color: Color) {
        if x >= self.area.width.saturating_mul(2) || y >= self.area.height.saturating_mul(4) {
            return;
        }
        let cell_x = x / 2;
        let cell_y = y / 4;
        let index = usize::from(cell_y) * usize::from(self.area.width) + usize::from(cell_x);
        let bit = match (x % 2, y % 4) {
            (0, 0) => 0,
            (0, 1) => 1,
            (0, 2) => 2,
            (0, 3) => 6,
            (1, 0) => 3,
            (1, 1) => 4,
            (1, 2) => 5,
            (1, 3) => 7,
            _ => unreachable!(),
        };
        self.dots[index] |= 1 << bit;
        self.colors[index] = Some(color);
    }

    fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            if x0 >= 0 && y0 >= 0 {
                self.set_dot(x0 as u16, y0 as u16, color);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let twice = 2 * error;
            if twice >= dy {
                error += dy;
                x0 += sx;
            }
            if twice <= dx {
                error += dx;
                y0 += sy;
            }
        }
    }

    fn flush(&self, buf: &mut Buffer, background: Color) {
        self.flush_at(buf, self.area.x, self.area.y, background);
    }

    fn flush_at(&self, buf: &mut Buffer, origin_x: u16, origin_y: u16, background: Color) {
        for y in 0..self.area.height {
            for x in 0..self.area.width {
                let index = usize::from(y) * usize::from(self.area.width) + usize::from(x);
                if self.dots[index] == 0 {
                    continue;
                }
                let symbol = char::from_u32(0x2800 + u32::from(self.dots[index]))
                    .unwrap_or(' ')
                    .to_string();
                if let Some(cell) = buf.cell_mut((origin_x + x, origin_y + y)) {
                    cell.set_symbol(&symbol).set_style(
                        Style::new()
                            .fg(self.colors[index].unwrap_or(background))
                            .bg(background),
                    );
                }
            }
        }
    }
}

#[derive(Clone)]
struct GhostCell {
    intensity: f32,
    symbol: String,
    color: Color,
}

impl Default for GhostCell {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            symbol: " ".to_owned(),
            color: Color::Reset,
        }
    }
}

#[derive(Default)]
struct GhostBuffer {
    area: Rect,
    cells: Vec<GhostCell>,
}

impl GhostBuffer {
    fn clear(&mut self) {
        self.area = Rect::default();
        self.cells.clear();
    }

    fn apply(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        terminal_background: Option<Color>,
    ) {
        if self.area.width != area.width || self.area.height != area.height {
            self.area = Rect::new(0, 0, area.width, area.height);
            self.cells =
                vec![GhostCell::default(); usize::from(area.width) * usize::from(area.height)];
        }
        for y in 0..area.height {
            for x in 0..area.width {
                let index = usize::from(y) * usize::from(area.width) + usize::from(x);
                let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) else {
                    continue;
                };
                let current = glyph_intensity(cell.symbol());
                let ghost = &mut self.cells[index];
                let decayed = ghost.intensity * 0.7;
                if current > 0.0 {
                    ghost.intensity = decayed.max(current);
                    ghost.symbol = cell.symbol().to_owned();
                    ghost.color = cell.fg;
                    if ghost.intensity > current {
                        cell.set_fg(mix_with_background(
                            cell.fg,
                            theme,
                            terminal_background,
                            1.0 - ghost.intensity,
                        ));
                    }
                } else {
                    ghost.intensity = decayed;
                    if ghost.intensity > 0.04 {
                        cell.set_symbol(&ghost.symbol).set_style(
                            Style::new()
                                .fg(mix_with_background(
                                    ghost.color,
                                    theme,
                                    terminal_background,
                                    1.0 - ghost.intensity,
                                ))
                                .bg(theme.bg),
                        );
                    }
                }
            }
        }
    }
}

fn glyph_intensity(symbol: &str) -> f32 {
    match symbol {
        " " => 0.0,
        "░" | "▁" | "▔" | "·" => 0.25,
        "▒" | "▂" | "▃" | "▄" | "▀" => 0.5,
        "▓" | "▅" | "▆" | "▇" => 0.75,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use rodio::buffer::SamplesBuffer;

    use super::*;

    #[test]
    fn tap_mixes_complete_stereo_frames_without_changing_audio() {
        let capture = SampleBuffer::shared();
        let source = SamplesBuffer::new(
            ChannelCount::new(2).unwrap(),
            SampleRate::new(44_100).unwrap(),
            vec![0.25, 0.75, -0.5, 0.0],
        );
        let forwarded = SampleTap::new(source, capture.clone()).collect::<Vec<_>>();

        assert_eq!(forwarded, vec![0.25, 0.75, -0.5, 0.0]);
        assert_eq!(capture.latest(8), vec![0.5, -0.25]);
    }

    #[test]
    fn analyzer_uses_fast_attack_and_slow_decay() {
        let mut analyzer = SpectrumAnalyzer::default();
        let loud = (0..FFT_SAMPLES)
            .map(|index| (2.0 * PI * 16.0 * index as f32 / FFT_SAMPLES as f32).sin())
            .collect::<Vec<_>>();
        let attacked = analyzer.process(&loud)[30];
        let attacked_again = analyzer.process(&loud)[30];
        let decayed = analyzer.process(&vec![0.0; FFT_SAMPLES])[30];

        assert!(attacked > 0.0);
        assert!((attacked_again - attacked * 1.6).abs() < 0.0001);
        assert!((decayed - attacked_again * 0.91).abs() < 0.0001);
    }

    #[test]
    fn paused_spectrum_decays_until_the_entire_area_is_empty() {
        let capture = SampleBuffer::default();
        for index in 0..FFT_SAMPLES {
            capture.push((2.0 * PI * 16.0 * index as f32 / FFT_SAMPLES as f32).sin());
        }
        let theme = Theme::db16();
        let area = Rect::new(0, 0, 24, 8);
        let mut buffer = Buffer::empty(area);
        let mut view = SpectrumView::new(SpectrumKind::Mirror);

        view.tick(&capture, true, false);
        view.render(SpectrumKind::Mirror, true, area, &mut buffer, &theme);
        assert!(buffer.content().iter().any(|cell| cell.symbol() != " "));
        let playing_peak = view.bins.iter().copied().fold(0.0_f32, f32::max);

        capture.clear();
        view.tick(&capture, false, false);
        let fading_peak = view.bins.iter().copied().fold(0.0_f32, f32::max);
        assert!(fading_peak > 0.0 && fading_peak < playing_peak);
        view.render(SpectrumKind::Mirror, true, area, &mut buffer, &theme);
        assert!(buffer.content().iter().any(|cell| cell.symbol() != " "));

        for _ in 1..200 {
            view.tick(&capture, false, false);
        }
        view.render(SpectrumKind::Mirror, true, area, &mut buffer, &theme);
        assert!(buffer.content().iter().all(|cell| cell.symbol() == " "));
    }

    #[test]
    fn glow_decays_and_keeps_a_visible_ghost() {
        let theme = Theme::db16();
        let area = Rect::new(0, 0, 2, 1);
        let mut buffer = Buffer::empty(area);
        let mut glow = GhostBuffer::default();
        put(&mut buffer, area, 0, 0, "█", theme.accent, theme.bg);
        glow.apply(area, &mut buffer, &theme, None);
        clear_area(&mut buffer, area, theme.bg);
        glow.apply(area, &mut buffer, &theme, None);

        assert_eq!(buffer[(0, 0)].symbol(), "█");
        assert!((glow.cells[0].intensity - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn transparent_glow_blends_with_detected_rgb_without_painting_the_background() {
        let theme = Theme::by_name("transparent");
        let detected = Color::Rgb(20, 30, 40);
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        let mut glow = GhostBuffer::default();
        put(&mut buffer, area, 0, 0, "█", theme.accent, theme.bg);
        glow.apply(area, &mut buffer, &theme, Some(detected));
        clear_area(&mut buffer, area, theme.bg);
        glow.apply(area, &mut buffer, &theme, Some(detected));

        assert_eq!(buffer[(0, 0)].fg, mix_color(theme.accent, detected, 0.3));
        assert_eq!(buffer[(0, 0)].bg, Color::Reset);
    }

    #[test]
    fn all_styles_render_with_test_backend() {
        let bins = (0..SPECTRUM_BINS)
            .map(|index| 0.2 + index as f32 / SPECTRUM_BINS as f32 * 0.8)
            .collect::<Vec<_>>();
        let samples = (0..FFT_SAMPLES)
            .map(|index| (2.0 * PI * 8.0 * index as f32 / FFT_SAMPLES as f32).sin() * 0.8)
            .collect::<Vec<_>>();
        for kind in SpectrumKind::ALL {
            let backend = TestBackend::new(42, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut style = style_for(kind);
            terminal
                .draw(|frame| {
                    style.render(
                        &bins,
                        &samples,
                        frame.area(),
                        frame.buffer_mut(),
                        &Theme::db16(),
                    );
                })
                .unwrap();
            let visible = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|cell| cell.symbol() != " ");
            assert!(visible, "{kind:?} rendered an empty frame");
        }
    }

    #[test]
    fn reflect_renders_separated_bricks_and_a_dimmed_reflection() {
        let theme = Theme::db16();
        let backend = TestBackend::new(12, 9);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut style = Reflect::default();

        terminal
            .draw(|frame| {
                style.render(
                    &[1.0; SPECTRUM_BINS],
                    &[],
                    frame.area(),
                    frame.buffer_mut(),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        // Bars own two thirds of the area: mirror line at 9 * 2 / 3.
        let line = 6;
        assert!((0..12).all(|x| buffer[(x, line)].symbol() == "─"));
        assert!((0..12).all(|x| buffer[(x, line)].fg == theme.faint));
        assert!((0..line).any(|y| buffer[(0, y)].symbol() == "▆"));
        assert!(
            (0..line).all(|y| (0..12).all(|x| { matches!(buffer[(x, y)].symbol(), " " | "▆") }))
        );
        assert!(((line + 1)..9)
            .all(|y| (0..12).all(|x| { matches!(buffer[(x, y)].symbol(), " " | "▆") })));
        let reflected_x = (0..12)
            .find(|&x| {
                buffer[(x, line - 1)].symbol() == "▆" && buffer[(x, line + 1)].symbol() == "▆"
            })
            .expect("reflection should keep at least one brick");
        assert_ne!(
            buffer[(reflected_x, line - 1)].fg,
            buffer[(reflected_x, line + 1)].fg
        );
    }

    #[test]
    fn led_uses_two_cell_bricks_with_one_column_gap() {
        let backend = TestBackend::new(6, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut style = Led::default();

        terminal
            .draw(|frame| {
                style.render(
                    &[1.0; SPECTRUM_BINS],
                    &[],
                    frame.area(),
                    frame.buffer_mut(),
                    &Theme::db16(),
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        for y in 1..4 {
            let symbols = (0..6).map(|x| buffer[(x, y)].symbol()).collect::<Vec<_>>();
            assert_eq!(symbols, ["▆", "▆", " ", "▆", "▆", " "]);
        }
    }

    #[test]
    fn reflect_sparkle_is_repeatable_and_transparent_safe() {
        let render = || {
            let theme = Theme::by_name("transparent");
            let backend = TestBackend::new(42, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut style = Reflect::default();
            terminal
                .draw(|frame| {
                    style.render(
                        &[1.0; SPECTRUM_BINS],
                        &[],
                        frame.area(),
                        frame.buffer_mut(),
                        &theme,
                    );
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };

        let first = render();
        let second = render();
        assert_eq!(first, second);

        let theme = Theme::by_name("transparent");
        let reflection = (6..=8).flat_map(|y| {
            let buffer = &first;
            (0..42)
                .filter(|x| x % BRICK_STRIDE < BRICK_WIDTH)
                .map(move |x| &buffer[(x, y)])
        });
        let cells = reflection.collect::<Vec<_>>();
        assert!(cells.iter().any(|cell| cell.symbol() == BRICK_GLYPH));
        assert!(cells.iter().any(|cell| cell.symbol() == " "));
        assert!(cells
            .iter()
            .filter(|cell| cell.symbol() == BRICK_GLYPH)
            .all(|cell| cell.fg == theme.faint && cell.fg != Color::Reset));
    }

    #[test]
    fn reflection_color_uses_rgb_mix_and_reset_fallback() {
        let mut theme = Theme::db16();
        theme.bg = Color::Rgb(20, 30, 40);
        assert_eq!(
            reflection_color(Color::Rgb(100, 150, 200), &theme, None),
            Color::Rgb(50, 76, 101)
        );

        theme.bg = Color::Reset;
        assert_eq!(
            reflection_color(Color::Rgb(100, 150, 200), &theme, None),
            theme.faint
        );

        assert_eq!(
            reflection_color(
                Color::Rgb(100, 150, 200),
                &theme,
                Some(Color::Rgb(20, 30, 40)),
            ),
            Color::Rgb(50, 76, 101)
        );
    }
}
