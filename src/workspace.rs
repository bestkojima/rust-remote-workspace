//! Top-level navigation. In a shell, editor or transfer, keys belong to that mode.
pub(crate) mod settings;
use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    backend::Backend,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::{App, ListSelectionState, config::manager::ConfigManager, key_event::KeyFlow};

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        Terminal,
        backend::{CrosstermBackend, TestBackend},
    };

    #[tokio::test]
    async fn workspace_keys_do_not_leak_into_devices_or_dialogs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let terminal = Terminal::with_options(
            CrosstermBackend::new(Vec::<u8>::new()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap();
        let mut app = App::with_config_path(terminal, Some(&path)).unwrap();
        for (i, expected) in PAGES.iter().enumerate() {
            crate::key_event::handle_key_event(&mut app, KeyCode::F(i as u8 + 1).into()).await;
            assert_eq!(app.workspace_page, *expected);
        }
        crate::key_event::handle_key_event(&mut app, KeyCode::F(3).into()).await;
        crate::key_event::handle_key_event(&mut app, KeyCode::Char('d').into()).await;
        assert!(matches!(app.mode, crate::AppMode::ConnectionList(_)));
        crate::key_event::handle_key_event(&mut app, KeyCode::F(4).into()).await;
        crate::key_event::handle_key_event(&mut app, KeyCode::Char('n').into()).await;
        assert!(matches!(app.mode, crate::AppMode::FormNew { .. }));
        crate::key_event::handle_key_event(&mut app, KeyCode::F(3).into()).await;
        assert!(matches!(app.mode, crate::AppMode::FormNew { .. }));
        assert!(!path.exists());
    }

    #[test]
    fn narrow_files_page_shows_next_action() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigManager::with_path(dir.path().join("devices.toml")).unwrap();
        let mut term = Terminal::new(TestBackend::new(49, 12)).unwrap();
        term.draw(|f| {
            draw(
                f,
                f.area(),
                Page::Files,
                &ListSelectionState::new(0),
                &config,
                0,
                &Default::default(),
                &Default::default(),
                None,
                None,
            )
        })
        .unwrap();
        let buf = term.backend().buffer();
        let view: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(view.contains("Files"));
        assert!(view.contains("F4 Devices"));
    }

    #[tokio::test]
    async fn empty_file_search_does_not_connect_to_the_first_device() {
        use crate::config::manager::{AuthMethod, Connection};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let terminal = Terminal::with_options(
            CrosstermBackend::new(Vec::<u8>::new()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap();
        let mut app = App::with_config_path(terminal, Some(&path)).unwrap();
        app.config
            .add_connection(Connection::new(
                "localhost".into(),
                22,
                "test".into(),
                AuthMethod::AutoLoadKey,
            ))
            .unwrap();
        app.workspace_page = Page::Files;
        if let crate::AppMode::ConnectionList(s) = &mut app.mode {
            s.search = crate::SearchState::Applied {
                query: "no such device".into(),
            };
        }
        assert!(app.selected_connection().is_none());
        handle_key(&mut app, KeyCode::Enter.into()).await;
        assert!(matches!(app.mode, crate::AppMode::ConnectionList(_)));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Terminal,
    Files,
    Sync,
    Devices,
    Settings,
    Tunnels,
}

const PAGES: [Page; 5] = [
    Page::Terminal,
    Page::Files,
    Page::Sync,
    Page::Devices,
    Page::Settings,
];
const LABELS: [&str; 5] = ["Terminal", "Files", "Sync", "Devices", "Settings"];

pub(crate) async fn handle_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    if app.workspace_page == Page::Settings && app.settings_editor.is_some() {
        settings::key(app, key);
        return KeyFlow::Continue;
    }
    if app.workspace_page == Page::Sync
        && (app.sync.editing() || !matches!(key.code, KeyCode::F(1..=5)))
    {
        return if app.sync_key(key) {
            KeyFlow::Quit
        } else {
            KeyFlow::Continue
        };
    }
    if app.workspace_page == Page::Tunnels
        && (app.tunnels.modal_open() || !matches!(key.code, KeyCode::F(1..=5)))
    {
        app.tunnel_key(key);
        return KeyFlow::Continue;
    }
    if let KeyCode::F(number @ 1..=5) = key.code {
        let page = PAGES[usize::from(number - 1)];
        app.workspace_page = page;
        if page == Page::Terminal {
            app.resume_terminal();
        }
        app.mark_redraw();
        return KeyFlow::Continue;
    }

    match app.workspace_page {
        Page::Devices => return crate::key_event::handle_connection_list_key(app, key).await,
        Page::Files => {
            let editing =
                matches!(&app.mode, crate::AppMode::ConnectionList(s) if s.search.is_on());
            let code = match key.code {
                KeyCode::Enter if !editing => KeyCode::Char('i'),
                _ if editing => key.code,
                c @ (KeyCode::Up
                | KeyCode::Down
                | KeyCode::Esc
                | KeyCode::Backspace
                | KeyCode::Char('j' | 'k' | '/' | 'q')) => c,
                _ => return KeyFlow::Continue,
            };
            return crate::key_event::handle_connection_list_key(app, KeyEvent { code, ..key })
                .await;
        }
        Page::Terminal if key.code == KeyCode::Enter => app.workspace_page = Page::Devices,
        Page::Settings if matches!(key.code, KeyCode::Char('e' | 'E')) => {
            app.settings_editor = Some(settings::Editor::from_config(&app.config))
        }
        Page::Settings if matches!(key.code, KeyCode::Char('p' | 'P')) => {
            app.go_to_port_forwarding_list().await
        }
        Page::Settings if matches!(key.code, KeyCode::Char('t' | 'T')) => {
            app.workspace_page = Page::Tunnels
        }
        _ if key.code == KeyCode::Char('q') => return KeyFlow::Quit,
        _ => {}
    }
    app.mark_redraw();
    KeyFlow::Continue
}

pub(crate) fn draw(
    f: &mut Frame,
    area: Rect,
    page: Page,
    state: &ListSelectionState,
    config: &ConfigManager,
    open: usize,
    tunnels: &crate::app::tunnels::Desk,
    sync: &crate::app::sync::SyncDesk,
    settings_editor: Option<&settings::Editor>,
    notice: Option<&str>,
) {
    let [status, nav, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let accent = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let devices = config.connections();
    let selected = if matches!(page, Page::Devices | Page::Files) && notice.is_none() {
        crate::ui::connection::resolve_visible_selection(
            devices,
            None,
            state.search.query(),
            state.selected,
        )
        .map(|index| format!("   ·   Selected: {}", devices[index].display_name))
        .unwrap_or_default()
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Rust Remote ", accent),
            Span::raw(format!(
                "  SSH {open} open   ·   Tunnel {}",
                tunnels.summary()
            )),
            Span::styled(
                notice.map(|n| format!("   ·   {n}")).unwrap_or_default(),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(selected),
        ])),
        status,
    );
    let terse = nav.width < 55;
    let mut tabs = Vec::new();
    for (i, (&item, label)) in PAGES.iter().zip(LABELS).enumerate() {
        let key = i + 1;
        let text = if terse && page != item {
            format!(" F{key} ")
        } else {
            format!(" F{key} {label} ")
        };
        tabs.push(Span::styled(
            text,
            if page == item || (page == Page::Tunnels && item == Page::Settings) {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ));
    }
    f.render_widget(Paragraph::new(Line::from(tabs)), nav);
    if page == Page::Settings {
        settings::draw(f, body, config, settings_editor);
        return;
    }
    if page == Page::Tunnels {
        tunnels.draw(f, body, config);
        return;
    }
    if page == Page::Sync {
        sync.draw(f, body, config);
        return;
    }

    if matches!(page, Page::Devices | Page::Files) && !devices.is_empty() {
        crate::ui::draw_connection_list(
            body,
            devices,
            state.selected,
            &state.search,
            f,
            page == Page::Files,
        );
        return;
    }

    let (title, detail, action) = match page {
        Page::Devices => ("Your devices", "No saved SSH devices.", "N  Add device"),
        Page::Files => (
            "Browse files",
            "A saved SSH device opens beside your local directory.",
            "F4 Devices  →  N Add device",
        ),
        Page::Terminal => (
            "No open terminals",
            "Connect to a device to start an interactive shell.",
            "Enter  Choose device",
        ),
        Page::Sync => unreachable!(),
        Page::Settings => unreachable!(),
        Page::Tunnels => unreachable!(),
    };
    let mut lines = vec![Line::styled(title, accent), Line::raw("")];
    lines.push(Line::raw(detail));
    let [content, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(body);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content.inner(Margin::new(2, 1)),
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {action}"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("    q  Quit"),
        ])),
        footer,
    );
}
