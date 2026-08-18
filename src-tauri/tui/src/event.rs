//! Terminal input → Action. Arrow keys and vim keys coexist; numbers jump
//! straight to a view (cmus model).

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};

use crate::action::{Action, View};
use crate::ui::Hits;

pub fn action_for(event: Event) -> Option<Action> {
    match event {
        // Keys route through the reducer, which knows whether a text
        // input owns the keyboard right now.
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(Action::RawKey(key)),
        Event::Mouse(mouse) => Some(Action::Mouse(mouse)),
        Event::Resize(cols, rows) => Some(Action::Resize { cols, rows }),
        Event::Paste(text) => Some(Action::Paste(text)),
        // Clicking back into the pane is exactly when a stale frame (e.g. a
        // multiplexer dropped our redraw) gets noticed: repaint to heal it.
        Event::FocusGained => Some(Action::ForceRedraw),
        _ => None,
    }
}

/// Resolve a mouse event against the geometry recorded at draw time.
/// Click a tab to switch; click a row to select, click it again to play;
/// the wheel moves the selection. An open quit-confirm dialog is modal:
/// only its buttons respond.
pub fn mouse_action(mouse: MouseEvent, hits: &Hits, selected: usize) -> Option<Action> {
    let position = ratatui::layout::Position {
        x: mouse.column,
        y: mouse.row,
    };
    if !hits.confirm.is_empty() {
        if let MouseEventKind::Down(crossterm::event::MouseButton::Left) = mouse.kind {
            for (rect, is_confirm) in &hits.confirm {
                if rect.contains(position) {
                    return Some(if *is_confirm {
                        Action::Quit
                    } else {
                        Action::Back
                    });
                }
            }
        }
        return None;
    }
    if let MouseEventKind::Down(crossterm::event::MouseButton::Left) = mouse.kind {
        for (rect, delta) in &hits.settings_adjust {
            if rect.contains(position) {
                return Some(Action::AdjustSetting(*delta));
            }
        }
        for rect in &hits.settings_save {
            if rect.contains(position) {
                return Some(Action::SaveSettings);
            }
        }
        for rect in &hits.settings_cancel {
            if rect.contains(position) {
                return Some(Action::CancelSettings);
            }
        }
        for (rect, index) in &hits.settings_rows {
            if rect.contains(position) {
                return Some(Action::SelectSetting(*index));
            }
        }
    }
    // Progress bar: click or drag inside seeks to that spot. Checked before
    // the volume row rule so a drag over the bar cells always means seeking.
    if matches!(
        mouse.kind,
        MouseEventKind::Down(crossterm::event::MouseButton::Left)
            | MouseEventKind::Drag(crossterm::event::MouseButton::Left)
    ) {
        for (rect, _) in &hits.progress {
            if rect.contains(position) {
                let ratio = (mouse.column.saturating_sub(rect.x) as f64 + 0.5) / rect.width as f64;
                return Some(Action::SeekToRatio(ratio.clamp(0.0, 1.0)));
            }
        }
    }
    // Battery-style volume bar: click or drag anywhere inside sets the level.
    if matches!(
        mouse.kind,
        MouseEventKind::Down(crossterm::event::MouseButton::Left)
            | MouseEventKind::Drag(crossterm::event::MouseButton::Left)
    ) {
        for (rect, _) in &hits.volume {
            if rect.contains(position)
                || matches!(mouse.kind, MouseEventKind::Drag(_)) && mouse.row == rect.y
            {
                // Leftmost cell is exactly 0 (mute), rightmost exactly full.
                let ratio = mouse.column.saturating_sub(rect.x) as f32
                    / f32::from(rect.width.saturating_sub(1).max(1));
                return Some(Action::SetVolumeTo(ratio.clamp(0.0, 1.0)));
            }
        }
    }
    match mouse.kind {
        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
            for (rect, view) in &hits.tabs {
                if rect.contains(position) {
                    return Some(Action::SwitchView(*view));
                }
            }
            for (rect, channel) in &hits.search_channels {
                if rect.contains(position) {
                    return Some(Action::SelectSearchChannel(*channel));
                }
            }
            for (rect, link) in &hits.links {
                if rect.contains(position) {
                    return Some(Action::OpenPage(link.clone()));
                }
            }
            for (rect, _) in &hits.heart {
                if rect.contains(position) {
                    return Some(Action::ToggleLike);
                }
            }
            for (rect, _) in &hits.play {
                if rect.contains(position) {
                    return Some(Action::TogglePlay);
                }
            }
            for (rect, _) in &hits.playback_mode {
                if rect.contains(position) {
                    return Some(Action::CyclePlaybackMode);
                }
            }
            for (rect, index) in &hits.sidebar {
                if rect.contains(position) {
                    return Some(Action::OpenSource(*index));
                }
            }
            for (rect, entry) in &hits.menu {
                if rect.contains(position) {
                    return Some(match entry {
                        crate::action::MenuEntry::Library => Action::SwitchView(View::Library),
                        crate::action::MenuEntry::Search => Action::SwitchView(View::Search),
                        crate::action::MenuEntry::Login => Action::StartLogin,
                        crate::action::MenuEntry::Settings => Action::SwitchView(View::Settings),
                        crate::action::MenuEntry::Quit => Action::Quit,
                    });
                }
            }
            for (rect, index) in &hits.rows {
                if rect.contains(position) {
                    return Some(if *index == selected {
                        Action::Activate
                    } else {
                        Action::SelectIndex(*index)
                    });
                }
            }
            None
        }
        MouseEventKind::ScrollDown => Some(Action::MoveSelection(1)),
        MouseEventKind::ScrollUp => Some(Action::MoveSelection(-1)),
        _ => None,
    }
}

