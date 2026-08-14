use yesplaymusic_core::cache::AudioQuality;

use crate::action::View;
use crate::i18n::Key;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandKind {
    TogglePlay,
    Next,
    Prev,
    Shuffle,
    Repeat,
    Like,
    Zen,
    Spectrum,
    Mute,
    Seek,
    Volume,
    Theme,
    Quality,
    Mouse,
    Goto,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CommandSpec {
    kind: CommandKind,
    pub(crate) usage: &'static str,
    pub(crate) alias: Key,
    keywords: &'static [&'static str],
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        kind: CommandKind::TogglePlay,
        usage: "play/pause",
        alias: Key::CommandPlayPause,
        keywords: &["play/pause", "play", "pause", "播放/暂停", "播放", "暂停"],
    },
    CommandSpec {
        kind: CommandKind::Next,
        usage: "next",
        alias: Key::CommandNext,
        keywords: &["next", "下一首", "下一曲"],
    },
    CommandSpec {
        kind: CommandKind::Prev,
        usage: "prev",
        alias: Key::CommandPrev,
        keywords: &["prev", "previous", "上一首", "上一曲"],
    },
    CommandSpec {
        kind: CommandKind::Shuffle,
        usage: "shuffle",
        alias: Key::CommandShuffle,
        keywords: &["shuffle", "random", "随机", "随机播放"],
    },
    CommandSpec {
        kind: CommandKind::Repeat,
        usage: "repeat",
        alias: Key::CommandRepeat,
        keywords: &["repeat", "循环", "重复"],
    },
    CommandSpec {
        kind: CommandKind::Like,
        usage: "like",
        alias: Key::CommandLike,
        keywords: &["like", "喜欢", "收藏"],
    },
    CommandSpec {
        kind: CommandKind::Zen,
        usage: "zen",
        alias: Key::CommandZen,
        keywords: &["zen", "纯净", "专注"],
    },
    CommandSpec {
        kind: CommandKind::Spectrum,
        usage: "spectrum",
        alias: Key::CommandSpectrum,
        keywords: &["spectrum", "visualizer", "频谱", "可视化"],
    },
    CommandSpec {
        kind: CommandKind::Mute,
        usage: "mute",
        alias: Key::CommandMute,
        keywords: &["mute", "静音"],
    },
    CommandSpec {
        kind: CommandKind::Seek,
        usage: "seek <±seconds>",
        alias: Key::CommandSeek,
        keywords: &["seek", "跳转", "定位", "快进", "快退"],
    },
    CommandSpec {
        kind: CommandKind::Volume,
        usage: "volume <0-100>",
        alias: Key::CommandVolume,
        keywords: &["volume", "vol", "音量"],
    },
    CommandSpec {
        kind: CommandKind::Theme,
        usage: "theme <name>",
        alias: Key::CommandTheme,
        keywords: &["theme", "主题", "配色"],
    },
    CommandSpec {
        kind: CommandKind::Quality,
        usage: "quality <level>",
        alias: Key::CommandQuality,
        keywords: &["quality", "音质", "品质"],
    },
    CommandSpec {
        kind: CommandKind::Mouse,
        usage: "mouse",
        alias: Key::CommandMouse,
        keywords: &["mouse", "select", "copy", "鼠标", "选择", "复制"],
    },
    CommandSpec {
        kind: CommandKind::Goto,
        usage: "goto <view>",
        alias: Key::CommandGoto,
        keywords: &["goto", "go", "前往", "转到", "视图"],
    },
    CommandSpec {
        kind: CommandKind::Quit,
        usage: "quit",
        alias: Key::CommandQuit,
        keywords: &["quit", "exit", "退出", "离开"],
    },
];

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CommandInvocation {
    TogglePlay,
    Next,
    Prev,
    Shuffle,
    Repeat,
    Like,
    Zen,
    Spectrum,
    Mute,
    Seek(i64),
    Volume(u8),
    Theme(String),
    Quality(AudioQuality),
    Mouse,
    Goto(View),
    Quit,
}

