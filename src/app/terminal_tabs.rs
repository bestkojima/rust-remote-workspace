use super::*;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use remote_core::{ParkedTabs, SessionId};
use tokio_util::sync::CancellationToken;

/// An ordinary SSH terminal behind another tab or the workspace.
/// Explorer shells stay in AppMode: they own a return path to an SFTP channel.
pub(super) struct ParkedTerminal {
    name: String,
    client: SshSession,
    state: Arc<Mutex<TerminalState>>,
    current_selected: usize,
    cancel_token: CancellationToken,
}

impl<B: Backend + Write> App<B> {
    pub(crate) fn active_terminal_id(&self) -> Option<SessionId> {
        match &self.mode {
            AppMode::Connected { client, .. } => Some(client.id()),
            _ => None,
        }
    }

    pub fn terminal_count(&self) -> usize {
        self.parked.iter().len()
            + usize::from(matches!(
                &self.mode,
                AppMode::Connected {
                    return_to_explorer: None,
                    ..
                }
            ))
    }

    pub fn last_terminal_notice(&self) -> Option<&str> {
        self.terminal_notice.as_deref()
    }

    pub(crate) async fn remote_disconnect(&mut self, id: SessionId) {
        let name = match &self.mode {
            AppMode::Connected { client, name, .. } if client.id() == id => Some(name.as_str()),
            _ => self
                .parked
                .iter()
                .find(|(key, _)| *key == id)
                .map(|(_, tab)| tab.name.as_str()),
        };
        if let Some(name) = name {
            let notice = format!(
                "#{id} {} disconnected",
                name.chars()
                    .filter(|c| !c.is_control())
                    .take(20)
                    .collect::<String>()
            );
            self.close_terminal(id).await;
            self.terminal_notice = Some(notice);
        }
    }

    pub fn park_terminal(&mut self) {
        let AppMode::Connected {
            current_selected,
            return_to_explorer: None,
            ..
        } = &self.mode
        else {
            return;
        };
        let selected = *current_selected;
        let saved = std::mem::replace(
            &mut self.mode,
            AppMode::ConnectionList(ListSelectionState::new(selected)),
        );
        if let AppMode::Connected {
            name,
            client,
            state,
            current_selected,
            cancel_token,
            ..
        } = saved
        {
            self.parked.park(
                client.id(),
                ParkedTerminal {
                    name,
                    client,
                    state,
                    current_selected,
                    cancel_token,
                },
            );
        }
        self.go_to_connection_list_with_selected(selected);
    }

    fn activate(&mut self, tab: ParkedTerminal) {
        self.go_to_connected(
            tab.name,
            tab.client,
            tab.state,
            tab.current_selected,
            tab.cancel_token,
        );
    }

    pub fn resume_terminal(&mut self) -> bool {
        if matches!(self.mode, AppMode::ConnectionList(_))
            && let Some(tab) = self.parked.take_recent()
        {
            self.activate(tab);
            true
        } else {
            false
        }
    }

    pub(crate) fn cycle_terminal(&mut self) {
        if let AppMode::Connected {
            client,
            return_to_explorer: None,
            ..
        } = &self.mode
            && let Some(next) = self.parked.take_next(client.id())
        {
            self.park_terminal();
            self.activate(next);
        }
    }

    pub(crate) async fn close_terminal(&mut self, id: SessionId) {
        if self.active_terminal_id() != Some(id) {
            // Duplicate or late events must never disturb the current mode.
            if let Some(tab) = self.parked.remove(id) {
                close(&tab.client, &tab.cancel_token).await;
                self.mark_redraw();
            }
            return;
        }
        let active = std::mem::replace(
            &mut self.mode,
            AppMode::ConnectionList(ListSelectionState::new(0)),
        );
        let AppMode::Connected {
            client,
            current_selected,
            cancel_token,
            return_to_explorer,
            ..
        } = active
        else {
            unreachable!()
        };
        close(&client, &cancel_token).await;
        self.clear_selection();
        if let Some(ScpReturnMode::FileExplorer {
            connection_name,
            left_pane,
            left_explorer,
            left_session,
            remote_explorer,
            ssh_connection,
            channel,
            ssh_session,
            active_pane,
            copy_buffer,
            return_to,
            search,
        }) = return_to_explorer
        {
            self.mode = AppMode::FileExplorer {
                connection_name,
                left_pane,
                left_explorer,
                left_session,
                remote_explorer,
                ssh_connection,
                channel,
                ssh_session,
                active_pane,
                copy_buffer,
                return_to,
                search,
                source_selector: Default::default(),
                delete_confirmation: Default::default(),
            };
        } else if let Some(next) = self.parked.take_next(id) {
            self.activate(next);
        } else {
            self.go_to_connection_list_with_selected(current_selected);
        }
        self.stop_ticker();
        self.mark_redraw();
    }

    pub(super) fn cancel_terminal_readers(&self) {
        if let AppMode::Connected { cancel_token, .. } = &self.mode {
            cancel_token.cancel();
        }
        for (_, tab) in self.parked.iter() {
            tab.cancel_token.cancel();
        }
    }
}

async fn close(client: &SshSession, cancel: &CancellationToken) {
    cancel.cancel();
    if let Err(e) = client.close_channel().await {
        tracing::debug!("Closing terminal channel: {e}");
    }
    if let Err(e) = client.close().await {
        tracing::debug!("Closing SSH session: {e}");
    }
}

pub(super) fn tab_header(
    parked: &ParkedTabs<ParkedTerminal>,
    id: SessionId,
    name: &str,
    width: u16,
    notice: Option<&str>,
) -> Line<'static> {
    let mut tabs: Vec<_> = parked
        .iter()
        .map(|(id, tab)| (id, tab.name.as_str()))
        .collect();
    tabs.push((id, name));
    tabs.sort_unstable_by_key(|&(id, _)| id);

    let hints = if width >= 66 {
        " F6 next  F8 close  F12 devices "
    } else {
        " F6 >  F8 ×  F12 + "
    };
    let budget = usize::from(width).saturating_sub(Span::raw(hints).width());
    let mut labels: Vec<_> = tabs
        .into_iter()
        .map(|(key, name)| {
            let short: String = name.chars().filter(|c| !c.is_control()).take(20).collect();
            let style = if key == id {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            (key, Span::styled(format!(" #{key} {short} "), style))
        })
        .collect();
    let total: usize = labels.iter().map(|(_, span)| span.width()).sum();
    // Keep the strip fixed while switching, unless active would disappear in a clipped row.
    if total > budget || notice.is_some() {
        let index = labels.iter().position(|(key, _)| *key == id).unwrap();
        labels.rotate_left(index);
    }
    let mut spans = Vec::new();
    let mut remaining = budget;
    for (_, label) in labels {
        if label.width() > remaining && !spans.is_empty() {
            break;
        }
        remaining = remaining.saturating_sub(label.width());
        spans.push(label);
        if let Some(msg) = notice {
            spans.push(Span::styled(
                format!(" {msg} "),
                Style::default().fg(Color::Yellow),
            ));
            break;
        }
    }
    spans.push(Span::styled(hints, Style::default().fg(Color::Gray)));
    Line::from(spans)
}
