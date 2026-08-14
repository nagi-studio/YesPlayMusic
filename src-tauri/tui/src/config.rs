//! ~/.config/ypm/config.toml — strong defaults, every field optional.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tempfile::NamedTempFile;
use yesplaymusic_core::cache::AudioQuality;

use crate::pixel::{CoverDetail, CoverPalette};
use crate::spectrum::SpectrumKind;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverMode {
    #[default]
    Pixel,
    Original,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IconStyle {
    #[default]
    Unicode,
    Nerd,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the terminal background detected over OSC 11; unknown = dark.
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Config {
    /// UI language: zh | en | ja
    #[serde(deserialize_with = "deserialize_language")]
    pub language: String,
    /// NCM quality level: 128 | 192 | 320/exhigh | lossless | hires
    #[serde(
        deserialize_with = "deserialize_quality",
        serialize_with = "serialize_quality"
    )]
    pub quality: AudioQuality,
    /// Try UNM when NCM explicitly returns no playable URL.
    pub unm_enabled: bool,
    /// Built-in theme name or a TOML file in themes/.
    pub theme: String,
    /// Swap paired themes to their light/dark counterpart; Auto follows
    /// the terminal background. Themes without a pair ignore this.
    pub theme_mode: ThemeMode,
    /// Explicit ypm process cache cap in MiB. None keeps the database value.
    pub cache_limit_mib: Option<u64>,
    /// Enter on a list: true = the list becomes the queue from that song
    /// (desktop/NCM semantics), false = play just that one song.
    pub enter_replaces_queue: bool,
    /// Playing layout: "side" = cover fills the height, lyrics beside;
    /// "stacked" = cover centered on top, lyrics below.
    pub layout: String,
    /// Optional image for the idle dashboard; run through the same pixel
    /// pipeline as covers. Falls back to the built-in vinyl.
    pub idle_art: Option<String>,
    /// Progress bar look: "dot" = thin line with a playhead dot,
    /// "bar" = thick block line, no dot.
    pub progress_style: String,
    /// UI glyph palette; Nerd requires a Nerd Font in the terminal.
    pub icons: IconStyle,
    /// Cover renderer: palette pixel art or terminal graphics protocol.
    pub cover_mode: CoverMode,
    /// Pixel cover colors: source truecolor or the active theme palette.
    pub cover_palette: CoverPalette,
    /// Sub-cell glyph resolution used by the pixel renderer.
    pub cover_detail: CoverDetail,
    /// Pixel sampling detail (0.5 chunky … 4.0 fine). The final cell
    /// footprint is unchanged.
    pub pixel_scale: f32,
    /// Retro spectrum visibility, renderer, and phosphor afterglow.
    pub spectrum_enabled: bool,
    pub spectrum_style: SpectrumKind,
    pub spectrum_glow: bool,
    /// Silent GitHub release check on startup; hints only, never self-updates.
    pub update_check: bool,
}

pub struct LoadedConfig {
    pub config: Config,
    pub theme_is_explicit: bool,
}

impl LoadedConfig {
    pub fn should_apply_terminal_brightness(&self) -> bool {
        !self.theme_is_explicit
    }

    pub fn apply_terminal_brightness(&mut self, is_light: Option<bool>) {
        if self.theme_is_explicit {
            return;
        }
        let Some(is_light) = is_light else { return };
        self.config.theme = if is_light { "fairyfloss" } else { "db16" }.into();
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "zh".into(),
            quality: AudioQuality::High320,
            unm_enabled: true,
            theme: "db16".into(),
            theme_mode: ThemeMode::default(),
            cache_limit_mib: None,
            enter_replaces_queue: true,
            layout: "side".into(),
            idle_art: None,
            progress_style: "dot".into(),
            icons: IconStyle::Unicode,
            cover_mode: CoverMode::Pixel,
            cover_palette: CoverPalette::Original,
            cover_detail: CoverDetail::Half,
            pixel_scale: 1.0,
            spectrum_enabled: false,
            spectrum_style: SpectrumKind::Blocks,
            spectrum_glow: false,
            update_check: true,
        }
    }
}

