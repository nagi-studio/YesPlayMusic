use yesplaymusic_core::cache::AudioQuality;

use crate::action::{Action, View};
use crate::config::{Config, CoverMode, CoverSize, IconStyle};
use crate::i18n::{self, Key};
use crate::nerd_font::{self, Status as NerdFontStatus};
use crate::pixel::{CoverDetail, CoverPalette};
use crate::spectrum::SpectrumKind;
use crate::theme::{Theme, BUILTIN_NAMES};

use super::{spawn_render_idle, AppState, Effects, PlayLayout, PREVIEW_CELLS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingField {
    Theme,
    ThemeMode,
    Language,
    Quality,
    CoverMode,
    CoverSize,
    CoverPalette,
    CoverDetail,
    Layout,
    LyricRows,
    ProgressStyle,
    PixelDetail,
    QueueBehavior,
    Icons,
    CacheLimit,
    TitleAccent,
    SpectrumEnabled,
    SpectrumStyle,
    SpectrumGlow,
    SpectrumFlatten,
    SpectrumDb,
    SpectrumGradient,
    SpectrumBars,
    SpectrumSensitivity,
    SpectrumStereo,
    UpdateCheck,
    IntroAnimation,
}

impl SettingField {
    pub(crate) const ALL: [Self; 27] = [
        Self::Theme,
        Self::ThemeMode,
        Self::Language,
        Self::Quality,
        Self::CoverMode,
        Self::CoverSize,
        Self::CoverPalette,
        Self::CoverDetail,
        Self::Layout,
        Self::LyricRows,
        Self::ProgressStyle,
        Self::PixelDetail,
        Self::QueueBehavior,
        Self::Icons,
        Self::CacheLimit,
        Self::TitleAccent,
        Self::SpectrumEnabled,
        Self::SpectrumStyle,
        Self::SpectrumGlow,
        Self::SpectrumFlatten,
        Self::SpectrumDb,
        Self::SpectrumGradient,
        Self::SpectrumBars,
        Self::SpectrumSensitivity,
        Self::SpectrumStereo,
        Self::UpdateCheck,
        Self::IntroAnimation,
    ];

    pub(crate) const fn label(self) -> Key {
        match self {
            Self::Theme => Key::SettingTheme,
            Self::ThemeMode => Key::SettingThemeMode,
            Self::Language => Key::SettingLanguage,
            Self::Quality => Key::SettingQuality,
            Self::CoverMode => Key::SettingCoverMode,
            Self::CoverSize => Key::SettingCoverSize,
            Self::CoverPalette => Key::SettingCoverPalette,
            Self::CoverDetail => Key::SettingCoverDetail,
            Self::Layout => Key::SettingLayout,
            Self::LyricRows => Key::SettingLyricRows,
            Self::ProgressStyle => Key::SettingProgressStyle,
            Self::PixelDetail => Key::SettingPixelDetail,
            Self::QueueBehavior => Key::SettingQueueBehavior,
            Self::Icons => Key::SettingIcons,
            Self::CacheLimit => Key::SettingCacheLimit,
            Self::TitleAccent => Key::SettingTitleAccent,
            Self::SpectrumEnabled => Key::SettingSpectrumEnabled,
            Self::SpectrumStyle => Key::SettingSpectrumStyle,
            Self::SpectrumGlow => Key::SettingSpectrumGlow,
            Self::SpectrumFlatten => Key::SettingSpectrumFlatten,
            Self::SpectrumDb => Key::SettingSpectrumDb,
            Self::SpectrumGradient => Key::SettingSpectrumGradient,
            Self::SpectrumBars => Key::SettingSpectrumBars,
            Self::SpectrumSensitivity => Key::SettingSpectrumSensitivity,
            Self::SpectrumStereo => Key::SettingSpectrumStereo,
            Self::UpdateCheck => Key::SettingUpdateCheck,
            Self::IntroAnimation => Key::SettingIntroAnimation,
        }
    }
}

pub(crate) struct SettingsState {
    pub(crate) selected: usize,
    original: Option<Config>,
    original_theme: Option<Theme>,
    return_view: View,
    nerd_font_probe: NerdFontProbe,
    pub(crate) cache_usage: Option<crate::action::CacheUsage>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum NerdFontProbe {
    #[default]
    Unchecked,
    Pending,
    Ready(NerdFontStatus),
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            selected: 0,
            original: None,
            original_theme: None,
            return_view: View::NowPlaying,
            nerd_font_probe: NerdFontProbe::Unchecked,
            cache_usage: None,
        }
    }
}