pub fn key_action(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            KeyCode::Char('l') => Some(Action::ForceRedraw),
            // vim half-page jumps
            KeyCode::Char('d') => Some(Action::MoveSelection(10)),
            KeyCode::Char('u') => Some(Action::MoveSelection(-10)),
            _ => None,
        };
    }
    // A Chinese IME turns the shifted punctuation keys into their fullwidth
    // twins (？ ， ：) before the terminal ever sees them; accept both so the
    // bindings survive a forgotten input-method switch.
    match key.code {
        KeyCode::Char(':') | KeyCode::Char('：') => Some(Action::OpenCommandPalette),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('1') => Some(Action::SwitchView(View::NowPlaying)),
        KeyCode::Char('2') => Some(Action::SwitchView(View::Library)),
        KeyCode::Char('3') => Some(Action::SwitchView(View::Search)),
        KeyCode::Char('4') => Some(Action::SwitchView(View::Queue)),
        KeyCode::Char('5') | KeyCode::Char(',') | KeyCode::Char('，') => {
            Some(Action::SwitchView(View::Settings))
        }
        // vim: h backs out, l dives in.
        KeyCode::Backspace | KeyCode::Char('h') => Some(Action::Back),
        KeyCode::Esc => Some(Action::Escape),
        KeyCode::Char('l') | KeyCode::Enter => Some(Action::Activate),
        KeyCode::Char('g') => Some(Action::GKey),
        KeyCode::Char('G') | KeyCode::End => Some(Action::JumpBottom),
        KeyCode::Home => Some(Action::JumpTop),
        KeyCode::Char('y') => Some(Action::ConfirmYes),
        KeyCode::Char('?') | KeyCode::Char('？') => Some(Action::ToggleHelp),
        KeyCode::Char('z') => Some(Action::ToggleZen),
        KeyCode::Char('v') => Some(Action::ToggleSpectrum),
        KeyCode::Char('s') => Some(Action::ToggleShuffle),
        KeyCode::Char('r') => Some(Action::CycleRepeat),
        KeyCode::Char('*') => Some(Action::ToggleLike),
        KeyCode::Char('f') => Some(Action::StartPersonalFm),
        KeyCode::Char('x') => Some(Action::TrashFmTrack),
        KeyCode::Char('m') => Some(Action::ToggleMute),
        KeyCode::Char('U') => Some(Action::StartSelfUpdate),
        KeyCode::Char('a') => Some(Action::AddSelectedToQueue),
        KeyCode::Char('/') => Some(Action::StartFilter),
        KeyCode::Tab => Some(Action::ToggleLibraryFocus),
        KeyCode::Char(' ') => Some(Action::TogglePlay),
        KeyCode::Char('n') => Some(Action::NextTrack),
        KeyCode::Char('p') => Some(Action::PrevTrack),
        KeyCode::Right => Some(Action::SeekBy(
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                30
            } else {
                5
            },
        )),
        KeyCode::Left => Some(Action::SeekBy(
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                -30
            } else {
                -5
            },
        )),
        KeyCode::Char('+') | KeyCode::Char('=') => Some(Action::VolumeBy(0.05)),
        KeyCode::Char('-') => Some(Action::VolumeBy(-0.05)),
        KeyCode::PageDown => Some(Action::MovePage(1)),
        KeyCode::PageUp => Some(Action::MovePage(-1)),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveSelection(1)),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveSelection(-1)),
        _ => None,
    }
}