impl CommandInvocation {
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Self::TogglePlay => "play/pause",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Shuffle => "shuffle",
            Self::Repeat => "repeat",
            Self::Like => "like",
            Self::Zen => "zen",
            Self::Spectrum => "spectrum",
            Self::Mute => "mute",
            Self::Seek(_) => "seek",
            Self::Volume(_) => "volume",
            Self::Theme(_) => "theme",
            Self::Quality(_) => "quality",
            Self::Mouse => "mouse",
            Self::Goto(_) => "goto",
            Self::Quit => "quit",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandError {
    Unknown(String),
    MissingArgument {
        command: &'static str,
        expected: &'static str,
    },
    UnexpectedArgument {
        command: &'static str,
    },
    InvalidArgument {
        command: &'static str,
        value: String,
        expected: &'static str,
    },
}

#[derive(Debug, Default)]
pub(crate) struct CommandPaletteState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) selected: usize,
}

impl CommandPaletteState {
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    pub(crate) fn push(&mut self, character: char) {
        if !character.is_control() {
            self.query.push(character);
            self.selected = 0;
        }
    }

    pub(crate) fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    pub(crate) fn paste(&mut self, text: &str) {
        for character in text.chars() {
            match character {
                '\r' | '\n' | '\t' if !self.query.ends_with(' ') => self.query.push(' '),
                '\r' | '\n' | '\t' => {}
                character if !character.is_control() => self.query.push(character),
                _ => {}
            }
        }
        self.selected = 0;
    }

    pub(crate) fn move_selection(&mut self, delta: i32) {
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        // Wrap around like every palette does.
        self.selected = (self.selected as i32 + delta).rem_euclid(len as i32) as usize;
    }

    pub(crate) fn select_first(&mut self) {
        self.selected = 0;
    }

    pub(crate) fn select_last(&mut self) {
        self.selected = self.filtered().len().saturating_sub(1);
    }

    pub(crate) fn filtered(&self) -> Vec<&'static CommandSpec> {
        let token = self.query.split_whitespace().next().unwrap_or_default();
        filtered_commands(token)
    }

    pub(crate) fn invocation(&self) -> Result<CommandInvocation, CommandError> {
        let input = self.query.trim();
        if input.is_empty() {
            let spec = self
                .filtered()
                .get(self.selected)
                .copied()
                .ok_or_else(|| CommandError::Unknown(String::new()))?;
            return invocation_for(spec, &[]);
        }

        let mut words = input.split_whitespace();
        let token = words.next().unwrap_or_default();
        let arguments = words.collect::<Vec<_>>();
        let exact = command_for_keyword(token);
        let spec = exact.or_else(|| self.filtered().get(self.selected).copied());
        let Some(spec) = spec else {
            return Err(CommandError::Unknown(token.to_owned()));
        };
        invocation_for(spec, &arguments)
    }
}

fn filtered_commands(token: &str) -> Vec<&'static CommandSpec> {
    if token.is_empty() {
        return COMMANDS.iter().collect();
    }
    let mut matches = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            command
                .keywords
                .iter()
                .filter_map(|keyword| fuzzy_score(token, keyword))
                .min()
                .map(|score| (score, index, command))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(score, index, _)| (*score, *index));
    matches.into_iter().map(|(_, _, command)| command).collect()
}

fn command_for_keyword(token: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|command| {
        command
            .keywords
            .iter()
            .any(|keyword| keyword.eq_ignore_ascii_case(token))
    })
}

fn fuzzy_score(needle: &str, target: &str) -> Option<usize> {
    let needle = needle.to_lowercase();
    let target = target.to_lowercase();
    if needle == target {
        return Some(0);
    }
    if target.starts_with(&needle) {
        return Some(
            10 + target
                .chars()
                .count()
                .saturating_sub(needle.chars().count()),
        );
    }

    let target = target.chars().collect::<Vec<_>>();
    let mut cursor = 0;
    let mut first = None;
    let mut previous = None;
    let mut gaps = 0;
    for wanted in needle.chars() {
        let relative = target[cursor..]
            .iter()
            .position(|candidate| *candidate == wanted)?;
        let position = cursor + relative;
        first.get_or_insert(position);
        if let Some(previous) = previous {
            gaps += position.saturating_sub(previous + 1);
        }
        previous = Some(position);
        cursor = position + 1;
    }
    Some(100 + first.unwrap_or_default() * 4 + gaps + target.len().saturating_sub(cursor))
}

