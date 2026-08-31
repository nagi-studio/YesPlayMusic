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
/// Stable, compact public projection used by the `ypm spectrum` stream.
#[cfg(any(unix, test))]
pub const REMOTE_SPECTRUM_BINS: usize = 32;
/// Highest public stream rate accepted by both CLI and socket server.
pub const REMOTE_SPECTRUM_MAX_FPS: u8 = 20;
/// Paused audio below this level no longer leaves rounded half-cells visible.
const SILENCE_THRESHOLD: f32 = 0.01;

/// Single-producer sample ring. The audio thread only performs atomic stores.
pub struct SampleBuffer {
    slots: Box<[AtomicU32]>,
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    sequence: AtomicU64,
}

impl Default for SampleBuffer {
    fn default() -> Self {
        let ring = || {
            (0..CAPTURE_SAMPLES)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect()
        };
        Self {
            slots: ring(),
            left: ring(),
            right: ring(),
            sequence: AtomicU64::new(0),
        }
    }
}

impl SampleBuffer {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn push(&self, sample: f32, left: f32, right: f32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        let index = sequence as usize % self.slots.len();
        self.slots[index].store(sample.to_bits(), Ordering::Relaxed);
        self.left[index].store(left.to_bits(), Ordering::Relaxed);
        self.right[index].store(right.to_bits(), Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Release);
    }

    pub fn clear(&self) {
        for ring in [&self.slots, &self.left, &self.right] {
            for slot in ring.iter() {
                slot.store(0.0_f32.to_bits(), Ordering::Relaxed);
            }
        }
        self.sequence.store(0, Ordering::Release);
    }

    pub fn latest(&self, wanted: usize) -> Vec<f32> {
        self.latest_from(&self.slots, wanted)
    }

    /// Latest complete frames of the first two channels; mono input
    /// duplicates into both sides. Both rings are read under one sequence
    /// snapshot so the two channels always pair the same audio frames.
    pub fn latest_stereo(&self, wanted: usize) -> (Vec<f32>, Vec<f32>) {
        for _ in 0..3 {
            let end = self.sequence.load(Ordering::Acquire);
            let count = wanted.min(end as usize).min(self.left.len());
            let start = end.saturating_sub(count as u64);
            let read = |ring: &[AtomicU32]| {
                (start..end)
                    .map(|sequence| {
                        let index = sequence as usize % ring.len();
                        f32::from_bits(ring[index].load(Ordering::Relaxed))
                    })
                    .collect::<Vec<_>>()
            };
            let left = read(&self.left);
            let right = read(&self.right);
            let after = self.sequence.load(Ordering::Acquire);
            if after.wrapping_sub(end) <= (self.left.len() - count) as u64 {
                return (left, right);
            }
        }
        (Vec::new(), Vec::new())
    }