impl SettingsState {
    fn begin_nerd_font_probe(&mut self) -> bool {
        if self.nerd_font_probe != NerdFontProbe::Unchecked {
            return false;
        }
        self.nerd_font_probe = NerdFontProbe::Pending;
        true
    }

    fn finish_nerd_font_probe(&mut self, status: NerdFontStatus) {
        self.nerd_font_probe = NerdFontProbe::Ready(status);
    }

    fn nerd_font_status(&self) -> Option<NerdFontStatus> {
        match self.nerd_font_probe {
            NerdFontProbe::Ready(status) => Some(status),
            NerdFontProbe::Unchecked | NerdFontProbe::Pending => None,
        }
    }
}

impl AppState {
    pub(crate) fn open_settings(&mut self, fx: &Effects) {
        if self.view == View::Settings {
            return;
        }
        self.settings.original = Some(self.config.clone());
        self.settings.original_theme = Some(self.theme);
        self.settings.return_view = self.view;
        self.settings.selected = 0;
        self.view = View::Settings;
        self.zen = false;
        self.status = None;
        self.start_nerd_font_probe(fx);
        self.start_cache_usage_probe(fx);
    }

    /// Snapshot both stores off the main thread; the settings page shows the
    /// numbers next to the limit so the cap is a fact, not a promise.
    fn start_cache_usage_probe(&mut self, fx: &Effects) {
        self.settings.cache_usage = None;
        let audio_root = fx.cache_root.clone();
        let covers = fx.covers.clone();
        let actions = fx.actions.clone();
        tokio::spawn(async move {
            let usage = tokio::task::spawn_blocking(move || {
                let (audio_used, audio_max) = audio_root
                    .and_then(|root| yesplaymusic_core::cache::TrackCache::open(root).ok())
                    .map(|cache| {
                        (
                            cache.total_bytes().unwrap_or(0),
                            cache.max_bytes().unwrap_or(0),
                        )
                    })
                    .unwrap_or((0, 0));
                let (cover_used, cover_max) = covers
                    .map(|cache| (cache.used_bytes(), cache.budget_bytes()))
                    .unwrap_or((0, 0));
                crate::action::CacheUsage {
                    audio_used,
                    audio_max,
                    cover_used,
                    cover_max,
                }
            })
            .await;
            if let Ok(usage) = usage {
                let _ = actions.send(Action::CacheUsageProbed(usage));
            }
        });
    }

    pub(crate) fn apply_cache_usage(&mut self, usage: crate::action::CacheUsage) {
        self.settings.cache_usage = Some(usage);
    }

    fn start_nerd_font_probe(&mut self, fx: &Effects) {
        if !self.settings.begin_nerd_font_probe() {
            return;
        }
        let actions = fx.actions.clone();
        tokio::spawn(async move {
            let status = tokio::task::spawn_blocking(nerd_font::detect)
                .await
                .unwrap_or(NerdFontStatus::Unknown);
            let _ = actions.send(Action::NerdFontProbeFinished(status));
        });
    }

    pub(crate) fn apply_nerd_font_probe(&mut self, status: NerdFontStatus) {
        self.settings.finish_nerd_font_probe(status);
    }

    pub(crate) fn nerd_font_status(&self) -> Option<NerdFontStatus> {
        self.settings.nerd_font_status()
    }

    pub(crate) fn select_setting(&mut self, index: usize) {
        if self.view == View::Settings && index < SettingField::ALL.len() {
            self.settings.selected = index;
        }
    }

    pub(crate) fn move_setting_selection(&mut self, delta: i32) {
        if self.view != View::Settings {
            return;
        }
        // Wrap around: up from the first row lands on the last and back.
        let count = SettingField::ALL.len() as i32;
        self.settings.selected =
            (self.settings.selected as i32 + delta.signum()).rem_euclid(count) as usize;
    }

