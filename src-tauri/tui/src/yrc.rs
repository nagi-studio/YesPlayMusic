//! Parser for NetEase word-synchronised (YRC) lyrics.

use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YrcWord {
    pub text: String,
    pub start: Duration,
    pub duration: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YrcLine {
    pub start: Duration,
    pub duration: Duration,
    pub words: Vec<YrcWord>,
}

impl YrcLine {
    pub fn text(&self) -> String {
        self.words.iter().map(|word| word.text.as_str()).collect()
    }
}

/// Parse YRC's common `[line_start,line_duration](word_start,word_duration,0)word`
/// form and the `(word,word_start,word_duration)` form used by some providers.
/// Malformed metadata lines are ignored without discarding neighbouring words.
pub fn parse_yrc(input: &str) -> Vec<YrcLine> {
    let mut lines = input.lines().filter_map(parse_line).collect::<Vec<_>>();
    lines.sort_by_key(|line| line.start);
    lines
}

fn parse_line(raw_line: &str) -> Option<YrcLine> {
    let line = raw_line
        .trim()
        .strip_prefix('\u{feff}')
        .unwrap_or(raw_line.trim())
        .trim_start();
    let after_open = line.strip_prefix('[')?;
    let close = after_open.find(']')?;
    let (start, duration) = parse_pair(&after_open[..close])?;
    let words = parse_words(&after_open[close + 1..]);
    if words.is_empty() {
        return None;
    }
    Some(YrcLine {
        start: Duration::from_millis(start),
        duration: Duration::from_millis(duration),
        words,
    })
}

fn parse_pair(value: &str) -> Option<(u64, u64)> {
    let mut fields = value.split(',');
    let first = parse_milliseconds(fields.next()?)?;
    let second = parse_milliseconds(fields.next()?)?;
    fields.next().is_none().then_some((first, second))
}

fn parse_words(input: &str) -> Vec<YrcWord> {
    let mut words = Vec::new();
    let mut cursor = 0;
    while let Some(tag) = next_tag(input, cursor) {
        match tag.kind {
            WordTag::TimingFirst { start, duration } => {
                let word_end = next_tag(input, tag.close).map_or(input.len(), |next| next.open);
                let text = input[tag.close..word_end].to_owned();
                if !text.is_empty() {
                    words.push(YrcWord {
                        text,
                        start: Duration::from_millis(start),
                        duration: Duration::from_millis(duration),
                    });
                }
                cursor = word_end;
            }
            WordTag::WordFirst {
                text,
                start,
                duration,
            } => {
                let next_open = next_tag(input, tag.close).map_or(input.len(), |next| next.open);
                let separator = &input[tag.close..next_open];
                let text = format!("{text}{separator}");
                if !text.is_empty() {
                    words.push(word(&text, start, duration));
                }
                cursor = next_open;
            }
        }
    }
    words
}

fn word(text: &str, start: u64, duration: u64) -> YrcWord {
    YrcWord {
        text: text.to_owned(),
        start: Duration::from_millis(start),
        duration: Duration::from_millis(duration),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WordTag<'a> {
    TimingFirst {
        start: u64,
        duration: u64,
    },
    WordFirst {
        text: &'a str,
        start: u64,
        duration: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LocatedTag<'a> {
    open: usize,
    close: usize,
    kind: WordTag<'a>,
}

fn next_tag(input: &str, from: usize) -> Option<LocatedTag<'_>> {
    let mut search_from = from;
    while search_from < input.len() {
        let open = search_from + input[search_from..].find('(')?;
        let relative_close = input[open + 1..].find(')')?;
        let close = open + 1 + relative_close;
        let after_close = &input[close + 1..];
        let external_end = after_close.find('(').unwrap_or(after_close.len());
        let has_external_text = !after_close[..external_end].trim().is_empty();
        if let Some(kind) = parse_word_tag(&input[open + 1..close], has_external_text) {
            return Some(LocatedTag {
                open,
                close: close + 1,
                kind,
            });
        }
        search_from = open + 1;
    }
    None
}

fn parse_word_tag(tag: &str, has_external_text: bool) -> Option<WordTag<'_>> {
    let fields = tag.split(',').map(str::trim).collect::<Vec<_>>();
    let numeric_timing = match fields.as_slice() {
        [start, duration] => parse_milliseconds(start).zip(parse_milliseconds(duration)),
        [start, duration, flag] => match (
            parse_milliseconds(start),
            parse_milliseconds(duration),
            parse_milliseconds(flag),
        ) {
            (Some(start), Some(duration), Some(flag)) if flag == 0 || has_external_text => {
                Some((start, duration))
            }
            _ => None,
        },
        _ => None,
    };
    if let Some((start, duration)) = numeric_timing {
        return Some(WordTag::TimingFirst { start, duration });
    }

    let (word_and_start, duration) = tag.rsplit_once(',')?;
    let (text, start) = word_and_start.rsplit_once(',')?;
    Some(WordTag::WordFirst {
        text,
        start: parse_milliseconds(start.trim())?,
        duration: parse_milliseconds(duration.trim())?,
    })
}

fn parse_milliseconds(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::parse_yrc;

    #[test]
    fn parses_netease_timing_first_words_and_preserves_spacing() {
        let lines =
            parse_yrc("[1000,1800](1000,250,0)风(1250,120,0) (1370,430,0)吹过(1800,700,0)旧站台");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start, Duration::from_millis(1_000));
        assert_eq!(lines[0].duration, Duration::from_millis(1_800));
        assert_eq!(lines[0].text(), "风 吹过旧站台");
        assert_word(&lines[0].words[0], 1_000, 250, "风");
        assert_word(&lines[0].words[1], 1_250, 120, " ");
        assert_word(&lines[0].words[2], 1_370, 430, "吹过");
        assert_word(&lines[0].words[3], 1_800, 700, "旧站台");
    }

    #[test]
    fn parses_word_first_variant_including_commas_and_spaces() {
        let lines = parse_yrc("[3000,1200](Hello, world,3000,400)( ,3400,100)(你好,3500,500)");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "Hello, world 你好");
        assert_word(&lines[0].words[0], 3_000, 400, "Hello, world");
        assert_word(&lines[0].words[1], 3_400, 100, " ");
        assert_word(&lines[0].words[2], 3_500, 500, "你好");
    }

    #[test]
    fn numeric_word_first_tokens_are_not_mistaken_for_timing_tags() {
        let lines = parse_yrc("[3000,700](1,3000,200)( ,3200,100)(2,3300,200)");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "1 2");
        assert_word(&lines[0].words[0], 3_000, 200, "1");
        assert_word(&lines[0].words[2], 3_300, 200, "2");

        let separated = parse_yrc("[3000,500](1,3000,200) (2,3200,200)");
        assert_eq!(separated[0].text(), "1 2");
        assert_eq!(separated[0].words.len(), 2);
        assert_word(&separated[0].words[0], 3_000, 200, "1 ");

        let timing_first = parse_yrc("[4000,300](4000,300,7)3");
        assert_eq!(timing_first[0].text(), "3");
        assert_word(&timing_first[0].words[0], 4_000, 300, "3");
    }

    #[test]
    fn skips_bad_metadata_lines_but_preserves_unknown_tokens_as_text() {
        let lines = parse_yrc(
            r#"
{"t":0,"c":[{"tx":"作词"}]}
[oops,900](0,100,0)坏行
[1000,900](1000,200,0)保留(bad timing)(1200,300,0)括号也保留(1500,nope,0)尾
[2000,800](前,2000,200)(broken)(后,2400,200)
[3000,500]no timed words
"#,
        );

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text(), "保留(bad timing)括号也保留(1500,nope,0)尾");
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[1].text(), "前(broken)后");
        assert_eq!(lines[1].words.len(), 2);
    }

    #[test]
    fn preserves_ascii_parentheses_that_are_lyric_text() {
        let lines =
            parse_yrc("[0,1000](0,1000,0)Hello (world) (feat. A,B) (2024, Remix) (Verse 2, 2024)");

        assert_eq!(
            lines[0].text(),
            "Hello (world) (feat. A,B) (2024, Remix) (Verse 2, 2024)"
        );
    }

    #[test]
    fn sorts_lines_stably() {
        let lines =
            parse_yrc("[2000,500](二,2000,200)\n[1000,500](一,1000,200)\n[2000,400](同,2000,100)");

        assert_eq!(
            lines.iter().map(|line| line.text()).collect::<Vec<_>>(),
            ["一", "二", "同"]
        );
    }

    #[test]
    fn rejects_overflows_and_handles_utf8_around_malformed_parentheses() {
        let lines =
            parse_yrc("[18446744073709551616,1](0,1,0)溢出\n[0,500](0,100,0)你（好）(100,100,0)呀");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "你（好）呀");
        assert_eq!(lines[0].words[0].start, Duration::ZERO);
        assert_eq!(lines[0].words[0].duration, Duration::from_millis(100));
        assert_eq!(lines[0].duration, Duration::from_millis(500));
    }

    fn assert_word(word: &super::YrcWord, start: u64, duration: u64, text: &str) {
        assert_eq!(word.start, Duration::from_millis(start));
        assert_eq!(word.duration, Duration::from_millis(duration));
        assert_eq!(word.text, text);
    }
}
