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
pub enum CoverSize {
    Compact,
    #[default]
    Auto,
    Large,
}

impl CoverSize {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Auto, Self::Large];
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
pub enum SpectrumBars {
    Slim,
    #[default]
    Duo,
    Wide,
}

impl SpectrumBars {
    pub const ALL: [Self; 3] = [Self::Slim, Self::Duo, Self::Wide];

    pub fn cells(self) -> (u16, u16) {
        match self {
            Self::Slim => (1, 0),
            Self::Duo => (2, 1),
            Self::Wide => (3, 1),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpectrumSensitivity {
    Soft,
    #[default]
    Normal,
    Sharp,
}

impl SpectrumSensitivity {
    pub const ALL: [Self; 3] = [Self::Soft, Self::Normal, Self::Sharp];

    /// (attack, decay) smoothing coefficients per 50ms frame.
    pub fn coefficients(self) -> (f32, f32) {
        match self {
            Self::Soft => (0.25, 0.06),
            Self::Normal => (0.4, 0.09),
            Self::Sharp => (0.65, 0.16),
        }
    }

    /// dB window under the adaptive ceiling: narrower = more contrast and
    /// motion, wider = calmer bars that all stay lit.
    pub fn range_db(self) -> f32 {
        match self {
            Self::Soft => 45.0,
            Self::Normal => 35.0,
            Self::Sharp => 28.0,
        }
    }
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
#[serde(default, deny_unknown_fields)]
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
    /// Cap on visible lyric rows (current line and translations included),
    /// centered in the panel. None = fill whatever the layout offers.
    pub lyric_rows: Option<u16>,
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
    /// Cover footprint preset; layout still controls its position.
    pub cover_size: CoverSize,
    /// Pixel cover colors: source truecolor or the active theme palette.
    pub cover_palette: CoverPalette,
    /// Sub-cell glyph resolution used by the pixel renderer.
    pub cover_detail: CoverDetail,
    /// Pixel sampling detail (0.5 chunky … 4.0 fine). The final cell
    /// footprint is unchanged.
    pub pixel_scale: f32,
    /// Paint the now-playing title in the theme accent instead of fg.
    pub title_accent: bool,
    /// Retro spectrum visibility, renderer, and phosphor afterglow.
    pub spectrum_enabled: bool,
    pub spectrum_style: SpectrumKind,
    pub spectrum_glow: bool,
    /// +4 dB/octave tilt so highs read as lively as bass (cava-style).
    pub spectrum_flatten: bool,
    /// dB window mapping (WinAmp/cava recipe): mids and highs stay alive
    /// instead of being crushed by the loudest bass bin. Off = legacy
    /// peak-normalized linear scale.
    pub spectrum_db: bool,
    /// Height gradient bar coloring; off = flat accent color.
    pub spectrum_gradient: bool,
    /// Bar footprint: slim 1+0, duo 2+1 (default), wide 3+1 cells.
    pub spectrum_bars: SpectrumBars,
    /// Attack/decay response preset.
    pub spectrum_sensitivity: SpectrumSensitivity,
    /// Split the spectrum into left/right channels meeting at the center.
    pub spectrum_stereo: bool,
    /// Silent GitHub release check on startup; hints only, never self-updates.
    pub update_check: bool,
    /// Play the skippable pixel-logo intro on startup and before updates.
    pub intro_animation: bool,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub theme_is_explicit: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    #[error("failed to read ypm config: {0}")]
    Read(#[source] io::Error),
    #[error("failed to create the default ypm config: {0}")]
    Create(#[source] io::Error),
    #[error("invalid TOML syntax in ypm config: {0}")]
    Syntax(#[source] toml::de::Error),
    #[error("invalid field in ypm config: {0}")]
    Field(#[source] toml::de::Error),
    #[error("invalid value for ypm config field `{field}`: {value}")]
    InvalidValue { field: &'static str, value: String },
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
            lyric_rows: None,
            idle_art: None,
            progress_style: "dot".into(),
            icons: IconStyle::Unicode,
            cover_mode: CoverMode::Pixel,
            cover_size: CoverSize::Auto,
            cover_palette: CoverPalette::Original,
            cover_detail: CoverDetail::Half,
            pixel_scale: 1.0,
            title_accent: false,
            spectrum_enabled: false,
            spectrum_style: SpectrumKind::Blocks,
            spectrum_glow: false,
            spectrum_flatten: true,
            spectrum_db: true,
            spectrum_gradient: true,
            spectrum_bars: SpectrumBars::default(),
            spectrum_sensitivity: SpectrumSensitivity::default(),
            spectrum_stereo: false,
            update_check: true,
            intro_animation: true,
        }
    }
}

impl Config {
    /// A missing file gets a commented template so the options are
    /// discoverable without reading docs. Existing files are authoritative:
    /// an unreadable or invalid config must be fixed before it can be saved.
    pub fn load_with_metadata() -> Result<LoadedConfig, ConfigLoadError> {
        let path = config_dir().join("config.toml");
        Self::load_from(&path)
    }

    fn load_from(path: &Path) -> Result<LoadedConfig, ConfigLoadError> {
        match std::fs::read_to_string(path) {
            Ok(text) => parse_with_metadata(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                write_template(path).map_err(ConfigLoadError::Create)?;
                Ok(LoadedConfig {
                    config: Self::default(),
                    theme_is_explicit: false,
                })
            }
            Err(error) => Err(ConfigLoadError::Read(error)),
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

fn parse_with_metadata(text: &str) -> Result<LoadedConfig, ConfigLoadError> {
    let value = toml::from_str::<toml::Value>(text).map_err(ConfigLoadError::Syntax)?;
    let theme_is_explicit = value
        .as_table()
        .is_some_and(|table| table.contains_key("theme"));
    let config = value.try_into::<Config>().map_err(ConfigLoadError::Field)?;
    validate_config(&config)?;
    Ok(LoadedConfig {
        config,
        theme_is_explicit,
    })
}

fn validate_config(config: &Config) -> Result<(), ConfigLoadError> {
    if !matches!(config.layout.as_str(), "side" | "stacked") {
        return Err(ConfigLoadError::InvalidValue {
            field: "layout",
            value: config.layout.clone(),
        });
    }
    if !matches!(config.progress_style.as_str(), "dot" | "bar") {
        return Err(ConfigLoadError::InvalidValue {
            field: "progress_style",
            value: config.progress_style.clone(),
        });
    }
    if !config.pixel_scale.is_finite() || !(0.5..=4.0).contains(&config.pixel_scale) {
        return Err(ConfigLoadError::InvalidValue {
            field: "pixel_scale",
            value: config.pixel_scale.to_string(),
        });
    }
    Ok(())
}

const TEMPLATE: &str = r#"# ypm 配置 — 常用项也可在 ypm 设置页修改；手动编辑后重启生效。
# 所有项都可省略（使用默认值）。

# language = "zh"            # zh | en | ja
# quality = "exhigh"          # 128 | 192 | 320/exhigh | lossless | hires
# unm_enabled = true          # 无版权/VIP 曲目无可播地址时尝试 UNM 换源
# theme = "db16"              # db16 | pico8 | gameboy | everforest | everforest-light | tokyo-night | tokyo-night-storm
#                              # tokyo-night-day | one-dark | one-dark-pro | one-light | dracula | alucard
#                              # synthwave84 | laserwave | fairyfloss | ultraviolence | transparent
#                              # 省略时：暗色终端用 db16，亮色终端用 fairyfloss
# layout = "side"             # side（封面撑满高度）| stacked（封面居中在上）
# lyric_rows = 7               # 歌词最多显示行数（含当前行与翻译行），在面板里居中；省略 = 自动填满
# progress_style = "dot"      # dot（细线+圆点）| bar（粗块）
# icons = "unicode"           # unicode（无需特殊字体）| nerd（需要 Nerd Font）
# cover_mode = "pixel"        # pixel（字符像素画）| original（终端原图协议，不支持时回退 pixel）
# cover_size = "auto"         # compact | auto | large，仅调整大小；位置跟随 layout
# cover_palette = "original"  # original（原图真彩）| theme（主题配色）
# cover_detail = "half"       # half（1×2）| quad（2×2）| sextant（2×3）| octant（2×4，需 Unicode 16 字体）
#                              # octant 显示方框时请切回 sextant
# enter_replaces_queue = true # Enter：整列表成为队列；false = 只播这一首
# idle_art = "~/my-art.png"   # 开屏像素画（png/jpg/webp/gif，自动像素化）
# cache_limit_mib = 8192       # 仅显式设置时更新 ypm 进程共享的上限
# pixel_scale = 1.0            # 像素细腻度：0.5 更复古块状，4.0 更细腻
# title_accent = false         # 歌名使用主题强调色
# spectrum_enabled = false     # v 键也可全局开关
# spectrum_style = "blocks"   # blocks | mirror | led | braille | shade | scope | fire | waterfall | vu | reflect
# spectrum_glow = false        # 荧光余辉残影
# spectrum_flatten = true      # 高频补偿（+4dB/倍频程），关掉显示真实能量比例
# spectrum_db = true           # dB 对数标度：中高频不再被最响的贝斯压扁；false 为旧线性标度
# spectrum_gradient = true     # 柱体高度渐变色；false 为纯主题色
# spectrum_bars = "duo"        # slim | duo | wide，适用于 blocks/mirror/led/reflect 柱状风格
# spectrum_sensitivity = "normal" # soft | normal | sharp 跳动灵敏度
# spectrum_stereo = false      # 左右声道分离显示（中间低频、两侧高频）
# update_check = true          # 启动时静默检查新版本
# intro_animation = true       # 启动和 ypm update 时播放动画；任意键可跳过
"#;

fn deserialize_language<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "zh" | "en" | "ja" => Ok(value),
        _ => Err(D::Error::custom("unsupported UI language")),
    }
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
    fn defaults_and_supported_values_are_stable() {
        let config = Config::default();
        assert_eq!(config.language, "zh");
        assert_eq!(config.quality, AudioQuality::High320);
        assert!(config.unm_enabled);
        assert_eq!(config.theme, "db16");
        assert_eq!(config.cover_mode, CoverMode::Pixel);
        assert_eq!(config.cover_size, CoverSize::Auto);
        assert_eq!(config.cover_palette, CoverPalette::Original);
        assert_eq!(config.cover_detail, CoverDetail::Half);
        assert_eq!(config.icons, IconStyle::Unicode);
        assert_eq!(config.cache_limit_mib, None);
        assert!(!config.spectrum_enabled);
        assert_eq!(config.spectrum_style, SpectrumKind::Blocks);
        assert!(!config.spectrum_glow);
        assert!(config.spectrum_flatten);
        assert!(config.spectrum_gradient);
        assert!(!config.spectrum_stereo);
        assert!(config.intro_animation);

        let parsed: Config = toml::from_str("quality = \"lossless\"").unwrap();
        assert_eq!(parsed.quality, AudioQuality::Lossless);
        assert_eq!(parsed.theme, "db16");

        let parsed: Config = toml::from_str("language = \"ja\"").unwrap();
        assert_eq!(parsed.language, "ja");
        assert!(toml::from_str::<Config>("language = \"fr\"").is_err());

        let parsed: Config = toml::from_str("cover_mode = \"original\"").unwrap();
        assert_eq!(parsed.cover_mode, CoverMode::Original);

        let parsed: Config = toml::from_str("cover_size = \"compact\"").unwrap();
        assert_eq!(parsed.cover_size, CoverSize::Compact);

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

        let parsed: Config = toml::from_str("lyric_rows = 7").unwrap();
        assert_eq!(parsed.lyric_rows, Some(7));
    }

    #[test]
    fn terminal_theme_only_applies_when_theme_is_implicit() {
        let mut light = parse_with_metadata("quality = \"lossless\"").unwrap();
        assert!(!light.theme_is_explicit);
        assert!(light.should_apply_terminal_brightness());
        light.apply_terminal_brightness(Some(true));
        assert_eq!(light.config.theme, "fairyfloss");

        let mut dark = parse_with_metadata("language = \"ja\"").unwrap();
        dark.apply_terminal_brightness(Some(false));
        assert_eq!(dark.config.theme, "db16");

        let mut unavailable = parse_with_metadata("quality = \"192\"").unwrap();
        unavailable.apply_terminal_brightness(None);
        assert_eq!(unavailable.config.theme, "db16");

        for is_light in [false, true] {
            let mut explicit = parse_with_metadata("theme = \"dracula\"").unwrap();
            assert!(explicit.theme_is_explicit);
            assert!(!explicit.should_apply_terminal_brightness());
            explicit.apply_terminal_brightness(Some(is_light));
            assert_eq!(explicit.config.theme, "dracula");
        }
    }

    #[test]
    fn syntax_errors_are_not_replaced_with_defaults() {
        assert!(matches!(
            parse_with_metadata("theme = ["),
            Err(ConfigLoadError::Syntax(_))
        ));
    }

    #[test]
    fn invalid_fields_are_not_dropped_from_an_otherwise_valid_file() {
        for text in [
            "language = \"fr\"",
            "quality = \"flac\"",
            "cover_detail = \"duodecant\"",
            "pixel_scale = \"chunky\"",
            "cache_limit_mib = -1",
            "future_option = 7",
        ] {
            assert!(
                matches!(parse_with_metadata(text), Err(ConfigLoadError::Field(_))),
                "field should fail: {text}"
            );
        }
    }

    #[test]
    fn values_previously_normalized_by_callers_fail_at_the_config_boundary() {
        for (text, field) in [
            ("layout = \"horizontal\"", "layout"),
            ("progress_style = \"line\"", "progress_style"),
            ("pixel_scale = 8.0", "pixel_scale"),
        ] {
            assert!(matches!(
                parse_with_metadata(text),
                Err(ConfigLoadError::InvalidValue {
                    field: actual,
                    ..
                }) if actual == field
            ));
        }
    }

    #[test]
    fn missing_config_creates_a_template_but_other_read_errors_surface() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config.toml");

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.config, Config::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);

        assert!(matches!(
            Config::load_from(directory.path()),
            Err(ConfigLoadError::Read(_))
        ));
    }

    #[test]
    fn invalid_utf8_is_a_read_error_instead_of_a_missing_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();

        assert!(matches!(
            Config::load_from(&path),
            Err(ConfigLoadError::Read(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe]);
    }

    #[test]
    fn invalid_existing_config_is_reported_without_rewriting_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");

        for (contents, expected) in [
            ("theme = [", "syntax"),
            ("language = \"ja\"\nquality = \"flac\"\n", "field"),
        ] {
            std::fs::write(&path, contents).unwrap();
            let error = Config::load_from(&path).unwrap_err();
            match expected {
                "syntax" => assert!(matches!(error, ConfigLoadError::Syntax(_))),
                "field" => assert!(matches!(error, ConfigLoadError::Field(_))),
                _ => unreachable!(),
            }
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
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
                lyric_rows: Some(7),
                idle_art: Some("~/cover.png".into()),
                progress_style: "bar".into(),
                icons: IconStyle::Nerd,
                cover_mode: CoverMode::Original,
                cover_size: CoverSize::Large,
                cover_palette: CoverPalette::Theme,
                cover_detail: CoverDetail::Quad,
                pixel_scale: 1.5,
                title_accent: true,
                spectrum_enabled: true,
                spectrum_style: SpectrumKind::Fire,
                spectrum_glow: true,
                spectrum_flatten: false,
                spectrum_db: false,
                spectrum_gradient: false,
                spectrum_bars: SpectrumBars::Wide,
                spectrum_sensitivity: SpectrumSensitivity::Sharp,
                spectrum_stereo: true,
                update_check: false,
                intro_animation: false,
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