    pub(crate) fn adjust_setting(&mut self, fx: &Effects, delta: i32) {
        if self.view != View::Settings {
            return;
        }
        self.status = None;
        let before = self.config.clone();
        match SettingField::ALL[self.settings.selected] {
            SettingField::Theme => {
                // Offer only names canonical under the active appearance;
                // otherwise the paired variants resolve straight back and
                // the row has stops where pressing → visibly does nothing.
                let names: Vec<&str> = BUILTIN_NAMES
                    .iter()
                    .copied()
                    .filter(|name| {
                        crate::theme::resolved_name(
                            name,
                            self.config.theme_mode,
                            self.terminal_is_light,
                        ) == *name
                    })
                    .collect();
                self.config.theme = cycle(&names, &self.config.theme.as_str(), delta).to_owned();
            }
            SettingField::ThemeMode => {
                use crate::config::ThemeMode;
                const VALUES: &[ThemeMode] = &[ThemeMode::Auto, ThemeMode::Dark, ThemeMode::Light];
                self.config.theme_mode = cycle(VALUES, &self.config.theme_mode, delta);
            }
            SettingField::Language => {
                const VALUES: &[&str] = &["zh", "en", "ja"];
                self.config.language =
                    cycle(VALUES, &self.config.language.as_str(), delta).to_owned();
            }
            SettingField::Quality => {
                const VALUES: &[AudioQuality] = &[
                    AudioQuality::Low128,
                    AudioQuality::Medium192,
                    AudioQuality::High320,
                    AudioQuality::Lossless,
                    AudioQuality::HiRes,
                ];
                self.config.quality = cycle(VALUES, &self.config.quality, delta);
            }
            SettingField::CoverMode => {
                const VALUES: &[CoverMode] = &[CoverMode::Pixel, CoverMode::Original];
                self.config.cover_mode = cycle(VALUES, &self.config.cover_mode, delta);
            }
            SettingField::CoverSize => {
                self.config.cover_size = cycle(&CoverSize::ALL, &self.config.cover_size, delta);
            }
            SettingField::CoverPalette => {
                const VALUES: &[CoverPalette] = &[CoverPalette::Original, CoverPalette::Theme];
                self.config.cover_palette = cycle(VALUES, &self.config.cover_palette, delta);
            }
            SettingField::CoverDetail => {
                const VALUES: &[CoverDetail] = &[
                    CoverDetail::Half,
                    CoverDetail::Quad,
                    CoverDetail::Sextant,
                    CoverDetail::Octant,
                ];
                self.config.cover_detail = cycle(VALUES, &self.config.cover_detail, delta);
            }
            SettingField::Layout => {
                const VALUES: &[&str] = &["side", "stacked"];
                self.config.layout = cycle(VALUES, &self.config.layout.as_str(), delta).to_owned();
            }
            SettingField::LyricRows => {
                // Odd presets keep the current line visually centered; larger
                // custom values live in the config file only.
                const VALUES: &[Option<u16>] =
                    &[None, Some(3), Some(5), Some(7), Some(9), Some(13), Some(17)];
                self.config.lyric_rows = cycle(VALUES, &self.config.lyric_rows, delta);
            }
            SettingField::ProgressStyle => {
                const VALUES: &[&str] = &["dot", "bar"];
                self.config.progress_style =
                    cycle(VALUES, &self.config.progress_style.as_str(), delta).to_owned();
            }
            SettingField::PixelDetail => {
                const VALUES: &[f32] = &[0.5, 1.0, 1.5, 2.0, 3.0, 4.0];
                self.config.pixel_scale = cycle_f32(VALUES, self.config.pixel_scale, delta);
            }
            SettingField::CacheLimit => {
                // Presets stop at 128G; larger custom values live in the
                // config file only.
                const VALUES: &[u64] = &[1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072];
                let current = self.config.cache_limit_mib.unwrap_or(8192);
                self.config.cache_limit_mib = Some(cycle(VALUES, &current, delta));
            }
            SettingField::TitleAccent => {
                self.config.title_accent = !self.config.title_accent;
            }
            SettingField::SpectrumEnabled => {
                self.config.spectrum_enabled = !self.config.spectrum_enabled;
            }
            SettingField::SpectrumStyle => {
                self.config.spectrum_style =
                    cycle(&SpectrumKind::ALL, &self.config.spectrum_style, delta);
            }
            SettingField::SpectrumFlatten => {
                self.config.spectrum_flatten = !self.config.spectrum_flatten;
            }
            SettingField::SpectrumDb => {
                self.config.spectrum_db = !self.config.spectrum_db;
            }
            SettingField::SpectrumGradient => {
                self.config.spectrum_gradient = !self.config.spectrum_gradient;
            }
            SettingField::SpectrumBars => {
                use crate::config::SpectrumBars;
                self.config.spectrum_bars =
                    cycle(&SpectrumBars::ALL, &self.config.spectrum_bars, delta);
            }
            SettingField::SpectrumSensitivity => {
                use crate::config::SpectrumSensitivity;
                self.config.spectrum_sensitivity = cycle(
                    &SpectrumSensitivity::ALL,
                    &self.config.spectrum_sensitivity,
                    delta,
                );
            }
            SettingField::SpectrumStereo => {
                self.config.spectrum_stereo = !self.config.spectrum_stereo;
            }
            SettingField::SpectrumGlow => {
                self.config.spectrum_glow = !self.config.spectrum_glow;
            }
            SettingField::UpdateCheck => {
                self.config.update_check = !self.config.update_check;
            }
            SettingField::IntroAnimation => {
                use crate::config::IntroStyle;
                self.config.intro_animation =
                    cycle(&IntroStyle::ALL, &self.config.intro_animation, delta);
                self.start_intro_preview();
            }
            SettingField::QueueBehavior => {
                self.config.enter_replaces_queue = !self.config.enter_replaces_queue;
            }
            SettingField::Icons => {
                const VALUES: &[IconStyle] = &[IconStyle::Unicode, IconStyle::Nerd];
                self.config.icons = cycle(VALUES, &self.config.icons, delta);
            }
        }
        self.apply_config_preview(fx, &before, None);
    }

