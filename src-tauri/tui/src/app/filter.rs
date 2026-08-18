use crate::api::SongRow;

#[derive(Default)]
pub struct ListFilter {
    pub query: String,
    pub input: bool,
}

impl ListFilter {
    pub fn start(&mut self) {
        self.input = true;
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.input = false;
    }

    pub fn push(&mut self, character: char) {
        self.query.push(character);
    }

    pub fn pop(&mut self) {
        self.query.pop();
    }

    pub fn paste(&mut self, text: &str) {
        self.query.push_str(&text.replace(['\n', '\r'], " "));
    }

    pub fn is_active(&self) -> bool {
        self.input || !self.query.is_empty()
    }

    pub fn matches(&self, row: &SongRow) -> bool {
        let mut needle = self
            .query
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|character| !character.is_whitespace());
        let mut next = needle.next();
        if next.is_none() {
            return true;
        }
        for character in row
            .title
            .chars()
            .chain(row.artist.chars())
            .flat_map(char::to_lowercase)
            .filter(|character| !character.is_whitespace())
        {
            if next == Some(character) {
                next = needle.next();
                if next.is_none() {
                    return true;
                }
            }
        }
        false
    }
}

impl super::AppState {
    pub(super) fn handle_filter_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.confirm_quit = true;
            }
            return;
        }
        match key.code {
            KeyCode::Char(character) => {
                self.filter.push(character);
                self.selected = 0;
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Enter => self.filter.input = false,
            KeyCode::Esc => self.clear_filter(),
            KeyCode::Down | KeyCode::Tab => {
                self.filter.input = false;
                self.selected = 0;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ListFilter;
    use crate::api::SongRow;

    fn row() -> SongRow {
        SongRow {
            id: 1,
            title: "Night Cruising".into(),
            artist: "Fishmans".into(),
            album: "Long Season".into(),
            duration_ms: 180_000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        }
    }

    #[test]
    fn fuzzy_filter_matches_in_order_across_title_and_artist() {
        let mut filter = ListFilter {
            query: "ngtfsh".into(),
            ..ListFilter::default()
        };
        assert!(filter.matches(&row()));

        filter.query = "fish night".into();
        assert!(!filter.matches(&row()));
    }

    #[test]
    fn clearing_a_filter_restores_its_idle_state() {
        let mut filter = ListFilter::default();
        filter.start();
        filter.push('夜');
        assert!(filter.is_active());

        filter.clear();

        assert!(filter.query.is_empty());
        assert!(!filter.input);
        assert!(!filter.is_active());
    }
}
