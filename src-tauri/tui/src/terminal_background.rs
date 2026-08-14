//! OSC 11 terminal background detection.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "macos"))]
const OSC_11_WITH_DA1_QUERY: &[u8] = b"\x1b]11;?\x07\x1b[c";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const DA1_QUERY: &[u8] = b"\x1b[c";
// A terminal that answers at all sends the first byte quickly relative to
// its own load, but a busy multiplexer can take well over a second (seen
// with Herdr under compile load). Waiting only bounds startup on the rare
// terminal that never answers DA1; giving up early leaks the late reply
// into the input stream, which is far worse.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(any(target_os = "linux", target_os = "macos"))]
const IDLE_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(any(target_os = "linux", target_os = "macos"))]
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);
// After a drain barrier completes, sweep briefly until the line is quiet
// so stale replies from earlier probers cannot leak past the barrier. The
// cap must exceed FIRST_BYTE_TIMEOUT: a dirty barrier waits that long for
// its own delayed reply before the quiet check takes over.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const QUIET_WINDOW: Duration = Duration::from_millis(150);
#[cfg(any(target_os = "linux", target_os = "macos"))]
const QUIET_SWEEP_CAP: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Appearance {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    pub(crate) fn appearance(self) -> Appearance {
        fn linear(channel: u8) -> f64 {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }

        let luminance =
            0.2126 * linear(self.red) + 0.7152 * linear(self.green) + 0.0722 * linear(self.blue);
        if luminance > 0.179 {
            Appearance::Light
        } else {
            Appearance::Dark
        }
    }

    pub(crate) const fn color(self) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(self.red, self.green, self.blue)
    }
}

pub(crate) fn probe() -> Option<Rgb> {
    platform::read_response()
}