    pub(crate) fn save_settings(&mut self, fx: &Effects) {
        if self.view != View::Settings {
            return;
        }
        match self.config.save_to(&fx.config_path) {
            Ok(()) => {
                self.settings.original = None;
                self.settings.original_theme = None;
                self.view = self.settings.return_view;
                self.status = Some(i18n::t(Key::SettingsSaved).to_owned());
            }
            Err(error) => {
                self.status = Some(format!("{}: {error}", i18n::t(Key::SettingsSaveFailed)));
            }
        }
    }

    pub(crate) fn cancel_settings(&mut self, fx: &Effects) {
        if self.view != View::Settings {
            return;
        }
        if let Some(original) = self.settings.original.take() {
            let before = self.config.clone();
            self.config = original;
            let original_theme = self.settings.original_theme.take();
            self.apply_config_preview(fx, &before, original_theme);
        }
        self.view = self.settings.return_view;
        self.status = None;
    }

    pub(crate) fn setting_value(&self, field: SettingField) -> String {
        match field {
            SettingField::Theme => crate::theme::resolved_name(
                &self.config.theme,
                self.config.theme_mode,
                self.terminal_is_light,
            )
            .to_owned(),
            SettingField::ThemeMode => {
                use crate::config::ThemeMode;
                match self.config.theme_mode {
                    ThemeMode::Auto => match self.terminal_is_light {
                        Some(true) => "auto → light",
                        Some(false) => "auto → dark",
                        None => "auto",
                    },
                    ThemeMode::Dark => "dark",
                    ThemeMode::Light => "light",
                }
                .to_owned()
            }
            SettingField::Language => match i18n::Lang::from_config(&self.config.language) {
                i18n::Lang::Zh => "中文",
                i18n::Lang::En => "English",
                i18n::Lang::Ja => "日本語",
            }
            .to_owned(),
            SettingField::Quality => match self.config.quality {
                AudioQuality::Low128 => "128 kbps",
                AudioQuality::Medium192 => "192 kbps",
                AudioQuality::High320 => "320 kbps",
                AudioQuality::Lossless => "Lossless",
                AudioQuality::HiRes => "Hi-Res",
            }
            .to_owned(),
            SettingField::CoverMode => match self.config.cover_mode {
                CoverMode::Pixel => "Pixel".to_owned(),
                CoverMode::Original if self.original_cover.is_some() => "Original".to_owned(),
                // The startup probe already ran: a missing picker means the
                // terminal genuinely lacks a graphics protocol.
                CoverMode::Original => "Original (unsupported)".to_owned(),
            },
            SettingField::CoverSize => match self.config.cover_size {
                CoverSize::Compact => "Compact",
                CoverSize::Auto => "Auto",
                CoverSize::Large => "Large",
            }
            .to_owned(),
            SettingField::CoverPalette => match self.config.cover_palette {
                CoverPalette::Original => "Original",
                CoverPalette::Theme => "Theme",
            }
            .to_owned(),
            SettingField::CoverDetail if self.config.cover_detail == CoverDetail::Octant => {
                format!("octant · {}", i18n::t(Key::OctantFontRequired))
            }
            SettingField::CoverDetail => self.config.cover_detail.as_str().to_owned(),
            SettingField::Layout => match self.config.layout.as_str() {
                "side" => "Side",
                "stacked" => "Stacked",
                value => unreachable!("layout was validated before opening settings: {value}"),
            }
            .to_owned(),
            SettingField::LyricRows => match self.config.lyric_rows {
                Some(rows) => rows.to_string(),
                None => format!("auto ({})", i18n::t(Key::SettingDefaultTag)),
            },
            SettingField::ProgressStyle => match self.config.progress_style.as_str() {
                "dot" => "Dot",
                "bar" => "Bar",
                value => {
                    unreachable!("progress style was validated before opening settings: {value}")
                }
            }
            .to_owned(),
            SettingField::PixelDetail => format!("{:.1}×", self.config.pixel_scale),
            SettingField::CacheLimit => match self.config.cache_limit_mib {
                Some(limit_mib) => format!("{}G", limit_mib.div_ceil(1024)),
                None => format!("8G ({})", i18n::t(Key::SettingDefaultTag)),
            },
            SettingField::TitleAccent => self.on_off(self.config.title_accent),
            SettingField::SpectrumEnabled => self.on_off(self.config.spectrum_enabled),
            SettingField::SpectrumStyle => {
                i18n::t_spectrum_style(self.config.spectrum_style).to_owned()
            }
            SettingField::SpectrumGlow => self.on_off(self.config.spectrum_glow),
            SettingField::SpectrumFlatten => self.on_off(self.config.spectrum_flatten),
            SettingField::SpectrumDb => self.on_off(self.config.spectrum_db),
            SettingField::SpectrumGradient => self.on_off(self.config.spectrum_gradient),
            SettingField::SpectrumBars => {
                i18n::t_spectrum_bars(self.config.spectrum_bars).to_owned()
            }
            SettingField::SpectrumSensitivity => {
                i18n::t_spectrum_sensitivity(self.config.spectrum_sensitivity).to_owned()
            }
            SettingField::SpectrumStereo => self.on_off(self.config.spectrum_stereo),
            SettingField::UpdateCheck => self.on_off(self.config.update_check),
            SettingField::IntroAnimation => {
                i18n::intro_style_name(self.config.intro_animation).to_owned()
            }
            SettingField::QueueBehavior => if self.config.enter_replaces_queue {
                i18n::t(Key::SettingQueueList)
            } else {
                i18n::t(Key::SettingQueueSingle)
            }
            .to_owned(),
            SettingField::Icons => match self.config.icons {
                IconStyle::Unicode => "Unicode",
                IconStyle::Nerd => "Nerd Font",
            }
            .to_owned(),
        }
    }