fn invocation_for(
    spec: &CommandSpec,
    arguments: &[&str],
) -> Result<CommandInvocation, CommandError> {
    match spec.kind {
        CommandKind::TogglePlay => no_arguments(spec, arguments, CommandInvocation::TogglePlay),
        CommandKind::Next => no_arguments(spec, arguments, CommandInvocation::Next),
        CommandKind::Prev => no_arguments(spec, arguments, CommandInvocation::Prev),
        CommandKind::Shuffle => no_arguments(spec, arguments, CommandInvocation::Shuffle),
        CommandKind::Repeat => no_arguments(spec, arguments, CommandInvocation::Repeat),
        CommandKind::Like => no_arguments(spec, arguments, CommandInvocation::Like),
        CommandKind::Zen => no_arguments(spec, arguments, CommandInvocation::Zen),
        CommandKind::Spectrum => no_arguments(spec, arguments, CommandInvocation::Spectrum),
        CommandKind::Mute => no_arguments(spec, arguments, CommandInvocation::Mute),
        CommandKind::Mouse => no_arguments(spec, arguments, CommandInvocation::Mouse),
        CommandKind::Quit => no_arguments(spec, arguments, CommandInvocation::Quit),
        CommandKind::Seek => {
            let value = one_argument(spec, arguments, "<±seconds>")?;
            let seconds = value
                .parse::<i64>()
                .map_err(|_| CommandError::InvalidArgument {
                    command: spec.usage,
                    value: value.to_owned(),
                    expected: "a signed number of seconds",
                })?;
            Ok(CommandInvocation::Seek(seconds))
        }
        CommandKind::Volume => {
            let value = one_argument(spec, arguments, "<0-100>")?;
            let volume = value
                .parse::<u8>()
                .ok()
                .filter(|volume| *volume <= 100)
                .ok_or_else(|| CommandError::InvalidArgument {
                    command: spec.usage,
                    value: value.to_owned(),
                    expected: "an integer from 0 to 100",
                })?;
            Ok(CommandInvocation::Volume(volume))
        }
        CommandKind::Theme => {
            let value = one_argument(spec, arguments, "<name>")?;
            let value = value.to_ascii_lowercase();
            if !crate::theme::BUILTIN_NAMES.contains(&value.as_str()) {
                return Err(CommandError::InvalidArgument {
                    command: spec.usage,
                    value,
                    expected: "a built-in theme name",
                });
            }
            Ok(CommandInvocation::Theme(value))
        }
        CommandKind::Quality => {
            let value = one_argument(spec, arguments, "<level>")?;
            let quality = parse_quality(value).ok_or_else(|| CommandError::InvalidArgument {
                command: spec.usage,
                value: value.to_owned(),
                expected: "128, 192, 320/exhigh, lossless, or hires",
            })?;
            Ok(CommandInvocation::Quality(quality))
        }
        CommandKind::Goto => {
            let value = one_argument(spec, arguments, "<view>")?;
            let view = parse_view(value).ok_or_else(|| CommandError::InvalidArgument {
                command: spec.usage,
                value: value.to_owned(),
                expected: "playing, library, search, queue, login, or settings",
            })?;
            Ok(CommandInvocation::Goto(view))
        }
    }
}

fn no_arguments(
    spec: &CommandSpec,
    arguments: &[&str],
    invocation: CommandInvocation,
) -> Result<CommandInvocation, CommandError> {
    if arguments.is_empty() {
        Ok(invocation)
    } else {
        Err(CommandError::UnexpectedArgument {
            command: spec.usage,
        })
    }
}

fn one_argument<'a>(
    spec: &CommandSpec,
    arguments: &'a [&str],
    expected: &'static str,
) -> Result<&'a str, CommandError> {
    match arguments {
        [] => Err(CommandError::MissingArgument {
            command: spec.usage,
            expected,
        }),
        [value] => Ok(value),
        _ => Err(CommandError::UnexpectedArgument {
            command: spec.usage,
        }),
    }
}