impl Config {
    /// Missing or invalid config falls back to defaults — the TUI must
    /// always start. A missing file gets a commented template so the
    /// options are discoverable without reading docs.
    pub fn load_with_metadata() -> LoadedConfig {
        let path = config_dir().join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => parse_with_metadata(&text),
            Err(_) => {
                let _ = write_template(&path);
                LoadedConfig {
                    config: Self::default(),
                    theme_is_explicit: false,
                }
            }
        }
    }

    /// Write the complete config through a same-directory temporary file,
    /// then atomically replace the destination.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;

        let contents = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(contents.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        sync_directory(parent)
    }
}

fn parse_with_metadata(text: &str) -> LoadedConfig {
    let Ok(value) = toml::from_str::<toml::Value>(text) else {
        return LoadedConfig {
            config: Config::default(),
            theme_is_explicit: false,
        };
    };
    let theme_is_explicit = value
        .as_table()
        .is_some_and(|table| table.contains_key("theme"));
    LoadedConfig {
        config: value.try_into().unwrap_or_default(),
        theme_is_explicit,
    }
}

const TEMPLATE: &str = r#"# ypm 配置 — 常用项也可在 ypm 设置页修改；手动编辑后重启生效。
# 所有项都可省略（使用默认值）。

# language = "zh"            # zh | en | ja
# quality = "exhigh"          # 128 | 192 | 320/exhigh | lossless | hires
# unm_enabled = true          # 无版权/VIP 曲目无可播地址时尝试 UNM 换源
# theme = "db16"              # db16 | pico8 | gameboy | everforest | tokyo-night | tokyo-night-storm
#                              # one-dark | one-dark-pro | dracula | synthwave84 | laserwave | fairyfloss | ultraviolence | transparent
#                              # 省略时：暗色终端用 db16，亮色终端用 fairyfloss
# layout = "side"             # side（封面撑满高度）| stacked（封面居中在上）
# progress_style = "dot"      # dot（细线+圆点）| bar（粗块）
# icons = "unicode"           # unicode（无需特殊字体）| nerd（需要 Nerd Font）
# cover_mode = "pixel"        # pixel（字符像素画）| original（终端原图协议，不支持时回退 pixel）
# cover_palette = "original"  # original（原图真彩）| theme（主题配色）
# cover_detail = "half"       # half（1×2）| quad（2×2）| sextant（2×3）| octant（2×4，需 Unicode 16 字体）
#                              # octant 显示方框时请切回 sextant
# enter_replaces_queue = true # Enter：整列表成为队列；false = 只播这一首
# idle_art = "~/my-art.png"   # 开屏像素画（png/jpg/webp/gif，自动像素化）
# cache_limit_mib = 8192       # 仅显式设置时更新 ypm 进程共享的上限
# pixel_scale = 1.0            # 像素细腻度：0.5 更复古块状，4.0 更细腻
# spectrum_enabled = false     # v 键也可全局开关
# spectrum_style = "blocks"   # blocks | mirror | led | braille | shade | scope | fire | waterfall | vu | reflect
# spectrum_glow = false        # 荧光余辉残影
# update_check = true          # 启动时静默检查新版本
"#;

fn deserialize_language<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "zh" | "en" | "ja" => value,
        _ => "zh".into(),
    })
}

#[derive(Deserialize)]
#[serde(untagged)]
enum QualitySetting {
    Name(String),
    Number(u32),
}