    fn on_off(&self, enabled: bool) -> String {
        i18n::t(if enabled { Key::On } else { Key::Off }).to_owned()
    }

    /// Cycling the intro style plays the chosen animation immediately as a
    /// preview; any key dismisses it. The same call starts the launch intro,
    /// so what the settings page previews is exactly what startup plays.
    pub(crate) fn start_intro_preview(&mut self) {
        use crate::config::IntroStyle;
        let style = self.config.intro_animation;
        if style == IntroStyle::Off {
            self.intro_preview = None;
            return;
        }
        // Transparent themes resolve to the detected terminal background,
        // so fades rise out of the colour actually behind the page instead
        // of assuming black under a light terminal.
        let bg = match self.theme.resolved_background(self.terminal_background) {
            Some(ratatui::style::Color::Rgb(r, g, b)) => (r, g, b),
            _ => (0, 0, 0),
        };
        let (cols, rows) = self.terminal_size;
        // A terminal too small for the mark gets no animation rather than a
        // clipped one; the dashboard's own art handles tiny sizes already.
        let spec = self.intro_mark_spec();
        if usize::from(rows) < spec.side / 2 + 2 || usize::from(cols) < spec.side + 2 {
            self.intro_preview = None;
            return;
        }
        self.intro_preview =
            crate::logo::sequence(style, bg, crate::logo::notes_pads(cols, rows), &spec).map(
                |seq| super::IntroPreview {
                    seq,
                    started: std::time::Instant::now(),
                },
            );
    }

