use crate::{
    App,
    config::manager::{ConfigManager, LogPreference},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    backend::Backend,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::io::Write;
use tui_textarea::{Input, TextArea};

pub(crate) struct Editor {
    fields: [TextArea<'static>; 3],
    log: LogPreference,
    focus: usize,
    error: Option<String>,
}

impl Editor {
    pub(super) fn from_config(config: &ConfigManager) -> Self {
        let fields = [
            config.default_port().to_string(),
            config.terminal_scrollback_lines().to_string(),
            config.rclone_path().to_string_lossy().into_owned(),
        ]
        .map(|value| {
            let mut field = TextArea::new(vec![value]);
            field.set_cursor_line_style(Style::new());
            field.set_cursor_style(Style::new().reversed());
            field
        });
        Self {
            fields,
            log: config.log_preference(),
            focus: 0,
            error: None,
        }
    }
    pub(crate) fn paste(&mut self, data: &str) {
        if let Some(field) = self.fields.get_mut(self.focus) {
            field.insert_str(data.split(['\r', '\n']).next().unwrap_or_default());
        }
    }
}

pub(super) fn key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) {
    app.mark_redraw();
    let Some(ed) = app.settings_editor.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Esc => app.settings_editor = None,
        KeyCode::Tab | KeyCode::Down => ed.focus = (ed.focus + 1) % 4,
        KeyCode::BackTab | KeyCode::Up => ed.focus = (ed.focus + 3) % 4,
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if ed.focus == 3 => {
            ed.log = ed.log.next(key.code == KeyCode::Left);
        }
        KeyCode::Enter => {
            let parse = |i: usize| ed.fields[i].lines()[0].trim();
            let Ok(port) = parse(0).parse::<u16>() else {
                ed.focus = 0;
                ed.error = Some("SSH port must be between 1 and 65535.".into());
                return;
            };
            let Ok(history) = parse(1).parse::<usize>() else {
                ed.focus = 1;
                ed.error = Some("Terminal history must be between 1 and 5000 lines.".into());
                return;
            };
            match app.config.save_defaults(port, history, parse(2), ed.log) {
                Ok(()) => app.settings_editor = None,
                Err(e) => {
                    if port == 0 {
                        ed.focus = 0;
                    } else if !(1..=5000).contains(&history) {
                        ed.focus = 1;
                    }
                    ed.error = Some(e.to_string());
                }
            }
        }
        _ => {
            if let Some(field) = ed.fields.get_mut(ed.focus) {
                if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    field.select_all();
                } else if !matches!(key.code, KeyCode::F(_)) {
                    field.input(Input::from(key));
                }
            }
        }
    }
}

pub(super) fn draw(f: &mut Frame, area: Rect, cfg: &ConfigManager, editor: Option<&Editor>) {
    let area = area.inner(Margin::new(2, 1));
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);
    let header = if editor.is_some() {
        "Edit workspace defaults"
    } else {
        "Workspace settings"
    };
    f.render_widget(Paragraph::new(header.cyan().bold()), title);
    if let Some(ed) = editor {
        let labels = [
            "New device SSH port",
            "Terminal history",
            "rclone executable",
            "Log on next launch",
        ];
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(body);
        for (i, label) in labels.iter().enumerate() {
            let [caption, value] = Layout::horizontal([
                Constraint::Length(if area.width < 50 { 21 } else { 24 }),
                Constraint::Min(1),
            ])
            .areas(rows[i]);
            f.render_widget(
                Paragraph::new(if i == ed.focus {
                    (*label).cyan().bold()
                } else {
                    (*label).gray()
                }),
                caption,
            );
            if i == 3 {
                f.render_widget(Paragraph::new(ed.log.label()), value);
            } else if i == ed.focus {
                f.render_widget(&ed.fields[i], value);
            } else {
                f.render_widget(Paragraph::new(ed.fields[i].lines()[0].as_str()), value);
            }
        }
        if let Some(e) = ed.error.as_ref() {
            f.render_widget(
                Paragraph::new(e.as_str())
                    .style(Style::new().fg(Color::Yellow))
                    .wrap(Wrap { trim: false }),
                rows[4],
            );
        } else {
            let hint = match ed.focus {
                0 => "1–65535 · Used when a new device leaves its port blank.",
                1 => "1–5000 lines · Applies to newly opened terminals.",
                2 => "Path or command name. Leave blank to use rclone on PATH.",
                _ => "Left / Right or Space: off, info, debug. Takes effect at next launch.",
            };
            f.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }), rows[4]);
        }
        f.render_widget(
            Paragraph::new("Tab  Next    Ctrl+A  Select all    Enter  Save    Esc  Cancel")
                .wrap(Wrap { trim: false }),
            footer,
        );
    } else {
        let lines = vec![
            Line::raw(format!("New device SSH port   {}", cfg.default_port())),
            Line::raw(format!(
                "Terminal history      {} lines",
                cfg.terminal_scrollback_lines()
            )),
            Line::raw(format!(
                "rclone executable     {}",
                cfg.rclone_path().display()
            )),
            Line::raw(format!(
                "Log on next launch    {}",
                cfg.log_preference().label()
            )),
            Line::raw(""),
            Line::raw(format!(
                "VPS tunnels           {} saved",
                cfg.tunnels().len()
            )),
        ];
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
        f.render_widget(
            Paragraph::new("E  Edit defaults    T  VPS tunnels    P  SSH port forwards    q  Quit")
                .wrap(Wrap { trim: false }),
            footer,
        );
    }
}
