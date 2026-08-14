use std::fmt;
use std::sync::OnceLock;

use crate::spectrum::SpectrumKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lang {
    Zh,
    En,
    Ja,
}

impl Lang {
    pub fn from_config(value: &str) -> Self {
        match value {
            "en" => Self::En,
            "ja" => Self::Ja,
            _ => Self::Zh,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::enum_variant_names)] // Op*/Api* prefixes group the key space deliberately
pub enum Key {
    Resolving,
    UnmSourceUsed,
    TrackUnavailable,
    QueueFinished,
    FetchingQr,
    ScanQr,
    QrScannedConfirm,
    QrExpired,
    NetworkRetrying,
    SessionExpired,
    NowPlaying,
    Cover,
    Library,
    Search,
    Queue,
    Settings,
    SettingTheme,
    SettingThemeMode,
    SettingLanguage,
    SettingQuality,
    SettingCoverMode,
    SettingCoverPalette,
    SettingCoverDetail,
    SettingLayout,
    SettingProgressStyle,
    SettingPixelDetail,
    SettingSpectrumEnabled,
    SettingSpectrumStyle,
    SettingSpectrumGlow,
    SettingQueueBehavior,
    SettingIcons,
    SettingQueueList,
    SettingQueueSingle,
    SettingAdjust,
    On,
    Off,
    SettingsHint,
    PixelDetailHint,
    NerdFontHint,
    NerdFontDetectedHint,
    OctantFontRequired,
    OctantFontHint,
    SettingsSaved,
    SettingsSaveFailed,
    Save,
    QuitQuestion,
    Quit,
    Cancel,
    Play,
    Select,
    TopBottom,
    Back,
    ChangeTrack,
    Page,
    Filter,
    ClearFilter,
    FinishFilter,
    AddToQueue,
    AddedToQueue,
    Mute,
    Shuffle,
    Repeat,
    LibraryFocus,
    Pause,
    Seek,
    Volume,
    Zen,
    Spectrum,
    SpectrumPreview,
    LoginTitle,
    LoginInstruction,
    SignedOut,
    LikedSongs,
    DailyRecommendations,
    PersonalFm,
    CloudDrive,
    SyncingLibrary,
    EmptyLibrary,
    ColumnTitle,
    SearchPrompt,
    SearchSongs,
    SearchArtists,
    SearchAlbums,
    SearchPlaylists,
    SearchChannelSwitch,
    Open,
    HelpTitle,
    HelpAnyKey,
    CommandPalette,
    CommandPaletteHint,
    CommandNoMatches,
    CommandPlayPause,
    CommandNext,
    CommandPrev,
    CommandShuffle,
    CommandRepeat,
    CommandLike,
    CommandZen,
    CommandSpectrum,
    CommandMute,
    CommandSeek,
    CommandVolume,
    CommandTheme,
    CommandQuality,
    CommandMouse,
    CommandGoto,
    CommandQuit,
    SettingUpdateCheck,
    LabelLike,
    LabelHelp,
    Searching,
    SearchFailed,
    NoResults,
    TypeToSearch,
    Liked,
    Unliked,
    LikeFailed,
    ColumnArtist,
    ColumnAlbum,
    ColumnDuration,
    EmptyQueue,
    Relogin,
    ScanLogin,
    OpQrKey,
    ApiQrKeyMissing,
    OpQrCheck,
    ApiLoginCookieMissing,
    OpPersistSession,
    OpAccount,
    ApiInvalidSession,
    OpUserPlaylist,
    ApiLikedPlaylistMissing,
    ApiLibraryPayloadMissing,
    OpPlaylistTracks,
    OpSongUrl,
    ApiPlaybackUrlUnavailable,
    OpLyrics,
    OpSearch,
    OpFetchCover,
    OpReadCover,
    OpBuildQr,
}

static LANGUAGE: OnceLock<Lang> = OnceLock::new();

pub fn init(lang: Lang) {
    let _ = LANGUAGE.set(lang);
}

pub fn t(key: Key) -> &'static str {
    t_for(language(), key)
}

pub fn t_track_count(n: usize) -> String {
    track_count_for(language(), n)
}

pub fn t_result_count(n: usize) -> String {
    result_count_for(language(), n)
}

fn result_count_for(lang: Lang, n: usize) -> String {
    match lang {
        Lang::Zh => format!("{n} 项"),
        Lang::En => format!("{n} results"),
        Lang::Ja => format!("{n}件"),
    }
}

pub fn t_welcome(name: &str) -> String {
    match language() {
        Lang::Zh => format!("欢迎，{name}"),
        Lang::En => format!("Welcome, {name}"),
        Lang::Ja => format!("ようこそ、{name}"),
    }
}

pub fn t_liked_songs_count(n: usize) -> String {
    format!("{} · {}", t(Key::LikedSongs), t_track_count(n))
}

pub fn t_playing(kind: &str) -> String {
    match language() {
        Lang::Zh => format!("播放中 · {kind}"),
        Lang::En => format!("Playing · {kind}"),
        Lang::Ja => format!("再生中 · {kind}"),
    }
}