    pub(crate) fn toggle_spectrum(&mut self, fx: &Effects) -> Result<(), String> {
        let before = self.config.clone();
        self.config.spectrum_enabled = !self.config.spectrum_enabled;
        self.apply_config_preview(fx, &before, None);
        let persistent = if let Some(original) = &mut self.settings.original {
            original.spectrum_enabled = self.config.spectrum_enabled;
            original.clone()
        } else {
            self.config.clone()
        };
        persistent
            .save_to(&fx.config_path)
            .map_err(|error| format!("{}: {error}", i18n::t(Key::SettingsSaveFailed)))
    }

    pub(crate) fn set_command_theme(
        &mut self,
        fx: &Effects,
        theme_name: &str,
    ) -> Result<(), String> {
        let before = self.config.clone();
        self.config.theme = theme_name.to_owned();
        self.apply_config_preview(fx, &before, None);

        let persistent = if let Some(original) = &mut self.settings.original {
            original.theme.clone_from(&self.config.theme);
            self.settings.original_theme = Some(self.theme);
            original.clone()
        } else {
            self.config.clone()
        };
        persistent
            .save_to(&fx.config_path)
            .map_err(|error| format!("{}: {error}", i18n::t(Key::SettingsSaveFailed)))
    }

    pub(crate) fn set_command_quality(
        &mut self,
        fx: &Effects,
        quality: AudioQuality,
    ) -> Result<(), String> {
        let before = self.config.clone();
        self.config.quality = quality;
        self.apply_config_preview(fx, &before, None);

        let persistent = if let Some(original) = &mut self.settings.original {
            original.quality = quality;
            original.clone()
        } else {
            self.config.clone()
        };
        persistent
            .save_to(&fx.config_path)
            .map_err(|error| format!("{}: {error}", i18n::t(Key::SettingsSaveFailed)))
    }