    fn latest_from(&self, ring: &[AtomicU32], wanted: usize) -> Vec<f32> {
        for _ in 0..3 {
            let end = self.sequence.load(Ordering::Acquire);
            let count = wanted.min(end as usize).min(ring.len());
            let start = end.saturating_sub(count as u64);
            let samples = (start..end)
                .map(|sequence| {
                    let index = sequence as usize % ring.len();
                    f32::from_bits(ring[index].load(Ordering::Relaxed))
                })
                .collect::<Vec<_>>();
            let after = self.sequence.load(Ordering::Acquire);
            if after.wrapping_sub(end) <= (ring.len() - count) as u64 {
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
    frame_left: f32,
    frame_right: f32,
}

impl<S: Source> SampleTap<S> {
    pub fn new(source: S, capture: Arc<SampleBuffer>) -> Self {
        Self {
            source,
            capture,
            channel_cursor: 0,
            frame_sum: 0.0,
            frame_left: 0.0,
            frame_right: 0.0,
        }
    }
}

impl<S: Source> Iterator for SampleTap<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.source.next()?;
        let channels = self.source.channels().get();
        if self.channel_cursor == 0 {
            self.frame_left = sample;
            // Mono sources feed both sides until a second channel overwrites.
            self.frame_right = sample;
        } else if self.channel_cursor == 1 {
            self.frame_right = sample;
        }
        self.frame_sum += sample;
        self.channel_cursor += 1;
        if self.channel_cursor >= channels {
            self.capture.push(
                self.frame_sum / f32::from(channels),
                self.frame_left,
                self.frame_right,
            );
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
// Music concentrates its energy in the low bins; +4 dB per octave rebalances
// the display toward flat for typical material (the cava/Monstercat recipe).
const FLATTEN_DB_PER_OCTAVE: f32 = 4.0;
// Monstercat-style smear: each bar props up its neighbors with this decay,
// turning isolated spikes into connected hills.
const SMEAR_DECAY: f32 = 1.6;
// dB mapping (the cava/WinAmp recipe): bar height is the bin's position
// inside a fixed dB window under a slowly adapting ceiling, so mids and
// highs stay alive instead of being crushed by the loudest bass bin.

const DB_CEILING_ATTACK: f32 = 0.5;
// ~1 dB/s at the 20 Hz tick: loud passages release gently, no pumping.
const DB_CEILING_FALL: f32 = 0.05;
const DB_CEILING_MIN: f32 = -35.0;

struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    work: Vec<Complex<f32>>,
    bins: [f32; SPECTRUM_BINS],
    ceiling: f32,
    ceiling_db: f32,
    db_range: f32,
    attack: f32,
    decay: f32,
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        let mut planner = FftPlanner::new();
        Self {
            fft: planner.plan_fft_forward(FFT_SAMPLES),
            work: vec![Complex::default(); FFT_SAMPLES],
            bins: [0.0; SPECTRUM_BINS],
            ceiling: CEILING_FLOOR,
            ceiling_db: DB_CEILING_MIN,
            db_range: 35.0,
            attack: 0.4,
            decay: 0.09,
        }
    }
}

impl SpectrumAnalyzer {
    /// Window + FFT one channel, then fill `targets` with log-spaced,
    /// optionally tilt-compensated magnitudes.
    fn channel_targets(&mut self, samples: &[f32], targets: &mut [f32], flatten: bool, db: bool) {
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
        let octaves_total = high.log2();
        let count = targets.len();
        for (index, target) in targets.iter_mut().enumerate() {
            let start = high.powf(index as f32 / count as f32).floor().max(1.0) as usize;
            let end = high
                .powf((index + 1) as f32 / count as f32)
                .ceil()
                .max((start + 1) as f32) as usize;
            let mut magnitude = self.work[start..end.min(FFT_SAMPLES / 2)]
                .iter()
                .map(|value| value.norm())
                .fold(0.0_f32, f32::max)
                / FFT_SAMPLES as f32;
            if flatten {
                let octaves = (index as f32 + 0.5) / count as f32 * octaves_total;
                magnitude *= 10.0_f32.powf(FLATTEN_DB_PER_OCTAVE * octaves / 20.0);
            }
            // dB mode defers to finish(): the window mapping needs raw
            // magnitudes, the legacy path bakes its compression in here.
            *target = if db {
                magnitude
            } else {
                (magnitude * 40.0).ln_1p() / 41.0_f32.ln()
            };
        }
    }

    fn process(&mut self, samples: &[f32], flatten: bool, db: bool) -> &[f32; SPECTRUM_BINS] {
        let mut targets = [0.0_f32; SPECTRUM_BINS];
        self.channel_targets(samples, &mut targets, flatten, db);
        self.finish(targets, db)
    }

    /// Left channel mirrored into the left half (lows meeting at the
    /// center), right channel in the right half — the cava stereo layout.
    fn process_stereo(
        &mut self,
        left_samples: &[f32],
        right_samples: &[f32],
        flatten: bool,
        db: bool,
    ) -> &[f32; SPECTRUM_BINS] {
        const HALF: usize = SPECTRUM_BINS / 2;
        let mut targets = [0.0_f32; SPECTRUM_BINS];
        let mut side = [0.0_f32; HALF];
        self.channel_targets(left_samples, &mut side, flatten, db);
        for (index, value) in side.iter().enumerate() {
            targets[HALF - 1 - index] = *value;
        }
        self.channel_targets(right_samples, &mut side, flatten, db);
        targets[HALF..].copy_from_slice(&side);
        self.finish(targets, db)
    }

    fn finish(&mut self, mut targets: [f32; SPECTRUM_BINS], db: bool) -> &[f32; SPECTRUM_BINS] {
        // Two sweeps propagate each peak to its neighbors with exponential
        // falloff — equivalent to the O(n²) Monstercat smear. In dB mode the
        // targets are linear magnitudes, where /1.6 per step is the same
        // ~-4 dB/bar slope.
        for index in 1..SPECTRUM_BINS {
            targets[index] = targets[index].max(targets[index - 1] / SMEAR_DECAY);
        }
        for index in (0..SPECTRUM_BINS - 1).rev() {
            targets[index] = targets[index].max(targets[index + 1] / SMEAR_DECAY);
        }
        let frame_peak = targets.iter().copied().fold(0.0_f32, f32::max);
        if db {
            let peak_db = 20.0 * frame_peak.max(1e-6).log10();
            if peak_db > self.ceiling_db {
                self.ceiling_db += (peak_db - self.ceiling_db) * DB_CEILING_ATTACK;
            } else {
                self.ceiling_db = (self.ceiling_db - DB_CEILING_FALL).max(DB_CEILING_MIN);
            }
            let floor_db = self.ceiling_db - self.db_range;
            for (bin, target) in self.bins.iter_mut().zip(targets) {
                let level_db = 20.0 * target.max(1e-6).log10();
                let normalized = ((level_db - floor_db) / self.db_range).clamp(0.0, 1.0);
                let coefficient = if normalized > *bin {
                    self.attack
                } else {
                    self.decay
                };
                *bin += (normalized - *bin) * coefficient;
            }
            return &self.bins;
        }
        self.ceiling = (self.ceiling * CEILING_DECAY)
            .max(frame_peak)
            .max(CEILING_FLOOR);
        for (bin, target) in self.bins.iter_mut().zip(targets) {
            let normalized = (target / self.ceiling).clamp(0.0, 1.0);
            let coefficient = if normalized > *bin {
                self.attack
            } else {
                self.decay
            };
            *bin += (normalized - *bin) * coefficient;
        }
        &self.bins
    }

    fn set_sensitivity(&mut self, attack: f32, decay: f32, range_db: f32) {
        self.attack = attack;
        self.decay = decay;
        self.db_range = range_db;
    }
}

/// Render-time knobs shared by the bar-family styles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub gradient: bool,
    pub bar_width: u16,
    pub bar_gap: u16,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            gradient: true,
            bar_width: 2,
            bar_gap: 1,
        }
    }
}

