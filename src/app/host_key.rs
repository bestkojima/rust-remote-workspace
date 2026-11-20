use super::{AfterTrust, App, AppMode, ConnectingSource, Connection, SshSession};
use crate::{AppError, ListSelectionState};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    backend::Backend,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph},
};
use std::io::Write;

impl<B: Backend + Write> App<B> {
    pub(crate) fn begin_connection(
        &mut self,
        mut connection: Connection,
        selected: usize,
        source: ConnectingSource,
    ) {
        if let Err(e) = self.tunnels.prepare_device(&self.config, &mut connection) {
            self.restore_connection_source(source, selected);
            self.set_error(e);
            return;
        }
        let (cols, rows) = self.ssh_terminal_size().unwrap_or((80, 24));
        let (cancel, result) = SshSession::initiate_connection(connection.clone(), cols, rows);
        let name = connection.display_name.clone();
        self.go_to_connecting(connection, name, selected, source, cancel, result);
    }

    pub(crate) fn restore_connection_source(&mut self, source: ConnectingSource, selected: usize) {
        self.stop_ticker();
        match source {
            ConnectingSource::FormNew { auto_auth, form } => {
                self.mode = AppMode::FormNew {
                    auto_auth,
                    form,
                    current_selected: selected,
                };
            }
            ConnectingSource::FormEdit { form, original } => {
                self.mode = AppMode::FormEdit {
                    form,
                    original,
                    current_selected: selected,
                };
            }
            ConnectingSource::ConnectionList { file_explorer } => {
                self.go_to_connection_list_with_selected(selected);
                if file_explorer {
                    self.workspace_page = crate::workspace::Page::Files;
                }
            }
        }
        self.mark_redraw();
    }

    pub(crate) fn offer_host_key_or_error(&mut self, connection: Connection, error: AppError) {
        self.offer_host_key(connection, error, AfterTrust::Return);
    }

    pub(super) fn offer_host_key(
        &mut self,
        connection: Connection,
        error: AppError,
        next: AfterTrust,
    ) {
        let AppError::UnknownHostKey {
            public_key,
            fingerprint,
        } = error
        else {
            self.set_error(error);
            return;
        };
        let return_to = self.current_selected();
        match self.config.host_key_is_trusted(&connection, &public_key) {
            Ok(false) => {}
            Ok(true) => {
                self.resume_trusted_connection(connection, public_key, return_to, next, false);
                return;
            }
            Err(e) => {
                self.set_error(e);
                return;
            }
        }
        let previous = std::mem::replace(
            &mut self.mode,
            AppMode::ConnectionList(ListSelectionState::new(0)),
        );
        self.mode = AppMode::VerifyHostKey {
            connection,
            previous: Box::new(previous),
            next,
            public_key,
            fingerprint,
            return_to,
            visible: false,
        };
        self.stop_ticker();
        self.mark_redraw();
    }

    pub(crate) fn handle_host_key(&mut self, key: KeyEvent) {
        let accept = match key.code {
            KeyCode::Char('y')
                if key.modifiers.is_empty()
                    && matches!(self.mode, AppMode::VerifyHostKey { visible: true, .. }) =>
            {
                true
            }
            KeyCode::Char('n') | KeyCode::Esc => false,
            _ => return,
        };
        let AppMode::VerifyHostKey {
            connection,
            public_key,
            return_to,
            previous,
            next,
            ..
        } = std::mem::replace(
            &mut self.mode,
            AppMode::ConnectionList(ListSelectionState::new(0)),
        )
        else {
            unreachable!()
        };
        self.mode = *previous;
        self.mark_redraw();
        if !accept {
            return;
        }
        match self.config.host_key_is_trusted(&connection, &public_key) {
            Ok(true) => {
                self.resume_trusted_connection(connection, public_key, return_to, next, false)
            }
            Ok(false) => match self.config.trust_host_key(&connection, &public_key) {
                Ok(()) => {
                    self.resume_trusted_connection(connection, public_key, return_to, next, true)
                }
                Err(e) => self.set_error(e),
            },
            Err(e) => self.set_error(e),
        }
    }