pub fn t_spectrum_style(kind: SpectrumKind) -> &'static str {
    match (language(), kind) {
        (Lang::Zh, SpectrumKind::Blocks) => "经典块条",
        (Lang::Zh, SpectrumKind::Mirror) => "镜像双向",
        (Lang::Zh, SpectrumKind::Led) => "LED 段条",
        (Lang::Zh, SpectrumKind::Braille) => "盲文高清",
        (Lang::Zh, SpectrumKind::Shade) => "热浪层",
        (Lang::Zh, SpectrumKind::Scope) => "示波器",
        (Lang::Zh, SpectrumKind::Fire) => "火焰频谱",
        (Lang::Zh, SpectrumKind::Waterfall) => "瀑布频谱",
        (Lang::Zh, SpectrumKind::Vu) => "VU 表盘",
        (Lang::Zh, SpectrumKind::Reflect) => "水面倒影",
        (Lang::En, SpectrumKind::Blocks) => "Classic blocks",
        (Lang::En, SpectrumKind::Mirror) => "Mirror",
        (Lang::En, SpectrumKind::Led) => "LED segments",
        (Lang::En, SpectrumKind::Braille) => "Braille ridge",
        (Lang::En, SpectrumKind::Shade) => "Heat haze",
        (Lang::En, SpectrumKind::Scope) => "Oscilloscope",
        (Lang::En, SpectrumKind::Fire) => "Fire",
        (Lang::En, SpectrumKind::Waterfall) => "Waterfall",
        (Lang::En, SpectrumKind::Vu) => "VU meters",
        (Lang::En, SpectrumKind::Reflect) => "Reflection",
        (Lang::Ja, SpectrumKind::Blocks) => "クラシックブロック",
        (Lang::Ja, SpectrumKind::Mirror) => "ミラー",
        (Lang::Ja, SpectrumKind::Led) => "LED セグメント",
        (Lang::Ja, SpectrumKind::Braille) => "ブライユ表示",
        (Lang::Ja, SpectrumKind::Shade) => "ヒートウェーブ",
        (Lang::Ja, SpectrumKind::Scope) => "オシロスコープ",
        (Lang::Ja, SpectrumKind::Fire) => "炎スペクトラム",
        (Lang::Ja, SpectrumKind::Waterfall) => "ウォーターフォール",
        (Lang::Ja, SpectrumKind::Vu) => "VU メーター",
        (Lang::Ja, SpectrumKind::Reflect) => "水面反射",
    }
}

pub fn t_login_interrupted(error: impl fmt::Display) -> String {
    match language() {
        Lang::Zh => format!("网络不稳定，登录中断（{error}）；请从主菜单重试"),
        Lang::En => format!("Login interrupted by an unstable network ({error}); retry from the main menu"),
        Lang::Ja => format!("通信が不安定なためログインを中断しました（{error}）。メインメニューから再試行してください"),
    }
}

pub fn t_library_load_failed(error: impl fmt::Display) -> String {
    match language() {
        Lang::Zh => format!("歌单加载失败：{error}"),
        Lang::En => format!("Failed to load library: {error}"),
        Lang::Ja => format!("ライブラリを読み込めませんでした：{error}"),
    }
}

pub fn t_api_failed(operation: Key, error: impl fmt::Debug) -> String {
    let operation = t(operation);
    match language() {
        Lang::Zh => format!("{operation}失败：{error:?}"),
        Lang::En => format!("Failed to {operation}: {error:?}"),
        Lang::Ja => format!("{operation}に失敗しました：{error:?}"),
    }
}

pub fn t_unknown_qr_status(status: i64) -> String {
    match language() {
        Lang::Zh => format!("二维码状态码未知：{status}"),
        Lang::En => format!("Unknown QR code status: {status}"),
        Lang::Ja => format!("不明なQRコードステータス：{status}"),
    }
}

pub fn t_search_not_found(keywords: &str) -> String {
    match language() {
        Lang::Zh => format!("搜索不到「{keywords}」"),
        Lang::En => format!("No results for “{keywords}”"),
        Lang::Ja => format!("「{keywords}」が見つかりません"),
    }
}

pub fn t_candidates_unavailable(keywords: &str) -> String {
    match language() {
        Lang::Zh => format!("「{keywords}」的候选都拿不到播放地址（可能需要登录）"),
        Lang::En => format!("No playable result for “{keywords}” (sign-in may be required)"),
        Lang::Ja => format!("「{keywords}」を再生できません（ログインが必要な場合があります）"),
    }
}

pub fn t_update_available(version: &str, brew: bool) -> String {
    match (language(), brew) {
        (Lang::Zh, true) => format!("新版本 {version} 可用 · brew upgrade ypm"),
        (Lang::Zh, false) => format!("新版本 {version} 可用 · GitHub Releases 下载"),
        (Lang::En, true) => format!("Update {version} available · brew upgrade ypm"),
        (Lang::En, false) => format!("Update {version} available · download from GitHub Releases"),
        (Lang::Ja, true) => format!("新バージョン {version} · brew upgrade ypm"),
        (Lang::Ja, false) => format!("新バージョン {version} · GitHub Releases からダウンロード"),
    }
}