fn deserialize_quality<'de, D>(deserializer: D) -> Result<AudioQuality, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let setting = QualitySetting::deserialize(deserializer)?;
    let quality = match setting {
        QualitySetting::Name(name) => match name.as_str() {
            "128" => Some(AudioQuality::Low128),
            "192" => Some(AudioQuality::Medium192),
            "320" | "exhigh" => Some(AudioQuality::High320),
            "lossless" => Some(AudioQuality::Lossless),
            "hires" => Some(AudioQuality::HiRes),
            _ => None,
        },
        QualitySetting::Number(number) => match number {
            128 => Some(AudioQuality::Low128),
            192 => Some(AudioQuality::Medium192),
            320 => Some(AudioQuality::High320),
            _ => None,
        },
    };
    quality.ok_or_else(|| D::Error::custom("unsupported audio quality"))
}

fn serialize_quality<S>(quality: &AudioQuality, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(match quality {
        AudioQuality::Low128 => "128",
        AudioQuality::Medium192 => "192",
        AudioQuality::High320 => "exhigh",
        AudioQuality::Lossless => "lossless",
        AudioQuality::HiRes => "hires",
    })
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn write_template(path: &std::path::Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, TEMPLATE)
}

/// All platforms use the ~/.config style directory expected by TUI users.
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/ypm")
}

pub fn cache_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache/ypm")
}

pub fn state_dir() -> PathBuf {
    cache_dir().join("state")
}