impl RenderOptions {
    fn stride(&self) -> u16 {
        (self.bar_width + self.bar_gap).max(1)
    }

    fn columns(&self, width: u16) -> usize {
        usize::from(width.saturating_add(self.bar_gap) / self.stride()).max(1)
    }
}

pub trait SpectrumStyle {
    fn set_terminal_background(&mut self, _background: Option<Color>) {}

    fn set_options(&mut self, _options: RenderOptions) {}

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

    pub fn set_sensitivity(&mut self, attack: f32, decay: f32, range_db: f32) {
        self.analyzer.set_sensitivity(attack, decay, range_db);
    }

    /// Quantize the analyzer output without exposing PCM or its internal bin count.
    #[cfg(unix)]
    pub fn remote_bins(&self) -> [u8; REMOTE_SPECTRUM_BINS] {
        quantized_bins(&self.bins)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        capture: &SampleBuffer,
        playing: bool,
        preview: bool,
        flatten: bool,
        stereo: bool,
        db: bool,
    ) {
        self.ticks = self.ticks.wrapping_add(1);
        let real = capture.latest(FFT_SAMPLES);
        let live = playing && !real.is_empty();
        self.samples = if live {
            real
        } else if playing || preview {
            simulated_samples(self.ticks)
        } else {
            vec![0.0; FFT_SAMPLES]
        };
        self.bins = if stereo {
            if live {
                let (left, right) = capture.latest_stereo(FFT_SAMPLES);
                *self.analyzer.process_stereo(&left, &right, flatten, db)
            } else {
                // Simulated/silent input has no channel separation: mirror it.
                *self.analyzer.process_stereo(
                    &self.samples.clone(),
                    &self.samples.clone(),
                    flatten,
                    db,
                )
            }
        } else {
            *self.analyzer.process(&self.samples, flatten, db)
        };
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
        options: RenderOptions,
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
        self.style.set_options(options);
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

#[cfg(any(unix, test))]
fn quantized_bins(source: &[f32]) -> [u8; REMOTE_SPECTRUM_BINS] {
    let mut result = [0; REMOTE_SPECTRUM_BINS];
    if source.is_empty() {
        return result;
    }
    for (index, value) in result.iter_mut().enumerate() {
        let start = index * source.len() / REMOTE_SPECTRUM_BINS;
        let end = ((index + 1) * source.len() / REMOTE_SPECTRUM_BINS).max(start + 1);
        let peak = source[start..end.min(source.len())]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        *value = (peak.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    result
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
        SpectrumKind::Blocks => Box::new(Blocks::default()),
        SpectrumKind::Mirror => Box::new(Mirror::default()),
        SpectrumKind::Led => Box::new(Led::default()),
        SpectrumKind::Braille => Box::new(Braille::default()),
        SpectrumKind::Shade => Box::new(Shade::default()),
        SpectrumKind::Scope => Box::new(Scope::default()),
        SpectrumKind::Fire => Box::new(Fire::default()),
        SpectrumKind::Waterfall => Box::new(Waterfall::default()),
        SpectrumKind::Vu => Box::new(Vu::default()),
        SpectrumKind::Reflect => Box::new(Reflect::default()),
    }
}

#[derive(Clone, Copy)]
struct Ramp([Color; 5]);

impl Ramp {
    /// Gradient off collapses every stop to the accent: call sites keep
    /// their intensity math and the bars come out flat-colored.
    fn for_options(theme: &Theme, gradient: bool) -> Self {
        if gradient {
            Self::new(theme)
        } else {
            Self([theme.accent; 5])
        }
    }

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
// The deepest reflection row sinks this far through the remaining
// contrast; 1.0 would dissolve it into the background entirely.
const REFLECTION_FADE_RANGE: f32 = 0.8;

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
    ramp: Ramp,
) -> u16 {
    let filled = (value.clamp(0.0, 1.0) * f32::from(height)).ceil() as u16;
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

/// Reflections stay contiguous — no punched-out bricks — and read as
/// water because they sink deeper into the background with distance.
fn reflection_color(
    main: Color,
    theme: &Theme,
    terminal_background: Option<Color>,
    fade: f32,
) -> Color {
    let weight = REFLECTION_BACKGROUND_WEIGHT
        + (1.0 - REFLECTION_BACKGROUND_WEIGHT) * REFLECTION_FADE_RANGE * fade.clamp(0.0, 1.0);
    mix_with_background(main, theme, terminal_background, weight)
}

struct EighthBar {
    x: u16,
    width: u16,
    value: f32,
    height: u16,
}

fn draw_eighth_bar(buf: &mut Buffer, area: Rect, bar: EighthBar, theme: &Theme, gradient: bool) {
    let EighthBar {
        x,
        width,
        value,
        height,
    } = bar;
    let ramp = Ramp::for_options(theme, gradient);
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

#[derive(Default)]
struct Blocks {
    options: RenderOptions,
}

impl SpectrumStyle for Blocks {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let count = self.options.columns(area.width);
        for index in 0..count {
            draw_eighth_bar(
                buf,
                area,
                EighthBar {
                    x: index as u16 * self.options.stride(),
                    width: self.options.bar_width,
                    value: bin_value(bins, index, count),
                    height: area.height,
                },
                theme,
                self.options.gradient,
            );
        }
    }
}

#[derive(Default)]
struct Mirror {
    options: RenderOptions,
}

impl SpectrumStyle for Mirror {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

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
        let count = self.options.columns(area.width);
        let ramp = Ramp::for_options(theme, self.options.gradient);
        for index in 0..count {
            let level = (bin_value(bins, index, count) * f32::from(half) * 2.0).round() as u16;
            for distance in 0..half {
                let remaining = level.saturating_sub(distance * 2).min(2);
                if remaining == 0 {
                    continue;
                }
                let top_symbol = if remaining == 1 { "▄" } else { "█" };
                let bottom_symbol = if remaining == 1 { "▀" } else { "█" };
                let color = if self.options.gradient {
                    ramp.at((f32::from(distance) + 1.0) / f32::from(half))
                } else {
                    theme.accent
                };
                for offset in 0..self.options.bar_width {
                    let x = index as u16 * self.options.stride() + offset;
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
    options: RenderOptions,
}

impl SpectrumStyle for Led {
    fn set_terminal_background(&mut self, background: Option<Color>) {
        self.terminal_background = background;
    }

    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
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
        let ramp = Ramp::for_options(theme, self.options.gradient);
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

#[derive(Default)]
struct Braille {
    options: RenderOptions,
}

impl SpectrumStyle for Braille {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

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
        let ramp = Ramp::for_options(theme, self.options.gradient);
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

#[derive(Default)]
struct Shade {
    options: RenderOptions,
}

impl SpectrumStyle for Shade {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

    fn render(
        &mut self,
        bins: &[f32],
        _samples: &[f32],
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let ramp = Ramp::for_options(theme, self.options.gradient);
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

#[derive(Default)]
struct Scope {
    options: RenderOptions,
}

impl SpectrumStyle for Scope {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

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
        let ramp = Ramp::for_options(theme, self.options.gradient);
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
    options: RenderOptions,
    width: u16,
    height: u16,
    heat: Vec<f32>,
    seed: u64,
}

impl SpectrumStyle for Fire {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

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
        let ramp = Ramp::for_options(theme, self.options.gradient);
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
    options: RenderOptions,
    history: Vec<Vec<f32>>,
    tick: bool,
    width: u16,
    height: u16,
}

impl SpectrumStyle for Waterfall {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

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
        let ramp = Ramp::for_options(theme, self.options.gradient);
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
    options: RenderOptions,
    levels: [f32; 2],
}

impl SpectrumStyle for Vu {
    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

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
        let ramp = Ramp::for_options(theme, self.options.gradient);
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
    options: RenderOptions,
}

impl SpectrumStyle for Reflect {
    fn set_terminal_background(&mut self, background: Option<Color>) {
        self.terminal_background = background;
    }

    fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
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
        let ramp = Ramp::for_options(theme, self.options.gradient);
        for x in 0..area.width {
            put(buf, area, x, line, "─", theme.faint, theme.bg);
        }
        if line == 0 {
            return;
        }
        let count = brick_count(area.width);
        for index in 0..count {
            let value = bin_value(bins, index, count);
            let main_height = draw_brick_bar(
                buf,
                area,
                index as u16 * BRICK_STRIDE,
                value,
                line,
                theme,
                ramp,
            );
            let reflected = main_height
                .div_ceil(2)
                .min(area.height.saturating_sub(line + 1));
            for distance in 0..reflected {
                let source_row = (distance * 2).min(main_height.saturating_sub(1));
                let fade = f32::from(distance) / f32::from(reflected.max(1));
                let color = reflection_color(
                    brick_row_color(ramp, source_row, line),
                    theme,
                    self.terminal_background,
                    fade,
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
    fn public_bins_are_fixed_size_peak_pooled_and_quantized() {
        let mut source = [0.0_f32; SPECTRUM_BINS];
        source[0] = -1.0;
        source[1] = 1.5;
        source[SPECTRUM_BINS - 1] = 0.5;

        let projected = quantized_bins(&source);
        assert_eq!(projected.len(), REMOTE_SPECTRUM_BINS);
        assert_eq!(projected[0], 255);
        assert_eq!(projected[REMOTE_SPECTRUM_BINS - 1], 128);
        assert!(projected[1..REMOTE_SPECTRUM_BINS - 1]
            .iter()
            .all(|value| *value == 0));
    }

    #[test]
    fn gradient_off_reaches_every_ramp_style() {
        let theme = crate::theme::Theme::by_name("dracula");
        // faint/dim double as legitimate chrome (baselines, idle marks); the
        // upper ramp stops only ever come from an active gradient.
        let banned = [theme.accent2, theme.fg];
        let bins = [0.9_f32; SPECTRUM_BINS];
        let samples = vec![0.5_f32; 4096];
        for kind in SpectrumKind::ALL {
            let mut style = style_for(kind);
            style.set_options(RenderOptions {
                gradient: false,
                bar_width: 1,
                bar_gap: 0,
            });
            let area = Rect::new(0, 0, 32, 12);
            let mut buf = Buffer::empty(area);
            // History-driven styles (fire, waterfall) need a warmed frame.
            style.render(&bins, &samples, area, &mut buf, &theme);
            style.render(&bins, &samples, area, &mut buf, &theme);
            for cell in buf.content() {
                assert!(
                    !banned.contains(&cell.fg),
                    "{kind:?} leaked a gradient color with the toggle off"
                );
            }
        }
    }

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
    fn db_scale_keeps_quiet_bins_alive_where_linear_crushes_them() {
        // A loud 16-cycle tone and a -20 dB 128-cycle tone in one signal.
        let mix = (0..FFT_SAMPLES)
            .map(|index| {
                let phase = 2.0 * PI * index as f32 / FFT_SAMPLES as f32;
                0.5 * (phase * 16.0).sin() + 0.05 * (phase * 128.0).sin()
            })
            .collect::<Vec<_>>();
        let ratio = |db: bool| {
            let mut analyzer = SpectrumAnalyzer::default();
            analyzer.set_sensitivity(1.0, 1.0, 45.0);
            let mut bins = [0.0; SPECTRUM_BINS];
            // Let the dB ceiling settle like a few seconds of playback would.
            for _ in 0..40 {
                bins = *analyzer.process(&mix, false, db);
            }
            bins[51] / bins[30].max(1e-6)
        };

        // Linear peak normalization leaves the quiet tone under a third of
        // the bass; the dB window keeps it above half height.
        assert!(ratio(false) < 0.45, "linear ratio {}", ratio(false));
        assert!(ratio(true) > 0.5, "db ratio {}", ratio(true));
    }

    #[test]
    fn analyzer_uses_fast_attack_and_slow_decay() {
        let mut analyzer = SpectrumAnalyzer::default();
        let loud = (0..FFT_SAMPLES)
            .map(|index| (2.0 * PI * 16.0 * index as f32 / FFT_SAMPLES as f32).sin())
            .collect::<Vec<_>>();
        let attacked = analyzer.process(&loud, false, false)[30];
        let attacked_again = analyzer.process(&loud, false, false)[30];
        let decayed = analyzer.process(&vec![0.0; FFT_SAMPLES], false, false)[30];

        assert!(attacked > 0.0);
        assert!((attacked_again - attacked * 1.6).abs() < 0.0001);
        assert!((decayed - attacked_again * 0.91).abs() < 0.0001);
    }

    #[test]
    fn stereo_split_puts_each_channel_on_its_own_side() {
        let mut analyzer = SpectrumAnalyzer::default();
        let tone = (0..FFT_SAMPLES)
            .map(|index| (2.0 * PI * 16.0 * index as f32 / FFT_SAMPLES as f32).sin())
            .collect::<Vec<_>>();
        let silence = vec![0.0; FFT_SAMPLES];
        let bins = *analyzer.process_stereo(&tone, &silence, false, false);
        let half = SPECTRUM_BINS / 2;
        let left: f32 = bins[..half].iter().sum();
        let right: f32 = bins[half..].iter().sum();
        // The tone fed only to the left channel must dominate the left half;
        // only the smear's decayed spill crosses the center.
        assert!(left > right * 2.0, "left={left} right={right}");
    }

    #[test]
    fn flatten_tilts_highs_and_smear_props_up_neighbors() {
        let mix = (0..FFT_SAMPLES)
            .map(|index| {
                let t = index as f32 / FFT_SAMPLES as f32;
                (2.0 * PI * 16.0 * t).sin() + 0.05 * (2.0 * PI * 400.0 * t).sin()
            })
            .collect::<Vec<_>>();
        let plain = *SpectrumAnalyzer::default().process(&mix, false, false);
        let tilted = *SpectrumAnalyzer::default().process(&mix, true, false);
        let half = SPECTRUM_BINS / 2;
        let ratio = |bins: &[f32; SPECTRUM_BINS]| {
            bins[half..].iter().sum::<f32>() / bins[..half].iter().sum::<f32>().max(f32::EPSILON)
        };
        // The tilt must shift relative weight toward the high half.
        assert!(ratio(&tilted) > ratio(&plain));

        // Smear: the strongest bar props its direct neighbor up to at least
        // peak / SMEAR_DECAY (allowing smoothing rounding).
        let peak = plain
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .unwrap();
        let neighbor = if peak + 1 < SPECTRUM_BINS {
            peak + 1
        } else {
            peak - 1
        };
        assert!(plain[neighbor] >= plain[peak] / (SMEAR_DECAY * 1.01));
    }

    #[test]
    fn paused_spectrum_decays_until_the_entire_area_is_empty() {
        let capture = SampleBuffer::default();
        for index in 0..FFT_SAMPLES {
            capture.push(
                (2.0 * PI * 16.0 * index as f32 / FFT_SAMPLES as f32).sin(),
                0.0,
                0.0,
            );
        }
        let theme = Theme::db16();
        let area = Rect::new(0, 0, 24, 8);
        let mut buffer = Buffer::empty(area);
        let mut view = SpectrumView::new(SpectrumKind::Mirror);

        view.tick(&capture, true, false, true, false, false);
        view.render(
            SpectrumKind::Mirror,
            true,
            RenderOptions::default(),
            area,
            &mut buffer,
            &theme,
        );
        assert!(buffer.content().iter().any(|cell| cell.symbol() != " "));
        let playing_peak = view.bins.iter().copied().fold(0.0_f32, f32::max);

        capture.clear();
        view.tick(&capture, false, false, true, false, false);
        let fading_peak = view.bins.iter().copied().fold(0.0_f32, f32::max);
        assert!(fading_peak > 0.0 && fading_peak < playing_peak);
        view.render(
            SpectrumKind::Mirror,
            true,
            RenderOptions::default(),
            area,
            &mut buffer,
            &theme,
        );
        assert!(buffer.content().iter().any(|cell| cell.symbol() != " "));

        for _ in 1..200 {
            view.tick(&capture, false, false, true, false, false);
        }
        view.render(
            SpectrumKind::Mirror,
            true,
            RenderOptions::default(),
            area,
            &mut buffer,
            &theme,
        );
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
    fn reflect_reflection_is_contiguous_and_transparent_safe() {
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

        // The reflection is contiguous — every brick position below the
        // mirror is filled, no punched-out holes — and transparent themes
        // fall back to the faint color.
        let theme = Theme::by_name("transparent");
        let reflection = (7..=8).flat_map(|y| {
            let buffer = &first;
            (0..42)
                .filter(|x| x % BRICK_STRIDE < BRICK_WIDTH)
                .map(move |x| &buffer[(x, y)])
        });
        let cells = reflection.collect::<Vec<_>>();
        assert!(!cells.is_empty());
        assert!(cells.iter().all(|cell| cell.symbol() == BRICK_GLYPH));
        assert!(cells
            .iter()
            .all(|cell| cell.fg == theme.faint && cell.fg != Color::Reset));
    }

    #[test]
    fn reflection_color_uses_rgb_mix_and_reset_fallback() {
        let mut theme = Theme::db16();
        theme.bg = Color::Rgb(20, 30, 40);
        assert_eq!(
            reflection_color(Color::Rgb(100, 150, 200), &theme, None, 0.0),
            Color::Rgb(50, 76, 101)
        );
        // Deeper rows sink further into the background.
        let Color::Rgb(faded_red, ..) =
            reflection_color(Color::Rgb(100, 150, 200), &theme, None, 1.0)
        else {
            panic!("faded reflection stays RGB");
        };
        assert!(faded_red < 50);

        theme.bg = Color::Reset;
        assert_eq!(
            reflection_color(Color::Rgb(100, 150, 200), &theme, None, 0.0),
            theme.faint
        );

        assert_eq!(
            reflection_color(
                Color::Rgb(100, 150, 200),
                &theme,
                Some(Color::Rgb(20, 30, 40)),
                0.0,
            ),
            Color::Rgb(50, 76, 101)
        );
    }
}