/// The command palette owns the keyboard while open. Text-editing keys are
/// handled by its state module; this maps modal navigation and execution.
pub fn command_palette_key_action(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Action::CloseCommandPalette);
    }
    match key.code {
        KeyCode::Esc => Some(Action::CloseCommandPalette),
        KeyCode::Enter => Some(Action::ExecuteCommand),
        KeyCode::Down => Some(Action::MoveCommandSelection(1)),
        KeyCode::Up => Some(Action::MoveCommandSelection(-1)),
        _ => None,
    }
}

pub fn settings_key_action(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Action::Quit);
    }
    match key.code {
        KeyCode::Char(':') => Some(Action::OpenCommandPalette),
        KeyCode::Esc | KeyCode::Char('q') => Some(Action::CancelSettings),
        KeyCode::Char('1') => Some(Action::SwitchView(View::NowPlaying)),
        KeyCode::Char('2') => Some(Action::SwitchView(View::Library)),
        KeyCode::Char('3') => Some(Action::SwitchView(View::Search)),
        KeyCode::Char('4') => Some(Action::SwitchView(View::Queue)),
        KeyCode::Char('5') | KeyCode::Char(',') => Some(Action::SwitchView(View::Settings)),
        KeyCode::Enter => Some(Action::SaveSettings),
        KeyCode::Char('v') => Some(Action::ToggleSpectrum),
        KeyCode::Left | KeyCode::Char('h') => Some(Action::AdjustSetting(-1)),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::AdjustSetting(1)),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveSelection(1)),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveSelection(-1)),
        KeyCode::Char('g') => Some(Action::GKey),
        KeyCode::Home => Some(Action::JumpTop),
        KeyCode::Char('G') | KeyCode::End => Some(Action::JumpBottom),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn keys_arrive_raw_and_map_through_key_action() {
        // action_for defers mapping so text inputs can own the keyboard.
        assert!(matches!(
            action_for(key(KeyCode::Down)),
            Some(Action::RawKey(_))
        ));
        let map = |code| key_action(KeyEvent::new(code, KeyModifiers::NONE));
        assert!(matches!(map(KeyCode::Down), Some(Action::MoveSelection(1))));
        assert!(matches!(
            map(KeyCode::Char('j')),
            Some(Action::MoveSelection(1))
        ));
        assert!(matches!(
            map(KeyCode::Char('2')),
            Some(Action::SwitchView(View::Library))
        ));
        assert!(matches!(map(KeyCode::Char('z')), Some(Action::ToggleZen)));
        assert!(matches!(
            map(KeyCode::Char(':')),
            Some(Action::OpenCommandPalette)
        ));
        assert!(matches!(
            map(KeyCode::Char('v')),
            Some(Action::ToggleSpectrum)
        ));
        assert!(matches!(
            map(KeyCode::Char('f')),
            Some(Action::StartPersonalFm)
        ));
        assert!(matches!(
            map(KeyCode::Char('x')),
            Some(Action::TrashFmTrack)
        ));
        assert!(map(KeyCode::Char('i')).is_none());
        assert!(matches!(
            map(KeyCode::Char(',')),
            Some(Action::SwitchView(View::Settings))
        ));
    }

    #[test]
    fn fullwidth_ime_punctuation_maps_like_its_ascii_twin() {
        let map = |code| key_action(KeyEvent::new(code, KeyModifiers::NONE));
        assert!(matches!(map(KeyCode::Char('？')), Some(Action::ToggleHelp)));
        assert!(matches!(
            map(KeyCode::Char('，')),
            Some(Action::SwitchView(View::Settings))
        ));
        assert!(matches!(
            map(KeyCode::Char('：')),
            Some(Action::OpenCommandPalette)
        ));
    }

    #[test]
    fn repaint_arrives_from_ctrl_l_and_regained_focus() {
        assert!(matches!(
            key_action(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Some(Action::ForceRedraw)
        ));
        assert!(matches!(
            action_for(Event::FocusGained),
            Some(Action::ForceRedraw)
        ));
        assert!(action_for(Event::FocusLost).is_none());
    }

    #[test]
    fn command_palette_keys_are_modal() {
        let map = |code| command_palette_key_action(KeyEvent::new(code, KeyModifiers::NONE));

        assert!(matches!(
            map(KeyCode::Esc),
            Some(Action::CloseCommandPalette)
        ));
        assert!(matches!(map(KeyCode::Enter), Some(Action::ExecuteCommand)));
        assert!(matches!(
            map(KeyCode::Down),
            Some(Action::MoveCommandSelection(1))
        ));
        assert!(map(KeyCode::Char('n')).is_none());
    }

    #[test]
    fn playback_and_list_keys_map_to_their_new_independent_actions() {
        let map = |code, modifiers| key_action(KeyEvent::new(code, modifiers));

        assert!(matches!(
            map(KeyCode::Right, KeyModifiers::NONE),
            Some(Action::SeekBy(5))
        ));
        assert!(matches!(
            map(KeyCode::Left, KeyModifiers::SHIFT),
            Some(Action::SeekBy(-30))
        ));
        assert!(matches!(
            map(KeyCode::Char('m'), KeyModifiers::NONE),
            Some(Action::ToggleMute)
        ));
        assert!(matches!(
            map(KeyCode::Char('s'), KeyModifiers::NONE),
            Some(Action::ToggleShuffle)
        ));
        assert!(matches!(
            map(KeyCode::Char('r'), KeyModifiers::NONE),
            Some(Action::CycleRepeat)
        ));
        assert!(matches!(
            map(KeyCode::Char('*'), KeyModifiers::SHIFT),
            Some(Action::ToggleLike)
        ));
        assert!(matches!(
            map(KeyCode::Char('/'), KeyModifiers::NONE),
            Some(Action::StartFilter)
        ));
        assert!(matches!(
            map(KeyCode::PageDown, KeyModifiers::NONE),
            Some(Action::MovePage(1))
        ));
        assert!(matches!(
            map(KeyCode::Home, KeyModifiers::NONE),
            Some(Action::JumpTop)
        ));
        assert!(matches!(
            map(KeyCode::Tab, KeyModifiers::NONE),
            Some(Action::ToggleLibraryFocus)
        ));
    }

    #[test]
    fn settings_mouse_targets_use_the_drawn_geometry() {
        let mut hits = Hits::default();
        hits.settings_rows
            .push((ratatui::layout::Rect::new(2, 2, 20, 1), 3));
        hits.settings_adjust
            .push((ratatui::layout::Rect::new(20, 2, 2, 1), 1));
        hits.settings_save
            .push(ratatui::layout::Rect::new(2, 5, 8, 1));
        hits.settings_cancel
            .push(ratatui::layout::Rect::new(12, 5, 8, 1));
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        assert!(matches!(
            mouse_action(click(3, 2), &hits, 0),
            Some(Action::SelectSetting(3))
        ));
        assert!(matches!(
            mouse_action(click(20, 2), &hits, 0),
            Some(Action::AdjustSetting(1))
        ));
        assert!(matches!(
            mouse_action(click(3, 5), &hits, 0),
            Some(Action::SaveSettings)
        ));
        assert!(matches!(
            mouse_action(click(13, 5), &hits, 0),
            Some(Action::CancelSettings)
        ));
    }

    #[test]
    fn search_channel_chips_use_their_drawn_mouse_targets() {
        let mut hits = Hits::default();
        hits.search_channels.push((
            ratatui::layout::Rect::new(4, 3, 8, 1),
            crate::api::SearchChannel::Albums,
        ));
        let click = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 6,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };

        assert!(matches!(
            mouse_action(click, &hits, 0),
            Some(Action::SelectSearchChannel(
                crate::api::SearchChannel::Albums
            ))
        ));
    }
}