fn parse_quality(value: &str) -> Option<AudioQuality> {
    match value.to_ascii_lowercase().as_str() {
        "128" | "128k" | "low" => Some(AudioQuality::Low128),
        "192" | "192k" | "medium" => Some(AudioQuality::Medium192),
        "320" | "320k" | "exhigh" | "high" => Some(AudioQuality::High320),
        "lossless" | "无损" => Some(AudioQuality::Lossless),
        "hires" | "hi-res" | "高清" => Some(AudioQuality::HiRes),
        _ => None,
    }
}

fn parse_view(value: &str) -> Option<View> {
    match value.to_ascii_lowercase().as_str() {
        "playing" | "player" | "now" | "now-playing" | "now_playing" | "正在播放" | "播放页" => {
            Some(View::NowPlaying)
        }
        "library" | "曲库" | "音乐库" => Some(View::Library),
        "search" | "搜索" => Some(View::Search),
        "queue" | "队列" | "播放队列" => Some(View::Queue),
        "login" | "登录" => Some(View::Login),
        "settings" | "setting" | "设置" => Some(View::Settings),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(query: &str) -> CommandPaletteState {
        CommandPaletteState {
            open: true,
            query: query.to_owned(),
            selected: 0,
        }
    }

    #[test]
    fn fuzzy_filter_matches_english_and_chinese_keywords() {
        let english = palette("spct").filtered();
        assert_eq!(
            english.first().map(|command| command.usage),
            Some("spectrum")
        );

        let chinese = palette("主题").filtered();
        assert_eq!(chinese.len(), 1);
        assert_eq!(chinese[0].usage, "theme <name>");

        let parameter_text_does_not_hide_its_command = palette("theme tokyo").filtered();
        assert_eq!(parameter_text_does_not_hide_its_command.len(), 1);
        assert_eq!(
            parameter_text_does_not_hide_its_command[0].usage,
            "theme <name>"
        );
    }

    #[test]
    fn english_and_chinese_aliases_execute_the_same_command() {
        assert_eq!(palette("next").invocation(), Ok(CommandInvocation::Next));
        assert_eq!(palette("下一首").invocation(), Ok(CommandInvocation::Next));
        assert_eq!(
            palette("pause").invocation(),
            Ok(CommandInvocation::TogglePlay)
        );
        assert_eq!(
            palette("暂停").invocation(),
            Ok(CommandInvocation::TogglePlay)
        );
    }

    #[test]
    fn parameter_commands_parse_and_validate_their_ranges() {
        assert_eq!(
            palette("seek -12").invocation(),
            Ok(CommandInvocation::Seek(-12))
        );
        assert_eq!(
            palette("跳转 +8").invocation(),
            Ok(CommandInvocation::Seek(8))
        );
        assert_eq!(
            palette("volume 73").invocation(),
            Ok(CommandInvocation::Volume(73))
        );
        assert!(matches!(
            palette("volume 101").invocation(),
            Err(CommandError::InvalidArgument { .. })
        ));
        assert!(matches!(
            palette("seek soon").invocation(),
            Err(CommandError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn setting_and_navigation_arguments_are_narrowed() {
        assert_eq!(
            palette("主题 tokyo-night").invocation(),
            Ok(CommandInvocation::Theme("tokyo-night".into()))
        );
        assert_eq!(
            palette("theme PICO8").invocation(),
            Ok(CommandInvocation::Theme("pico8".into()))
        );
        assert_eq!(
            palette("音质 无损").invocation(),
            Ok(CommandInvocation::Quality(AudioQuality::Lossless))
        );
        assert_eq!(
            palette("goto queue").invocation(),
            Ok(CommandInvocation::Goto(View::Queue))
        );
        assert_eq!(
            palette("前往 曲库").invocation(),
            Ok(CommandInvocation::Goto(View::Library))
        );
        assert!(matches!(
            palette("theme unknown").invocation(),
            Err(CommandError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn edits_reset_selection_and_paste_flattens_terminal_newlines() {
        let mut state = palette("");
        state.move_selection(3);
        assert_eq!(state.selected, 3);
        state.push('主');
        assert_eq!(state.selected, 0);
        state.paste("题\ntokyo-night");
        assert_eq!(state.query, "主题 tokyo-night");
        state.backspace();
        assert_eq!(state.selected, 0);
    }
}