pub fn t_command_executed(command: &str) -> String {
    match language() {
        Lang::Zh => format!("已执行命令：{command}"),
        Lang::En => format!("Command executed: {command}"),
        Lang::Ja => format!("コマンドを実行しました：{command}"),
    }
}

pub fn t_command_unknown(command: &str) -> String {
    match language() {
        Lang::Zh => format!("没有匹配的命令：{command}"),
        Lang::En => format!("No matching command: {command}"),
        Lang::Ja => format!("一致するコマンドがありません：{command}"),
    }
}

pub fn t_command_missing_argument(command: &str, expected: &str) -> String {
    match language() {
        Lang::Zh => format!("{command} 缺少参数（需要 {expected}）"),
        Lang::En => format!("{command} needs an argument ({expected})"),
        Lang::Ja => format!("{command} に引数が必要です（{expected}）"),
    }
}

pub fn t_command_unexpected_argument(command: &str) -> String {
    match language() {
        Lang::Zh => format!("{command} 不接受这些参数"),
        Lang::En => format!("{command} does not accept those arguments"),
        Lang::Ja => format!("{command} は引数を受け取りません"),
    }
}

pub fn t_command_invalid_argument(command: &str, value: &str, expected: &str) -> String {
    match language() {
        Lang::Zh => format!("{command} 的参数“{value}”无效（需要 {expected}）"),
        Lang::En => format!("Invalid argument “{value}” for {command} (expected {expected})"),
        Lang::Ja => format!("{command} の引数「{value}」は無効です（{expected}）"),
    }
}

fn language() -> Lang {
    LANGUAGE.get().copied().unwrap_or(Lang::Zh)
}

fn track_count_for(lang: Lang, n: usize) -> String {
    match lang {
        Lang::Zh => format!("{n} 首"),
        Lang::En => format!("{n} tracks"),
        Lang::Ja => format!("{n}曲"),
    }
}