pub fn session_path() -> PathBuf {
    config_dir().join("session.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_hold_when_config_is_missing_or_broken() {
        let config = Config::default();
        assert_eq!(config.language, "zh");
        assert_eq!(config.quality, AudioQuality::High320);
        assert!(config.unm_enabled);
        assert_eq!(config.theme, "db16");
        assert_eq!(config.cover_mode, CoverMode::Pixel);
        assert_eq!(config.cover_palette, CoverPalette::Original);
        assert_eq!(config.cover_detail, CoverDetail::Half);
        assert_eq!(config.icons, IconStyle::Unicode);
        assert_eq!(config.cache_limit_mib, None);
        assert!(!config.spectrum_enabled);
        assert_eq!(config.spectrum_style, SpectrumKind::Blocks);
        assert!(!config.spectrum_glow);

        let parsed: Config = toml::from_str("quality = \"lossless\"").unwrap();
        assert_eq!(parsed.quality, AudioQuality::Lossless);
        assert_eq!(parsed.theme, "db16");

        let parsed: Config = toml::from_str("language = \"ja\"").unwrap();
        assert_eq!(parsed.language, "ja");
        let parsed: Config = toml::from_str("language = \"fr\"").unwrap();
        assert_eq!(parsed.language, "zh");

        let parsed: Config = toml::from_str("cover_mode = \"original\"").unwrap();
        assert_eq!(parsed.cover_mode, CoverMode::Original);

        let parsed: Config = toml::from_str("cover_palette = \"theme\"").unwrap();
        assert_eq!(parsed.cover_palette, CoverPalette::Theme);

        let migrated: Config = toml::from_str("cover_mode = \"pixel\"").unwrap();
        assert_eq!(migrated.cover_palette, CoverPalette::Original);

        let parsed: Config = toml::from_str("icons = \"nerd\"").unwrap();
        assert_eq!(parsed.icons, IconStyle::Nerd);

        let parsed: Config = toml::from_str("cover_detail = \"sextant\"").unwrap();
        assert_eq!(parsed.cover_detail, CoverDetail::Sextant);

        let parsed: Config = toml::from_str("cover_detail = \"octant\"").unwrap();
        assert_eq!(parsed.cover_detail, CoverDetail::Octant);

        let parsed: Config = toml::from_str("unm_enabled = false").unwrap();
        assert!(!parsed.unm_enabled);

        let parsed: Config = toml::from_str("cache_limit_mib = 4096").unwrap();
        assert_eq!(parsed.cache_limit_mib, Some(4096));
    }

    #[test]
    fn terminal_theme_only_applies_when_theme_is_implicit() {
        let mut light = parse_with_metadata("quality = \"lossless\"");
        assert!(!light.theme_is_explicit);
        assert!(light.should_apply_terminal_brightness());
        light.apply_terminal_brightness(Some(true));
        assert_eq!(light.config.theme, "fairyfloss");

        let mut dark = parse_with_metadata("language = \"ja\"");
        dark.apply_terminal_brightness(Some(false));
        assert_eq!(dark.config.theme, "db16");

        let mut unavailable = parse_with_metadata("quality = \"192\"");
        unavailable.apply_terminal_brightness(None);
        assert_eq!(unavailable.config.theme, "db16");

        for is_light in [false, true] {
            let mut explicit = parse_with_metadata("theme = \"dracula\"");
            assert!(explicit.theme_is_explicit);
            assert!(!explicit.should_apply_terminal_brightness());
            explicit.apply_terminal_brightness(Some(is_light));
            assert_eq!(explicit.config.theme, "dracula");
        }

        let malformed = parse_with_metadata("theme = [");
        assert!(!malformed.theme_is_explicit);
        assert_eq!(malformed.config, Config::default());

        let invalid_value = parse_with_metadata("theme = 42");
        assert!(invalid_value.theme_is_explicit);
        assert_eq!(invalid_value.config.theme, "db16");
    }

    #[test]
    fn every_supported_quality_maps_to_the_shared_wire_value() {
        let cases = [
            ("\"128\"", AudioQuality::Low128, 128_000),
            ("\"192\"", AudioQuality::Medium192, 192_000),
            ("\"320\"", AudioQuality::High320, 320_000),
            ("\"exhigh\"", AudioQuality::High320, 320_000),
            ("\"lossless\"", AudioQuality::Lossless, 350_000),
            ("\"hires\"", AudioQuality::HiRes, 999_000),
        ];

        for (setting, expected, wire) in cases {
            let parsed: Config = toml::from_str(&format!("quality = {setting}")).unwrap();
            assert_eq!(parsed.quality, expected);
            assert_eq!(parsed.quality.bitrate(), wire);
        }

        let numeric: Config = toml::from_str("quality = 192").unwrap();
        assert_eq!(numeric.quality, AudioQuality::Medium192);
    }

    #[test]
    fn config_round_trips_every_quality_name() {
        let cases = [
            (AudioQuality::Low128, "128"),
            (AudioQuality::Medium192, "192"),
            (AudioQuality::High320, "exhigh"),
            (AudioQuality::Lossless, "lossless"),
            (AudioQuality::HiRes, "hires"),
        ];

        for (quality, name) in cases {
            let config = Config {
                language: "ja".into(),
                quality,
                unm_enabled: false,
                theme: "tokyo-night".into(),
                theme_mode: ThemeMode::Light,
                cache_limit_mib: Some(2048),
                enter_replaces_queue: false,
                layout: "stacked".into(),
                idle_art: Some("~/cover.png".into()),
                progress_style: "bar".into(),
                icons: IconStyle::Nerd,
                cover_mode: CoverMode::Original,
                cover_palette: CoverPalette::Theme,
                cover_detail: CoverDetail::Quad,
                pixel_scale: 1.5,
                spectrum_enabled: true,
                spectrum_style: SpectrumKind::Fire,
                spectrum_glow: true,
                update_check: false,
            };

            let encoded = toml::to_string(&config).unwrap();
            assert!(encoded.contains(&format!("quality = \"{name}\"")));
            assert!(encoded.contains("icons = \"nerd\""));
            assert!(encoded.contains("cover_palette = \"theme\""));
            let decoded: Config = toml::from_str(&encoded).unwrap();
            assert_eq!(decoded, config);
        }
    }

    #[test]
    fn save_to_atomically_replaces_without_leaving_a_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "theme = \"old\"\n").unwrap();

        let config = Config {
            theme: "pico8".into(),
            quality: AudioQuality::HiRes,
            ..Config::default()
        };
        config.save_to(&path).unwrap();

        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(toml::from_str::<Config>(&saved).unwrap(), config);
        let entries = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries, [std::ffi::OsString::from("config.toml")]);
    }
}