    fn apply_config_preview(
        &mut self,
        fx: &Effects,
        before: &Config,
        restored_theme: Option<Theme>,
    ) {
        let theme_changed =
            before.theme != self.config.theme || before.theme_mode != self.config.theme_mode;
        let pixel_changed = (before.pixel_scale - self.config.pixel_scale).abs() > f32::EPSILON
            || before.cover_detail != self.config.cover_detail
            || before.cover_palette != self.config.cover_palette;
        // The spectrum only claims cover rows in the stacked layout's
        // bottom band; in the side layout it lives in the right panel, so
        // toggling it must not reload the cover (visible vinyl flash).
        let layout_changed = before.layout != self.config.layout
            || before.cover_size != self.config.cover_size
            || (before.spectrum_enabled != self.config.spectrum_enabled
                && self.layout == PlayLayout::Stacked);

        if theme_changed {
            self.theme = restored_theme.unwrap_or_else(|| {
                Theme::by_name(crate::theme::resolved_name(
                    &self.config.theme,
                    self.config.theme_mode,
                    self.terminal_is_light,
                ))
            });
        }
        if before.language != self.config.language {
            i18n::set_language(i18n::Lang::from_config(&self.config.language));
        }
        self.layout = PlayLayout::from_config(&self.config.layout);
        self.thick_progress = super::thick_progress_from_config(&self.config.progress_style);
        self.pixel_detail_scale = self.config.pixel_scale;
        self.enter_replaces_queue = self.config.enter_replaces_queue;
        if before.quality != self.config.quality {
            fx.ncm.set_quality(self.config.quality);
            self.prefetched = None;
        }
        if before.cache_limit_mib != self.config.cache_limit_mib {
            if let (Some(root), Some(limit_mib)) =
                (fx.cache_root.clone(), self.config.cache_limit_mib)
            {
                // Eviction scans the store; keep it off the UI thread.
                std::thread::spawn(move || {
                    let updated =
                        yesplaymusic_core::cache::TrackCache::open(root).and_then(|cache| {
                            match limit_mib.checked_mul(1024 * 1024) {
                                Some(max_bytes) => cache.set_max_bytes(max_bytes),
                                None => Ok(()),
                            }
                        });
                    if let Err(error) = updated {
                        tracing::warn!(%error, "cache limit update failed");
                    }
                });
            }
        }

        if theme_changed {
            if let Some(original) = &mut self.original_cover {
                original.set_background(self.theme.bg);
            }
            if let Some(original) = &mut self.selected_original_cover {
                original.set_background(self.theme.bg);
            }
            if let Some(original) = &mut self.bar_original_cover {
                original.set_background(self.theme.bg);
            }
        }
        if theme_changed
            || pixel_changed
            || layout_changed
            || before.cover_mode != self.config.cover_mode
        {
            self.refresh_pixel_art(fx);
        }
    }

    pub(super) fn refresh_pixel_art(&mut self, fx: &Effects) {
        self.style_revision = self.style_revision.wrapping_add(1);
        let desired = self.desired_cover_cells();
        if self.now.is_some() {
            self.placeholder = Some(crate::pixel::vinyl(
                self.theme.palette,
                self.theme.bg,
                desired.0,
                desired.1,
                self.config.cover_detail,
            ));
        }
        self.selected_cover.placeholder = crate::pixel::vinyl(
            self.theme.palette,
            self.theme.bg,
            PREVIEW_CELLS.0,
            PREVIEW_CELLS.1,
            self.config.cover_detail,
        );
        self.cover = None;
        self.bar_cover = None;
        if let Some(row) = self.active_row.clone() {
            self.load_playing_cover(fx, &row);
        }
        if let Some(bytes) = self.idle_bytes.clone() {
            spawn_render_idle(
                fx,
                bytes,
                self.desired_idle_cells(),
                self.pixel_style(),
                self.style_revision,
            );
        }
    }
}

fn cycle<T: Copy + PartialEq>(values: &[T], current: &T, delta: i32) -> T {
    let current = values.iter().position(|value| value == current);
    let index = match (current, delta.is_negative()) {
        (Some(index), false) => (index + 1) % values.len(),
        (Some(0), true) | (None, true) => values.len() - 1,
        (Some(index), true) => index - 1,
        (None, false) => 0,
    };
    values[index]
}

fn cycle_f32(values: &[f32], current: f32, delta: i32) -> f32 {
    let index = values
        .iter()
        .position(|value| (*value - current).abs() < f32::EPSILON);
    match (index, delta.is_negative()) {
        (Some(index), false) => values[(index + 1) % values.len()],
        (Some(0), true) | (None, true) => values[values.len() - 1],
        (Some(index), true) => values[index - 1],
        (None, false) => values[0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nerd_font_probe_starts_once_and_keeps_the_completed_result() {
        let mut settings = SettingsState::default();

        assert!(settings.begin_nerd_font_probe());
        assert!(!settings.begin_nerd_font_probe());

        settings.finish_nerd_font_probe(NerdFontStatus::Detected);
        assert_eq!(settings.nerd_font_status(), Some(NerdFontStatus::Detected));
        assert!(!settings.begin_nerd_font_probe());
    }
}