fn t_for(lang: Lang, key: Key) -> &'static str {
    match lang {
        Lang::Zh => match key {
            Key::Resolving => "解析中…",
            Key::UnmSourceUsed => "已通过 UNM 换源",
            Key::TrackUnavailable => "无法播放这首歌",
            Key::QueueFinished => "队列播完了",
            Key::FetchingQr => "正在获取二维码…",
            Key::ScanQr => "用网易云音乐 App 扫码",
            Key::QrScannedConfirm => "已扫码，在手机上确认…",
            Key::QrExpired => "二维码已过期，请从主菜单重新扫码",
            Key::NetworkRetrying => "网络抖动，重试中…",
            Key::SessionExpired => "登录态已失效，请从主菜单重新扫码",
            Key::NowPlaying => "正在播放",
            Key::Cover => "封面",
            Key::Library => "曲库",
            Key::Search => "搜索",
            Key::Queue => "队列",
            Key::Settings => "设置",
            Key::SettingTheme => "主题",
            Key::SettingThemeMode => "明暗",
            Key::SettingLanguage => "语言（重启后）",
            Key::SettingQuality => "音质",
            Key::SettingCoverMode => "封面模式（重启后）",
            Key::SettingCoverPalette => "封面配色",
            Key::SettingCoverDetail => "封面分块",
            Key::SettingLayout => "播放布局",
            Key::SettingProgressStyle => "进度条",
            Key::SettingPixelDetail => "像素细节",
            Key::SettingSpectrumEnabled => "频谱开关",
            Key::SettingSpectrumStyle => "频谱样式",
            Key::SettingSpectrumGlow => "荧光余辉",
            Key::SettingQueueBehavior => "Enter 行为",
            Key::SettingIcons => "图标",
            Key::SettingQueueList => "整列入队",
            Key::SettingQueueSingle => "只播单曲",
            Key::SettingAdjust => "调整",
            Key::On => "开",
            Key::Off => "关",
            Key::SettingsHint => "j/k 选择 · h/l 即时预览 · Enter 保存 · Esc 取消",
            Key::PixelDetailHint => {
                "3.0× / 4.0× 更高清晰度请配合封面分块 quad / sextant / octant"
            }
            Key::NerdFontHint => {
                "终端字体需支持 Nerd Font\nmacOS：brew install font-symbols-only-nerd-font"
            }
            Key::NerdFontDetectedHint => "✓ 已检测到 Nerd Font（终端字体需选用它）",
            Key::OctantFontRequired => "需字体支持",
            Key::OctantFontHint => {
                "octant 需 Unicode 16 字体支持；不支持时会显示方框\n显示方框时请切回 sextant"
            }
            Key::SettingsSaved => "设置已保存",
            Key::SettingsSaveFailed => "设置保存失败",
            Key::Save => "保存",
            Key::QuitQuestion => "退出 ypm？",
            Key::Quit => "退出",
            Key::Cancel => "取消",
            Key::Play => "播放",
            Key::Select => "选择",
            Key::TopBottom => "顶/底",
            Key::Back => "返回",
            Key::ChangeTrack => "切歌",
            Key::Page => "翻页",
            Key::Filter => "过滤当前列表",
            Key::ClearFilter => "清除过滤",
            Key::FinishFilter => "完成输入",
            Key::AddToQueue => "加入队列",
            Key::AddedToQueue => "已加入队列",
            Key::Mute => "静音",
            Key::Shuffle => "随机开 / 关",
            Key::Repeat => "列表 / 单曲 / 关",
            Key::LibraryFocus => "曲库焦点 / 搜索类型",
            Key::Pause => "暂停",
            Key::Seek => "快进",
            Key::Volume => "音量",
            Key::Zen => "纯净",
            Key::Spectrum => "频谱开 / 关",
            Key::SpectrumPreview => "实时预览",
            Key::SearchPrompt => "搜索网易云曲库",
            Key::SearchSongs => "单曲",
            Key::SearchArtists => "歌手",
            Key::SearchAlbums => "专辑",
            Key::SearchPlaylists => "歌单",
            Key::SearchChannelSwitch => "切换类型",
            Key::Open => "打开",
            Key::HelpTitle => "快捷键",
            Key::HelpAnyKey => "按任意键关闭",
            Key::CommandPalette => "命令面板",
            Key::CommandPaletteHint => "↑/↓ 选择 · Enter 执行 · Esc 关闭",
            Key::CommandNoMatches => "没有匹配的命令",
            Key::CommandPlayPause => "播放 / 暂停",
            Key::CommandNext => "下一首",
            Key::CommandPrev => "上一首",
            Key::CommandShuffle => "随机",
            Key::CommandRepeat => "循环",
            Key::CommandLike => "喜欢 / 收藏",
            Key::CommandZen => "纯净",
            Key::CommandSpectrum => "频谱",
            Key::CommandMute => "静音",
            Key::CommandSeek => "跳转 <±秒>",
            Key::CommandVolume => "音量 <0-100>",
            Key::CommandTheme => "主题 <名称>",
            Key::CommandQuality => "音质 <档位>",
            Key::CommandMouse => "鼠标捕获",
            Key::CommandGoto => "前往 <视图>",
            Key::CommandQuit => "退出",
            Key::SettingUpdateCheck => "更新检测",
            Key::LabelLike => "收藏",
            Key::LabelHelp => "全部键位",
            Key::Searching => "搜索中…",
            Key::SearchFailed => "搜索失败，请稍后重试",
            Key::NoResults => "没有找到相关结果",
            Key::TypeToSearch => "输入关键词，Enter 搜索",
            Key::Liked => "已收藏",
            Key::Unliked => "已取消收藏",
            Key::LikeFailed => "收藏失败",
            Key::LoginTitle => "扫码登录网易云",
            Key::LoginInstruction => {
                "请用网易云音乐 App 里的「扫一扫」（系统相机扫会提示无效）"
            }
            Key::SignedOut => "未登录",
            Key::LikedSongs => "我喜欢的音乐",
            Key::DailyRecommendations => "每日推荐",
            Key::PersonalFm => "私人FM",
            Key::CloudDrive => "云盘",
            Key::SyncingLibrary => "歌单同步中…",
            Key::EmptyLibrary => "这里还是空的",
            Key::ColumnTitle => "歌名",
            Key::ColumnArtist => "歌手",
            Key::ColumnAlbum => "专辑",
            Key::ColumnDuration => "时长",
            Key::EmptyQueue => "队列是空的——去曲库按 Enter，整列表就会成为播放队列",
            Key::Relogin => "重新扫码登录",
            Key::ScanLogin => "扫码登录",
            Key::OpQrKey => "请求二维码密钥",
            Key::ApiQrKeyMissing => "二维码密钥响应缺少 unikey",
            Key::OpQrCheck => "检查二维码状态",
            Key::ApiLoginCookieMissing => "登录成功但响应里没有 MUSIC_U cookie",
            Key::OpPersistSession => "保存登录信息",
            Key::OpAccount => "获取账号信息",
            Key::ApiInvalidSession => "登录态无效（拿不到账号 id）",
            Key::OpUserPlaylist => "获取用户歌单",
            Key::ApiLikedPlaylistMissing => "没有找到「我喜欢的音乐」歌单",
            Key::ApiLibraryPayloadMissing => "服务端没有返回有效的曲库数据",
            Key::OpPlaylistTracks => "获取歌单歌曲",
            Key::OpSongUrl => "获取播放地址",
            Key::ApiPlaybackUrlUnavailable => {
                "这首歌暂时拿不到播放地址（可能需要登录或 VIP）"
            }
            Key::OpLyrics => "获取歌词",
            Key::OpSearch => "搜索歌曲",
            Key::OpFetchCover => "下载封面",
            Key::OpReadCover => "读取封面数据",
            Key::OpBuildQr => "生成二维码",
        },
        Lang::En => match key {
            Key::Resolving => "Resolving…",
            Key::UnmSourceUsed => "Switched source via UNM",
            Key::TrackUnavailable => "This track cannot be played",
            Key::QueueFinished => "End of queue",
            Key::FetchingQr => "Getting QR code…",
            Key::ScanQr => "Scan with the NetEase Cloud Music app",
            Key::QrScannedConfirm => "Scanned—confirm on your phone…",
            Key::QrExpired => "QR code expired; scan again from the main menu",
            Key::NetworkRetrying => "Network hiccup—retrying…",
            Key::SessionExpired => "Session expired; scan again from the main menu",
            Key::NowPlaying => "Now Playing",
            Key::Cover => "Cover",
            Key::Library => "Library",
            Key::Search => "Search",
            Key::Queue => "Queue",
            Key::Settings => "Settings",
            Key::SettingTheme => "Theme",
            Key::SettingThemeMode => "Appearance",
            Key::SettingLanguage => "Language (restart)",
            Key::SettingQuality => "Audio quality",
            Key::SettingCoverMode => "Cover mode (restart)",
            Key::SettingCoverPalette => "Cover palette",
            Key::SettingCoverDetail => "Cover blocks",
            Key::SettingLayout => "Player layout",
            Key::SettingProgressStyle => "Progress style",
            Key::SettingPixelDetail => "Pixel detail",
            Key::SettingSpectrumEnabled => "Spectrum",
            Key::SettingSpectrumStyle => "Spectrum style",
            Key::SettingSpectrumGlow => "Phosphor glow",
            Key::SettingQueueBehavior => "Enter behavior",
            Key::SettingIcons => "Icons",
            Key::SettingQueueList => "Queue the list",
            Key::SettingQueueSingle => "Play one track",
            Key::SettingAdjust => "Adjust",
            Key::On => "On",
            Key::Off => "Off",
            Key::SettingsHint => "j/k select · h/l preview · Enter save · Esc cancel",
            Key::PixelDetailHint => {
                "Pair 3.0× / 4.0× detail with quad / sextant / octant cover blocks"
            }
            Key::NerdFontHint => {
                "Your terminal font must support Nerd Font\nmacOS: brew install font-symbols-only-nerd-font"
            }
            Key::NerdFontDetectedHint => {
                "✓ Nerd Font detected (select it as your terminal font)"
            }
            Key::OctantFontRequired => "font support required",
            Key::OctantFontHint => {
                "octant needs a Unicode 16 font; unsupported glyphs show □\nIf □ appears, switch back to sextant"
            }
            Key::SettingsSaved => "Settings saved",
            Key::SettingsSaveFailed => "Could not save settings",
            Key::Save => "Save",
            Key::QuitQuestion => "Quit ypm?",
            Key::Quit => "Quit",
            Key::Cancel => "Cancel",
            Key::Play => "Play",
            Key::Select => "Select",
            Key::TopBottom => "Top/Bottom",
            Key::Back => "Back",
            Key::ChangeTrack => "Prev/Next",
            Key::Page => "Page",
            Key::Filter => "Filter this list",
            Key::ClearFilter => "Clear filter",
            Key::FinishFilter => "Finish filter",
            Key::AddToQueue => "Add to queue",
            Key::AddedToQueue => "Added to queue",
            Key::Mute => "Mute",
            Key::Shuffle => "Shuffle on / off",
            Key::Repeat => "List / one / off",
            Key::LibraryFocus => "Library focus / search type",
            Key::Pause => "Pause",
            Key::Seek => "Seek",
            Key::Volume => "Volume",
            Key::Zen => "Zen",
            Key::Spectrum => "Spectrum on / off",
            Key::SpectrumPreview => "Live preview",
            Key::SearchPrompt => "Search NCM",
            Key::SearchSongs => "Songs",
            Key::SearchArtists => "Artists",
            Key::SearchAlbums => "Albums",
            Key::SearchPlaylists => "Playlists",
            Key::SearchChannelSwitch => "Switch type",
            Key::Open => "Open",
            Key::HelpTitle => "Keyboard",
            Key::HelpAnyKey => "Press any key to close",
            Key::CommandPalette => "Command Palette",
            Key::CommandPaletteHint => "↑/↓ select · Enter run · Esc close",
            Key::CommandNoMatches => "No matching commands",
            Key::CommandPlayPause => "Play / Pause",
            Key::CommandNext => "Next track",
            Key::CommandPrev => "Previous track",
            Key::CommandShuffle => "Shuffle",
            Key::CommandRepeat => "Repeat",
            Key::CommandLike => "Like / Favorite",
            Key::CommandZen => "Zen mode",
            Key::CommandSpectrum => "Spectrum",
            Key::CommandMute => "Mute",
            Key::CommandSeek => "Seek <±sec>",
            Key::CommandVolume => "Volume <0-100>",
            Key::CommandTheme => "Theme <name>",
            Key::CommandQuality => "Quality <level>",
            Key::CommandMouse => "Mouse capture",
            Key::CommandGoto => "Go to <view>",
            Key::CommandQuit => "Quit",
            Key::SettingUpdateCheck => "Update check",
            Key::LabelLike => "Like",
            Key::LabelHelp => "All keys",
            Key::Searching => "Searching…",
            Key::SearchFailed => "Search failed. Try again later",
            Key::NoResults => "No results",
            Key::TypeToSearch => "Type keywords, Enter to search",
            Key::Liked => "Liked",
            Key::Unliked => "Removed from likes",
            Key::LikeFailed => "Like failed",
            Key::LoginTitle => "Sign in to NetEase Cloud Music",
            Key::LoginInstruction => {
                "Use Scan in the NetEase Cloud Music app (the camera app will not work)"
            }
            Key::SignedOut => "Signed out",
            Key::LikedSongs => "Liked Songs",
            Key::DailyRecommendations => "Daily Picks",
            Key::PersonalFm => "Personal FM",
            Key::CloudDrive => "Cloud Drive",
            Key::SyncingLibrary => "Syncing library…",
            Key::EmptyLibrary => "Nothing here yet",
            Key::ColumnTitle => "Title",
            Key::ColumnArtist => "Artist",
            Key::ColumnAlbum => "Album",
            Key::ColumnDuration => "Time",
            Key::EmptyQueue => "Queue is empty—press Enter in Library to queue the list",
            Key::Relogin => "Scan again",
            Key::ScanLogin => "Sign in with QR",
            Key::OpQrKey => "request a QR code key",
            Key::ApiQrKeyMissing => "QR code response is missing unikey",
            Key::OpQrCheck => "check QR code status",
            Key::ApiLoginCookieMissing => "Signed in, but MUSIC_U cookie is missing",
            Key::OpPersistSession => "save sign-in data",
            Key::OpAccount => "load account details",
            Key::ApiInvalidSession => "Invalid session (account id is unavailable)",
            Key::OpUserPlaylist => "load user playlists",
            Key::ApiLikedPlaylistMissing => "Liked Songs playlist not found",
            Key::ApiLibraryPayloadMissing => "The server returned no valid library data",
            Key::OpPlaylistTracks => "load playlist tracks",
            Key::OpSongUrl => "get the playback URL",
            Key::ApiPlaybackUrlUnavailable => {
                "No playback URL is available (sign-in or VIP may be required)"
            }
            Key::OpLyrics => "load lyrics",
            Key::OpSearch => "search for songs",
            Key::OpFetchCover => "download cover art",
            Key::OpReadCover => "read cover data",
            Key::OpBuildQr => "build the QR code",
        },
        Lang::Ja => match key {
            Key::Resolving => "読み込み中…",
            Key::UnmSourceUsed => "UNMで音源を切り替えました",
            Key::TrackUnavailable => "この曲は再生できません",
            Key::QueueFinished => "キューの再生が終了しました",
            Key::FetchingQr => "QRコードを取得中…",
            Key::ScanQr => "NetEase Cloud Musicアプリでスキャン",
            Key::QrScannedConfirm => "スキャン済みです。スマートフォンで確認してください…",
            Key::QrExpired => "QRコードの期限切れです。メインメニューから再スキャンしてください",
            Key::NetworkRetrying => "通信が不安定です。再試行中…",
            Key::SessionExpired => "ログイン期限切れです。メインメニューから再スキャンしてください",
            Key::NowPlaying => "再生中",
            Key::Cover => "カバー",
            Key::Library => "ライブラリ",
            Key::Search => "検索",
            Key::Queue => "キュー",
            Key::Settings => "設定",
            Key::SettingTheme => "テーマ",
            Key::SettingThemeMode => "外観",
            Key::SettingLanguage => "言語（再起動後）",
            Key::SettingQuality => "音質",
            Key::SettingCoverMode => "カバーモード（再起動後）",
            Key::SettingCoverPalette => "カバー配色",
            Key::SettingCoverDetail => "カバー分割",
            Key::SettingLayout => "再生レイアウト",
            Key::SettingProgressStyle => "進行表示",
            Key::SettingPixelDetail => "ピクセル詳細",
            Key::SettingSpectrumEnabled => "スペクトラム",
            Key::SettingSpectrumStyle => "スペクトラム表示",
            Key::SettingSpectrumGlow => "蛍光残像",
            Key::SettingQueueBehavior => "Enter の動作",
            Key::SettingIcons => "アイコン",
            Key::SettingQueueList => "一覧をキューへ",
            Key::SettingQueueSingle => "1曲だけ再生",
            Key::SettingAdjust => "調整",
            Key::On => "オン",
            Key::Off => "オフ",
            Key::SettingsHint => "j/k 選択 · h/l プレビュー · Enter 保存 · Esc キャンセル",
            Key::PixelDetailHint => {
                "3.0× / 4.0× は quad / sextant / octant のカバー分割と併用してください"
            }
            Key::NerdFontHint => {
                "端末のフォントはNerd Font対応が必要です\nmacOS: brew install font-symbols-only-nerd-font"
            }
            Key::NerdFontDetectedHint => {
                "✓ Nerd Fontを検出しました（端末フォントに選択してください）"
            }
            Key::OctantFontRequired => "フォント対応必須",
            Key::OctantFontHint => {
                "octant は Unicode 16 対応フォントが必要です\n□ 表示時は sextant に戻してください"
            }
            Key::SettingsSaved => "設定を保存しました",
            Key::SettingsSaveFailed => "設定を保存できませんでした",
            Key::Save => "保存",
            Key::QuitQuestion => "ypmを終了しますか？",
            Key::Quit => "終了",
            Key::Cancel => "キャンセル",
            Key::Play => "再生",
            Key::Select => "選択",
            Key::TopBottom => "先頭/末尾",
            Key::Back => "戻る",
            Key::ChangeTrack => "曲を切替",
            Key::Page => "ページ移動",
            Key::Filter => "現在の一覧を絞り込む",
            Key::ClearFilter => "絞り込みを解除",
            Key::FinishFilter => "入力を完了",
            Key::AddToQueue => "キューに追加",
            Key::AddedToQueue => "キューに追加しました",
            Key::Mute => "ミュート",
            Key::Shuffle => "シャッフル オン / オフ",
            Key::Repeat => "リスト / 1曲 / オフ",
            Key::LibraryFocus => "ライブラリ操作 / 検索種類",
            Key::Pause => "一時停止",
            Key::Seek => "シーク",
            Key::Volume => "音量",
            Key::Zen => "集中表示",
            Key::Spectrum => "スペクトラム切替",
            Key::SpectrumPreview => "ライブプレビュー",
            Key::SearchPrompt => "検索",
            Key::SearchSongs => "曲",
            Key::SearchArtists => "アーティスト",
            Key::SearchAlbums => "アルバム",
            Key::SearchPlaylists => "プレイリスト",
            Key::SearchChannelSwitch => "種類を切替",
            Key::Open => "開く",
            Key::HelpTitle => "キー操作",
            Key::HelpAnyKey => "任意のキーで閉じる",
            Key::CommandPalette => "コマンドパレット",
            Key::CommandPaletteHint => "↑/↓ 選択 · Enter 実行 · Esc 閉じる",
            Key::CommandNoMatches => "一致するコマンドがありません",
            Key::CommandPlayPause => "再生 / 一時停止",
            Key::CommandNext => "次の曲",
            Key::CommandPrev => "前の曲",
            Key::CommandShuffle => "シャッフル",
            Key::CommandRepeat => "リピート",
            Key::CommandLike => "お気に入り",
            Key::CommandZen => "禅モード",
            Key::CommandSpectrum => "スペクトラム",
            Key::CommandMute => "ミュート",
            Key::CommandSeek => "シーク <±秒>",
            Key::CommandVolume => "音量 <0-100>",
            Key::CommandTheme => "テーマ <名称>",
            Key::CommandQuality => "音質 <段階>",
            Key::CommandMouse => "マウスキャプチャ",
            Key::CommandGoto => "移動 <ビュー>",
            Key::CommandQuit => "終了",
            Key::SettingUpdateCheck => "更新チェック",
            Key::LabelLike => "お気に入り",
            Key::LabelHelp => "すべてのキー",
            Key::Searching => "検索中…",
            Key::SearchFailed => "検索できませんでした。後でもう一度お試しください",
            Key::NoResults => "見つかりませんでした",
            Key::TypeToSearch => "キーワードを入力して Enter",
            Key::Liked => "お気に入りに追加",
            Key::Unliked => "お気に入り解除",
            Key::LikeFailed => "追加に失敗",
            Key::LoginTitle => "NetEase Cloud Musicにログイン",
            Key::LoginInstruction => {
                "NetEase Cloud Musicアプリのスキャン機能を使用してください（カメラアプリは使用不可）"
            }
            Key::SignedOut => "未ログイン",
            Key::LikedSongs => "お気に入り",
            Key::DailyRecommendations => "デイリーレコメンド",
            Key::PersonalFm => "パーソナルFM",
            Key::CloudDrive => "クラウド",
            Key::SyncingLibrary => "同期中…",
            Key::EmptyLibrary => "まだ何もありません",
            Key::ColumnTitle => "曲名",
            Key::ColumnArtist => "アーティスト",
            Key::ColumnAlbum => "アルバム",
            Key::ColumnDuration => "時間",
            Key::EmptyQueue => "キューは空です。ライブラリでEnterを押すと一覧を追加できます",
            Key::Relogin => "もう一度スキャン",
            Key::ScanLogin => "QRコードでログイン",
            Key::OpQrKey => "QRコードキーの取得",
            Key::ApiQrKeyMissing => "QRコードの応答にunikeyがありません",
            Key::OpQrCheck => "QRコード状態の確認",
            Key::ApiLoginCookieMissing => "ログイン成功後の応答にMUSIC_U cookieがありません",
            Key::OpPersistSession => "ログイン情報の保存",
            Key::OpAccount => "アカウント情報の取得",
            Key::ApiInvalidSession => "ログイン情報が無効です（アカウントIDを取得できません）",
            Key::OpUserPlaylist => "ユーザープレイリストの取得",
            Key::ApiLikedPlaylistMissing => "お気に入りプレイリストが見つかりません",
            Key::ApiLibraryPayloadMissing => "サーバーから有効なライブラリデータが返されませんでした",
            Key::OpPlaylistTracks => "プレイリスト曲の取得",
            Key::OpSongUrl => "再生URLの取得",
            Key::ApiPlaybackUrlUnavailable => {
                "再生URLを取得できません（ログインまたはVIPが必要な場合があります）"
            }
            Key::OpLyrics => "歌詞の取得",
            Key::OpSearch => "曲の検索",
            Key::OpFetchCover => "カバー画像のダウンロード",
            Key::OpReadCover => "カバー画像データの読み込み",
            Key::OpBuildQr => "QRコードの生成",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        init, result_count_for, t, t_for, t_login_interrupted, track_count_for, Key, Lang,
    };

    #[test]
    fn comparable_keys_are_complete_and_distinct_in_all_languages() {
        for key in [
            Key::Quit,
            Key::LikedSongs,
            Key::SyncingLibrary,
            Key::Seek,
            Key::LabelLike,
            Key::LabelHelp,
            Key::SignedOut,
            Key::ColumnAlbum,
            Key::Cover,
            Key::SearchSongs,
            Key::SearchArtists,
            Key::SearchAlbums,
            Key::SearchPlaylists,
            Key::SearchChannelSwitch,
            Key::Open,
        ] {
            let translations = [
                t_for(Lang::Zh, key),
                t_for(Lang::En, key),
                t_for(Lang::Ja, key),
            ];
            assert!(translations.iter().all(|text| !text.is_empty()));
            assert_ne!(translations[0], translations[1]);
            assert_ne!(translations[0], translations[2]);
            assert_ne!(translations[1], translations[2]);
        }
    }

    #[test]
    fn compact_account_and_album_labels_match_each_language() {
        for (lang, signed_out, album) in [
            (Lang::Zh, "未登录", "专辑"),
            (Lang::En, "Signed out", "Album"),
            (Lang::Ja, "未ログイン", "アルバム"),
        ] {
            assert_eq!(t_for(lang, Key::SignedOut), signed_out);
            assert_eq!(t_for(lang, Key::ColumnAlbum), album);
            assert!(!t_for(lang, Key::SignedOut).contains('·'));
        }
    }

    #[test]
    fn cover_panel_title_matches_each_language() {
        for (lang, cover) in [
            (Lang::Zh, "封面"),
            (Lang::En, "Cover"),
            (Lang::Ja, "カバー"),
        ] {
            assert_eq!(t_for(lang, Key::Cover), cover);
        }
    }

    #[test]
    fn octant_font_warning_is_localized_and_actionable() {
        for (lang, required, fallback) in [
            (Lang::Zh, "需字体支持", "显示方框时请切回 sextant"),
            (
                Lang::En,
                "font support required",
                "If □ appears, switch back to sextant",
            ),
            (
                Lang::Ja,
                "フォント対応必須",
                "□ 表示時は sextant に戻してください",
            ),
        ] {
            assert_eq!(t_for(lang, Key::OctantFontRequired), required);
            assert!(t_for(lang, Key::OctantFontHint).contains("Unicode 16"));
            assert!(t_for(lang, Key::OctantFontHint).contains(fallback));
        }
    }

    #[test]
    fn language_config_falls_back_to_chinese() {
        assert_eq!(Lang::from_config("zh"), Lang::Zh);
        assert_eq!(Lang::from_config("en"), Lang::En);
        assert_eq!(Lang::from_config("ja"), Lang::Ja);
        assert_eq!(Lang::from_config("fr"), Lang::Zh);
    }

    #[test]
    fn parameterized_song_counts_follow_each_language() {
        assert_eq!(track_count_for(Lang::Zh, 12), "12 首");
        assert_eq!(track_count_for(Lang::En, 12), "12 tracks");
        assert_eq!(track_count_for(Lang::Ja, 12), "12曲");
    }

    #[test]
    fn parameterized_search_result_counts_follow_each_language() {
        assert_eq!(result_count_for(Lang::Zh, 12), "12 项");
        assert_eq!(result_count_for(Lang::En, 12), "12 results");
        assert_eq!(result_count_for(Lang::Ja, 12), "12件");
    }

    #[test]
    fn login_retry_messages_point_back_to_the_main_menu() {
        for (lang, marker) in [
            (Lang::Zh, "主菜单"),
            (Lang::En, "main menu"),
            (Lang::Ja, "メインメニュー"),
        ] {
            for message in [
                t_for(lang, Key::QrExpired).to_owned(),
                t_for(lang, Key::SessionExpired).to_owned(),
            ] {
                assert!(message.contains(marker), "{lang:?}: {message}");
            }
        }
        let interrupted = t_login_interrupted("offline");
        assert!(["主菜单", "main menu", "メインメニュー"]
            .iter()
            .any(|marker| interrupted.contains(marker)));
    }

    #[test]
    fn global_init_drives_public_translation_functions() {
        init(Lang::En);
        assert_eq!(t(Key::Quit), "Quit");
        assert_eq!(super::t_track_count(3), "3 tracks");
    }
}
