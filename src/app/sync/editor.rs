use super::*;
use crate::config::manager::Connection;
use ratatui::style::Style;
use tui_textarea::{CursorMove, Input, TextArea};

pub(super) struct Field(pub TextArea<'static>);
impl Field {
    pub fn new(s: &str) -> Self {
        let mut field = TextArea::new(vec![s.to_owned()]);
        field.set_cursor_line_style(Style::default());
        field.move_cursor(CursorMove::End);
        Self(field)
    }
    pub fn value(&self) -> &str {
        &self.0.lines()[0]
    }
    pub fn key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('a') {
            self.0.select_all();
        } else if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('u') {
            self.0.select_all();
            self.0.cut();
        } else if !k.modifiers.contains(KeyModifiers::CONTROL)
            || !matches!(k.code, KeyCode::Char('j' | 'm'))
        {
            self.0.input(Input::from(k));
        }
    }
    pub fn paste(&mut self, s: &str) {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        self.0.insert_str(clean);
    }
}
pub(super) struct Editor {
    pub profile: SyncProfile,
    pub focus: usize,
    pub name: Field,
    pub local: Field,
    pub remote: Field,
    pub excludes: Field,
}
impl Editor {
    pub fn new(profile: SyncProfile) -> Self {
        Self {
            name: Field::new(&profile.name),
            local: Field::new(&profile.spec.local_root.to_string_lossy()),
            remote: Field::new(&profile.spec.remote_root),
            excludes: Field::new(&profile.spec.excludes.join("; ")),
            profile,
            focus: 0,
        }
    }
    fn field_mut(&mut self) -> Option<&mut Field> {
        match self.focus {
            0 => Some(&mut self.name),
            2 => Some(&mut self.local),
            3 => Some(&mut self.remote),
            5 => Some(&mut self.excludes),
            _ => None,
        }
    }
    pub fn paste(&mut self, s: &str) {
        if let Some(f) = self.field_mut() {
            f.paste(s);
        }
    }
    pub fn key(&mut self, k: KeyEvent, devices: &[Connection]) {
        match k.code {
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % 6,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + 5) % 6,
            KeyCode::Left | KeyCode::Right if self.focus == 1 && !devices.is_empty() => {
                let n = devices.len();
                let i = devices
                    .iter()
                    .position(|p| p.id == self.profile.device_id)
                    .unwrap_or(0);
                self.profile.device_id = devices
                    [(i + if k.code == KeyCode::Right { 1 } else { n - 1 }) % n]
                    .id
                    .clone();
            }
            KeyCode::Left | KeyCode::Right if self.focus == 4 => {
                self.profile.spec.direction = match self.profile.spec.direction {
                    Direction::Upload => Direction::Download,
                    Direction::Download => Direction::Upload,
                }
            }
            _ => {
                if let Some(f) = self.field_mut() {
                    f.key(k);
                }
            }
        }
    }
    pub fn value(&self) -> SyncProfile {
        let mut p = self.profile.clone();
        p.name = self.name.value().trim().to_string();
        p.spec.local_root = self.local.value().into();
        p.spec.remote_root = self.remote.value().to_string();
        p.spec.excludes = self
            .excludes
            .value()
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        p
    }
}
