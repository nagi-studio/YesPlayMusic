use yesplaymusic_core::cache::AudioQuality;

use crate::action::{Action, View};
use crate::config::{Config, CoverMode, IconStyle};
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
    CoverPalette,
    CoverDetail,
    Layout,
    ProgressStyle,
    PixelDetail,
    QueueBehavior,
    Icons,
    SpectrumEnabled,
    SpectrumStyle,
    SpectrumGlow,
}

impl SettingField {
    pub(crate) const ALL: [Self; 15] = [
        Self::Theme,
        Self::ThemeMode,
        Self::Language,
        Self::Quality,
        Self::CoverMode,
        Self::CoverPalette,
        Self::CoverDetail,
        Self::Layout,
        Self::ProgressStyle,
        Self::PixelDetail,
        Self::QueueBehavior,
        Self::Icons,
        Self::SpectrumEnabled,
        Self::SpectrumStyle,
        Self::SpectrumGlow,
    ];

    pub(crate) const fn label(self) -> Key {
        match self {
            Self::Theme => Key::SettingTheme,
            Self::ThemeMode => Key::SettingThemeMode,
            Self::Language => Key::SettingLanguage,
            Self::Quality => Key::SettingQuality,
            Self::CoverMode => Key::SettingCoverMode,
            Self::CoverPalette => Key::SettingCoverPalette,
            Self::CoverDetail => Key::SettingCoverDetail,
            Self::Layout => Key::SettingLayout,
            Self::ProgressStyle => Key::SettingProgressStyle,
            Self::PixelDetail => Key::SettingPixelDetail,
            Self::QueueBehavior => Key::SettingQueueBehavior,
            Self::Icons => Key::SettingIcons,
            Self::SpectrumEnabled => Key::SettingSpectrumEnabled,
            Self::SpectrumStyle => Key::SettingSpectrumStyle,
            Self::SpectrumGlow => Key::SettingSpectrumGlow,
        }
    }
}

pub(crate) struct SettingsState {
    pub(crate) selected: usize,
    original: Option<Config>,
    original_theme: Option<Theme>,
    return_view: View,
    nerd_font_probe: NerdFontProbe,
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
        let last = SettingField::ALL.len() as i32 - 1;
        self.settings.selected =
            (self.settings.selected as i32 + delta.signum()).clamp(0, last) as usize;
    }

    pub(crate) fn adjust_setting(&mut self, fx: &Effects, delta: i32) {
        if self.view != View::Settings {
            return;
        }
        self.status = None;
        let before = self.config.clone();
        match SettingField::ALL[self.settings.selected] {
            SettingField::Theme => {
                self.config.theme =
                    cycle(BUILTIN_NAMES, &self.config.theme.as_str(), delta).to_owned();
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
            SettingField::ProgressStyle => {
                const VALUES: &[&str] = &["dot", "bar"];
                self.config.progress_style =
                    cycle(VALUES, &self.config.progress_style.as_str(), delta).to_owned();
            }
            SettingField::PixelDetail => {
                const VALUES: &[f32] = &[0.5, 1.0, 1.5, 2.0, 3.0, 4.0];
                self.config.pixel_scale = cycle_f32(VALUES, self.config.pixel_scale, delta);
            }
            SettingField::SpectrumEnabled => {
                self.config.spectrum_enabled = !self.config.spectrum_enabled;
            }
            SettingField::SpectrumStyle => {
                self.config.spectrum_style =
                    cycle(&SpectrumKind::ALL, &self.config.spectrum_style, delta);
            }
            SettingField::SpectrumGlow => {
                self.config.spectrum_glow = !self.config.spectrum_glow;
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
                CoverMode::Original => "Original (restart)".to_owned(),
            },
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
                "stacked" => "Stacked",
                _ => "Side",
            }
            .to_owned(),
            SettingField::ProgressStyle => match self.config.progress_style.as_str() {
                "bar" => "Bar",
                _ => "Dot",
            }
            .to_owned(),
            SettingField::PixelDetail => format!("{:.1}×", self.config.pixel_scale),
            SettingField::SpectrumEnabled => self.on_off(self.config.spectrum_enabled),
            SettingField::SpectrumStyle => {
                i18n::t_spectrum_style(self.config.spectrum_style).to_owned()
            }
            SettingField::SpectrumGlow => self.on_off(self.config.spectrum_glow),
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
        self.layout = PlayLayout::from_config(&self.config.layout);
        self.thick_progress = self.config.progress_style == "bar";
        self.pixel_detail_scale = self.config.pixel_scale.clamp(0.5, 4.0);
        self.enter_replaces_queue = self.config.enter_replaces_queue;
        if before.quality != self.config.quality {
            fx.ncm.set_quality(self.config.quality);
            self.prefetched = None;
        }

        if theme_changed {
            if let Some(original) = &mut self.original_cover {
                original.set_background(self.theme.bg);
            }
            if let Some(original) = &mut self.selected_original_cover {
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

    fn refresh_pixel_art(&mut self, fx: &Effects) {
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