/// Sweep stray query responses out of the tty before the event stream owns
/// stdin. Third-party startup probes (ratatui-image sends its own OSC 11)
/// stop reading before a slow terminal finishes answering; the leftover
/// bytes would otherwise arrive as phantom key presses. DA1 acts as the
/// sync point: every real terminal answers it, and it always comes last.
pub(crate) fn drain_pending_responses() {
    platform::drain_pending();
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
#[derive(Debug, Eq, PartialEq)]
enum ProbeResponse {
    AwaitingDa1,
    Complete(Option<Rgb>),
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn parse_response(response: &[u8]) -> ProbeResponse {
    let Some(da1_end) = find_da1_end(response) else {
        return ProbeResponse::AwaitingDa1;
    };
    ProbeResponse::Complete(parse_osc_11(&response[..da1_end]))
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn parse_osc_11(response: &[u8]) -> Option<Rgb> {
    for prefix in [b"\x1b]11;rgb:".as_slice(), b"\x9d11;rgb:".as_slice()] {
        for (offset, window) in response.windows(prefix.len()).enumerate() {
            if window == prefix {
                if let Some(rgb) = parse_rgb(&response[offset + prefix.len()..]) {
                    return Some(rgb);
                }
            }
        }
    }
    None
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn find_da1_end(response: &[u8]) -> Option<usize> {
    for (offset, byte) in response.iter().enumerate() {
        let parameter_start = if response[offset..].starts_with(b"\x1b[?") {
            offset + 3
        } else if *byte == 0x9b && response.get(offset + 1) == Some(&b'?') {
            offset + 2
        } else {
            continue;
        };

        let mut cursor = parameter_start;
        while response
            .get(cursor)
            .is_some_and(|value| (0x30..=0x3f).contains(value))
        {
            cursor += 1;
        }
        if cursor == parameter_start {
            continue;
        }
        while response
            .get(cursor)
            .is_some_and(|value| (0x20..=0x2f).contains(value))
        {
            cursor += 1;
        }
        if response.get(cursor) == Some(&b'c') {
            return Some(cursor + 1);
        }
    }
    None
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn parse_rgb(input: &[u8]) -> Option<Rgb> {
    let mut cursor = 0;
    let red = parse_component(input, &mut cursor)?;
    expect(input, &mut cursor, b'/')?;
    let green = parse_component(input, &mut cursor)?;
    expect(input, &mut cursor, b'/')?;
    let blue = parse_component(input, &mut cursor)?;

    match input.get(cursor..) {
        Some([0x07, ..] | [0x9c, ..] | [0x1b, b'\\', ..]) => Some(Rgb { red, green, blue }),
        _ => None,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn parse_component(input: &[u8], cursor: &mut usize) -> Option<u8> {
    let start = *cursor;
    while input.get(*cursor).is_some_and(u8::is_ascii_hexdigit) {
        *cursor += 1;
    }
    let digits = input.get(start..*cursor)?;
    if !(1..=4).contains(&digits.len()) {
        return None;
    }

    let text = std::str::from_utf8(digits).ok()?;
    let value = u32::from_str_radix(text, 16).ok()?;
    let maximum = (1_u32 << (digits.len() * 4)) - 1;
    Some(((value * 255 + maximum / 2) / maximum) as u8)
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn expect(input: &[u8], cursor: &mut usize, expected: u8) -> Option<()> {
    if input.get(*cursor) != Some(&expected) {
        return None;
    }
    *cursor += 1;
    Some(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use std::ffi::{c_int, c_short};
    use std::fs::{File, OpenOptions};
    use std::io::{self, IsTerminal, Read, Write};
    use std::mem::ManuallyDrop;
    use std::os::fd::{FromRawFd, RawFd};
    use std::time::{Duration, Instant};

    use super::{
        find_da1_end, parse_response, ProbeResponse, Rgb, DA1_QUERY, FIRST_BYTE_TIMEOUT,
        IDLE_TIMEOUT, OSC_11_WITH_DA1_QUERY, QUIET_SWEEP_CAP, QUIET_WINDOW, TOTAL_TIMEOUT,
    };

    const POLLIN: c_short = 0x0001;

    #[cfg(target_os = "linux")]
    type PollCount = std::ffi::c_ulong;
    #[cfg(target_os = "macos")]
    type PollCount = std::ffi::c_uint;

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: c_short,
        revents: c_short,
    }

    unsafe extern "C" {
        #[link_name = "poll"]
        fn system_poll(descriptors: *mut PollFd, count: PollCount, timeout: c_int) -> c_int;
    }

    pub(super) fn read_response() -> Option<Rgb> {
        match parse_response(&transact(OSC_11_WITH_DA1_QUERY)?) {
            ProbeResponse::Complete(rgb) => rgb,
            ProbeResponse::AwaitingDa1 => None,
        }
    }

    pub(super) fn drain_pending() {
        // A stale DA1 reply left over from an earlier prober (e.g. a timed
        // out Picker query) can satisfy the barrier before the reply to
        // OUR query arrives. After the barrier, keep sweeping until the
        // line goes quiet so no reply survives into the event stream.
        let Some(response) = transact(DA1_QUERY) else {
            return;
        };
        // A buffer that is exactly one DA1 reply and nothing else is ours:
        // stale probe leftovers (kitty/OSC/cell-size replies) would precede
        // it. When leftovers are present, our own reply may still be queued
        // behind them on a busy multiplexer, so sweep with the full
        // first-byte patience instead of a short quiet check.
        let clean = find_da1_end(&response) == Some(response.len())
            && (response.starts_with(b"\x1b[?") || response.starts_with(&[0x9b]));
        let mut patience = if clean {
            QUIET_WINDOW
        } else {
            FIRST_BYTE_TIMEOUT
        };
        let mut stdin = ManuallyDrop::new(unsafe { File::from_raw_fd(STDIN_FD) });
        let deadline = Instant::now() + QUIET_SWEEP_CAP;
        loop {
            let Some(remaining) = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
            else {
                return;
            };
            match wait_readable(STDIN_FD, patience.min(remaining)) {
                Ok(true) => {
                    patience = QUIET_WINDOW;
                    let mut byte = [0];
                    match stdin.read(&mut byte) {
                        Ok(1) => {}
                        Ok(_) => return,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => return,
                    }
                }
                Ok(false) => return,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    }

    /// Send a query whose reply chain ends in a DA1 response and collect
    /// every byte up to and including that response.
    fn transact(query: &[u8]) -> Option<Vec<u8>> {
        let result = transact_inner(query);
        debug_log(query, result.as_deref());
        result
    }

    // Replies must be read from stdin (fd 0), not from a fresh `/dev/tty`
    // handle: macOS `poll()` reports POLLNVAL for the `/dev/tty` alias
    // device even while its data is readable, so polling it gives up
    // instantly and the reply later leaks into the input stream as
    // phantom key presses.
    const STDIN_FD: RawFd = 0;

    fn transact_inner(query: &[u8]) -> Option<Vec<u8>> {
        match crossterm::terminal::is_raw_mode_enabled() {
            Ok(true) => {}
            other => {
                debug_step(&format!("raw-mode gate failed: {other:?}"));
                return None;
            }
        }
        if !io::stdin().is_terminal() {
            debug_step("stdin is not a terminal");
            return None;
        }

        let mut terminal = match OpenOptions::new().write(true).open("/dev/tty") {
            Ok(terminal) => terminal,
            Err(error) => {
                debug_step(&format!("open /dev/tty failed: {error}"));
                return None;
            }
        };
        if let Err(error) = terminal.write_all(query).and_then(|()| terminal.flush()) {
            debug_step(&format!("query write failed: {error}"));
            return None;
        }
        debug_step("query written");

        // SAFETY: fd 0 outlives this function; ManuallyDrop keeps the
        // borrowed descriptor open when the File wrapper goes away.
        let mut stdin = ManuallyDrop::new(unsafe { File::from_raw_fd(STDIN_FD) });
        let deadline = Instant::now().checked_add(TOTAL_TIMEOUT)?;
        let mut response = Vec::with_capacity(64);
        loop {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let patience = if response.is_empty() {
                FIRST_BYTE_TIMEOUT
            } else {
                IDLE_TIMEOUT
            };
            match wait_readable(STDIN_FD, patience.min(remaining)) {
                Ok(true) => {}
                Ok(false) => {
                    debug_step(&format!("timed out with partial {response:?}"));
                    return None;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }

            let mut byte = [0];
            match stdin.read(&mut byte) {
                Ok(1) => response.push(byte[0]),
                Ok(_) => return None,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
            if find_da1_end(&response).is_some() {
                return Some(response);
            }
        }
    }

    /// `YPM_OSC_DEBUG=<file>` appends every terminal transaction for
    /// diagnosing reply leaks on exotic terminals; off by default.
    fn debug_log(query: &[u8], response: Option<&[u8]>) {
        debug_step(&format!("query={query:?} response={response:?}"));
    }

    fn debug_step(message: &str) {
        let Ok(path) = std::env::var("YPM_OSC_DEBUG") else {
            return;
        };
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{message}");
        }
    }

    fn wait_readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
        let timeout_millis = timeout.as_millis().min(c_int::MAX as u128) as c_int;
        if timeout_millis == 0 {
            return Ok(false);
        }
        let mut descriptor = PollFd {
            fd,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: `fd` stays open and `PollFd` matches the platform C layout.
        let ready = unsafe { system_poll(&raw mut descriptor, 1, timeout_millis) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ready > 0 && descriptor.revents & POLLIN != 0)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::Rgb;

    pub(super) fn read_response() -> Option<Rgb> {
        // Windows has no `/dev/tty`; unsupported platforms skip probing without reading input.
        None
    }

    pub(super) fn drain_pending() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn unsupported_platform_probe_is_silent() {
        assert_eq!(probe(), None);
    }

    #[test]
    fn waits_for_da1_before_accepting_osc() {
        assert_eq!(
            parse_response(b"\x1b]11;rgb:ffff/8080/0000\x07"),
            ProbeResponse::AwaitingDa1
        );
    }

    #[test]
    fn osc_11_and_da1_complete_the_probe() {
        assert_eq!(
            parse_response(b"\x1b]11;rgb:ffff/8080/0000\x07\x1b[?1;2c"),
            ProbeResponse::Complete(Some(Rgb {
                red: 255,
                green: 128,
                blue: 0,
            }))
        );
        assert_eq!(
            parse_response(b"noise\x1b]11;rgb:0000/4040/ffff\x1b\\tail\x9b?62;4c"),
            ProbeResponse::Complete(Some(Rgb {
                red: 0,
                green: 64,
                blue: 255,
            }))
        );
    }

    #[test]
    fn da1_without_osc_11_completes_without_a_color() {
        assert_eq!(parse_response(b"\x1b[?1;2c"), ProbeResponse::Complete(None));
    }

    #[test]
    fn parses_fragmented_responses_with_interleaved_noise() {
        let chunks: [&[u8]; 5] = [
            b"noise:rgb:\x1b]11;rgb:ef",
            b"ef/ebeb/e7",
            b"e7\x1b\\junk\x1b[?6",
            b"2;4",
            b"c",
        ];
        let mut response = Vec::new();
        for chunk in &chunks[..chunks.len() - 1] {
            response.extend_from_slice(chunk);
            assert_eq!(parse_response(&response), ProbeResponse::AwaitingDa1);
        }
        response.extend_from_slice(chunks[chunks.len() - 1]);
        assert_eq!(
            parse_response(&response),
            ProbeResponse::Complete(Some(Rgb {
                red: 239,
                green: 235,
                blue: 231,
            }))
        );
    }

    #[test]
    fn parses_variable_precision_and_c1_terminator() {
        assert_eq!(
            parse_response(b"\x9d11;rgb:f/80/1234\x9c\x9b?62;4c"),
            ProbeResponse::Complete(Some(Rgb {
                red: 255,
                green: 128,
                blue: 18,
            }))
        );
    }

    #[test]
    fn rejects_wrong_or_malformed_responses() {
        for response in [
            b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b[?1;2c".as_slice(),
            b"\x1b]11;rgb:ffff/zzzz/ffff\x07\x1b[?1;2c".as_slice(),
            b"\x1b]11;rgb:fffff/0000/0000\x07\x1b[?1;2c".as_slice(),
            b"\x1b]11;rgb:ffff/0000/0000\x1b[?1;2c".as_slice(),
        ] {
            assert_eq!(parse_response(response), ProbeResponse::Complete(None));
        }
    }

    #[test]
    fn ignores_an_osc_response_after_the_da1_barrier() {
        assert_eq!(
            parse_response(b"\x1b[?1;2c\x1b]11;rgb:ffff/ffff/ffff\x07"),
            ProbeResponse::Complete(None)
        );
    }

    #[test]
    fn classifies_dark_and_light_backgrounds() {
        assert_eq!(
            Rgb {
                red: 0x1a,
                green: 0x1b,
                blue: 0x26,
            }
            .appearance(),
            Appearance::Dark
        );
        assert_eq!(
            Rgb {
                red: 0xf8,
                green: 0xf8,
                blue: 0xf2,
            }
            .appearance(),
            Appearance::Light
        );
    }

    #[test]
    fn preserves_the_detected_rgb_for_rendering() {
        let ProbeResponse::Complete(Some(rgb)) =
            parse_response(b"\x1b]11;rgb:1a1a/2b2b/3c3c\x07\x1b[?1;2c")
        else {
            panic!("expected a completed OSC 11 probe");
        };
        assert_eq!(rgb.color(), ratatui::style::Color::Rgb(0x1a, 0x2b, 0x3c));
    }
}