    fn resume_trusted_connection(
        &mut self,
        mut connection: Connection,
        public_key: String,
        return_to: usize,
        next: AfterTrust,
        saved_here: bool,
    ) {
        if matches!(next, AfterTrust::Return) {
            self.set_info(
                if saved_here {
                    "Host key saved. Repeat the action to connect."
                } else {
                    "Host key already trusted. Repeat the action to connect."
                }
                .into(),
            );
            return;
        }
        connection.public_key = Some(public_key);
        if !saved_here
            && let AppMode::FormNew {
                form, auto_auth, ..
            } = &self.mode
        {
            // The other process saved an endpoint pin, not this new profile.
            // Preserve the ordinary form's save-on-success behavior.
            let source = ConnectingSource::FormNew {
                form: form.clone(),
                auto_auth: *auto_auth,
            };
            self.begin_connection(connection, return_to, source);
            return;
        }
        let selected = self
            .config
            .connections()
            .iter()
            .position(|c| c.id == connection.id)
            .unwrap_or(return_to);
        // Confirmation already saved a new device. A later authentication failure
        // should return to that device rather than offer to create it a second time.
        let files = matches!(next, AfterTrust::Files);
        self.begin_connection(
            connection,
            selected,
            ConnectingSource::ConnectionList {
                file_explorer: files,
            },
        );
    }
}

pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    conn: &Connection,
    key: &str,
    fingerprint: &str,
) -> bool {
    let host = conn.host_port();
    let alg = key.split_ascii_whitespace().next().unwrap_or("");
    panel(
        f,
        area,
        "Verify SSH host key",
        Color::Yellow,
        &[
            (&host, Style::new().bold()),
            (alg, Style::default()),
            ("", Style::default()),
            (fingerprint, Style::new().cyan().bold()),
            ("", Style::default()),
            (
                "Compare with the fingerprint on the device",
                Style::default(),
            ),
            ("or one supplied by its administrator.", Style::default()),
            ("", Style::default()),
            ("y  Trust and save     n / Esc  Cancel", Style::new().bold()),
        ],
    )
}

pub(super) fn draw_changed(f: &mut Frame, area: Rect, endpoint: &str, old: &str, new: &str) {
    panel(
        f,
        area,
        "Host key changed",
        Color::Red,
        &[
            (endpoint, Style::new().bold()),
            ("Connection blocked. Saved key kept.", Style::default()),
            ("", Style::default()),
            ("Saved", Style::default()),
            (old, Style::new().bold()),
            ("Received", Style::default()),
            (new, Style::new().yellow().bold()),
            ("", Style::default()),
            ("Check with the device administrator.", Style::default()),
            ("Enter / Esc  Back", Style::new().bold()),
        ],
    );
}

/// Hard-wrap long identities, including unbroken fingerprints. Never accept a
/// key while the viewport is too short to show what the user would be trusting.
fn panel(f: &mut Frame, area: Rect, title: &str, accent: Color, rows: &[(&str, Style)]) -> bool {
    let w = area.width.min(66);
    let available = usize::from(w.saturating_sub(4));
    let mut lines = Vec::new();
    for &(s, style) in rows {
        let mut row = String::new();
        let mut used = 0;
        for ch in s.chars() {
            let width = Span::raw(ch.to_string()).width();
            if used + width > available && !row.is_empty() {
                lines.push(Line::styled(std::mem::take(&mut row), style));
                used = 0;
            }
            row.push(ch);
            used += width;
        }
        lines.push(Line::styled(row, style));
    }
    let h = (lines.len() + 2) as u16;
    let fits = w >= 28 && area.height >= h;
    let height = if fits { h } else { area.height.min(4) };
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - height) / 2,
        w,
        height,
    );
    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::new().fg(accent))
        .padding(Padding::horizontal(1));
    f.render_widget(Clear, rect);
    let body = if fits {
        Paragraph::new(lines)
    } else {
        Paragraph::new("Enlarge window to view key.\nEsc  Back")
    };
    f.render_widget(body.block(block), rect);
    fits
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn fingerprint_wraps_without_truncation_or_blind_approval() {
        let fp = "SHA256:1234567890123456789012345678901234567890123";
        for (height, should_fit) in [(15, true), (3, false)] {
            let mut term = Terminal::new(TestBackend::new(38, height)).unwrap();
            term.draw(|f| {
                assert_eq!(
                    panel(
                        f,
                        f.area(),
                        "Verify SSH host key",
                        Color::Yellow,
                        &[
                            (fp, Style::default()),
                            ("y  Trust and save", Style::default())
                        ]
                    ),
                    should_fit
                );
            })
            .unwrap();
            let buf = term.backend().buffer();
            let cells: String = buf.content.iter().map(|c| c.symbol()).collect();
            assert_eq!(cells.contains("y  Trust and save"), should_fit);
            if should_fit {
                let digest: String = cells.chars().filter(char::is_ascii_digit).collect();
                assert_eq!(
                    digest,
                    fp.chars().filter(char::is_ascii_digit).collect::<String>()
                );
            }
        }
    }
}
