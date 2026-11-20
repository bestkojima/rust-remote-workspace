use super::{App, AppMode, ConnectingSource};
use crate::{
    AppError, AppEvent, Result,
    config::manager::{ConfigManager, Connection, TunnelRoute},
    tunnel_home::{HomeEvent, HomeTunnel},
    workspace::Page,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    backend::Backend,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListState, Paragraph, Wrap},
};
use std::{collections::HashMap, io::Write};

#[derive(Default)]
pub(crate) struct Desk {
    selected: usize,
    editor: Option<Editor>,
    delete: bool,
    generation: u64,
    runs: HashMap<String, Run>,
}

struct Run {
    generation: u64,
    worker: Option<HomeTunnel>,
    gateway: Connection,
    phase: Phase,
    message: String,
    pending: Option<AppError>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connecting,
    Ready,
    Retry,
    Blocked,
    Stopping,
}

struct Editor {
    original: Option<usize>,
    field: usize,
    gateway: usize,
    device: usize,
    port: String,
}

impl Desk {
    pub fn modal_open(&self) -> bool {
        self.editor.is_some() || self.delete
    }

    pub fn summary(&self) -> String {
        if self.runs.is_empty() {
            return "off".into();
        }
        let ready = self
            .runs
            .values()
            .filter(|r| r.phase == Phase::Ready)
            .count();
        if ready > 0 {
            format!("{ready} ready")
        } else if self.runs.values().any(|r| r.phase == Phase::Blocked) {
            "blocked".into()
        } else if self.runs.values().any(|r| r.phase == Phase::Retry) {
            "retrying".into()
        } else if self.runs.values().any(|r| r.phase == Phase::Connecting) {
            "connecting".into()
        } else {
            "stopping".into()
        }
    }

    fn can_open(&self, device: &str) -> bool {
        self.runs
            .get(device)
            .is_some_and(|r| r.phase == Phase::Ready)
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, cfg: &ConfigManager) {
        let area = area.inner(Margin::new(2, 1));
        let [head, body, foot] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .areas(area);
        let profile = |id: &str| {
            cfg.find_connection(id)
                .map(|c| c.display_name.as_str())
                .unwrap_or("(missing)")
        };
        let mut title = "VPS tunnels";
        let actions;
        if let Some(e) = &self.editor {
            title = if e.original.is_some() {
                "Edit VPS tunnel"
            } else {
                "New VPS tunnel"
            };
            let items = cfg.connections();
            let device = |i| {
                items
                    .get(i)
                    .map(|c: &Connection| {
                        format!("{}   {}@{}", c.display_name, c.username, c.host_port())
                    })
                    .unwrap_or_default()
            };
            let fields = [
                ("VPS SSH profile", device(e.gateway)),
                ("Device SSH profile", device(e.device)),
                ("VPS service port", e.port.clone()),
            ];
            let mut lines = vec![];
            for (i, (label, value)) in fields.into_iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<20}", label), Style::default().fg(Color::Gray)),
                    Span::styled(
                        value,
                        if i == e.field {
                            Style::new().black().on_cyan()
                        } else {
                            Style::default()
                        },
                    ),
                ]));
            }
            lines.push(Line::raw(""));
            lines.push(Line::raw(
                "The device profile supplies the home listener (127.0.0.1).",
            ));
            lines.push(Line::raw(
                "Use the rathole service port on the VPS, not its control port.",
            ));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
            actions = "Tab  Next field    ←/→  Select profile    Enter  Save    Esc  Cancel";
        } else if self.delete {
            let name = cfg
                .tunnels()
                .get(self.selected)
                .map(|r| profile(&r.device_id))
                .unwrap_or("");
            f.render_widget(
                Paragraph::new(format!(
                    "Remove the tunnel for {name}? Its SSH profiles will be kept."
                ))
                .wrap(Wrap { trim: false }),
                body,
            );
            actions = "y  Remove    Esc  Cancel";
        } else if cfg.tunnels().is_empty() {
            f.render_widget(Paragraph::new("No VPS tunnels yet. Add a VPS and a device in F4 Devices first.\nUse Ctrl+S in the device form to save without connecting.").wrap(Wrap{trim:false}),body);
            actions = "N  Add tunnel    Esc  Settings";
        } else {
            let [list, detail] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(4)]).areas(body);
            let rows = cfg
                .tunnels()
                .iter()
                .map(|r| {
                    let phase = self
                        .runs
                        .get(&r.device_id)
                        .map_or("Off", |run| match run.phase {
                            Phase::Ready => "Ready",
                            Phase::Blocked => "Blocked",
                            Phase::Connecting => "Connecting",
                            Phase::Stopping => "Stopping",
                            Phase::Retry => "Retrying",
                        });
                    Line::from(vec![
                        Span::styled(
                            format!("{phase:<12}"),
                            Style::default().fg(if phase == "Ready" {
                                Color::Green
                            } else {
                                Color::Yellow
                            }),
                        ),
                        Span::raw(format!(
                            "{}   via   {}",
                            profile(&r.device_id),
                            profile(&r.gateway_id)
                        )),
                    ])
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(self.selected));
            f.render_stateful_widget(
                List::new(rows)
                    .highlight_symbol("› ")
                    .highlight_style(Style::new().bold().reversed()),
                list,
                &mut state,
            );
            if let Some(r) = cfg.tunnels().get(self.selected) {
                let local = cfg
                    .find_connection(&r.device_id)
                    .map(|c| c.host_port())
                    .unwrap_or_default();
                let message = self
                    .runs
                    .get(&r.device_id)
                    .map_or("Stopped", |r| r.message.as_str());
                f.render_widget(
                    Paragraph::new(vec![
                        Line::raw(format!(
                            "Home {local} → {} → VPS 127.0.0.1:{}",
                            profile(&r.gateway_id),
                            r.service_port
                        )),
                        Line::styled(message, Style::new().cyan()),
                    ])
                    .wrap(Wrap { trim: false }),
                    detail,
                );
            }
            actions = "Enter  Start/retry    o  Open shell    x  Stop    n/e/d  Add/edit/remove    Esc  Back";
        }
        f.render_widget(Paragraph::new(title.bold().cyan()), head);
        f.render_widget(Paragraph::new(actions).wrap(Wrap { trim: false }), foot);
    }
}

impl Desk {
    pub(crate) fn prepare_device(
        &self,
        config: &ConfigManager,
        device: &mut Connection,
    ) -> Result<()> {
        if config.tunnels().iter().any(|r| r.device_id == device.id) && !self.can_open(&device.id) {
            return Err(AppError::ValidationError(
                "Start this device's VPS tunnel in Settings → T before connecting.".into(),
            ));
        }
        config.apply_host_pin(device)
    }
}

impl<B: Backend + Write> App<B> {
    pub(crate) fn tunnel_key(&mut self, key: KeyEvent) {
        self.mark_redraw();
        let cfg = &self.config;
        let desk = &mut self.tunnels;
        let profiles = cfg.connections();
        if let Some(e) = &mut desk.editor {
            let n = profiles.len();
            match key.code {
                KeyCode::Esc => desk.editor = None,
                KeyCode::Tab | KeyCode::Down => e.field = (e.field + 1) % 3,
                KeyCode::BackTab | KeyCode::Up => e.field = (e.field + 2) % 3,
                KeyCode::Left | KeyCode::Right if e.field < 2 && n > 0 => {
                    let value = if e.field == 0 {
                        &mut e.gateway
                    } else {
                        &mut e.device
                    };
                    *value = (*value + if key.code == KeyCode::Right { 1 } else { n - 1 }) % n;
                }
                KeyCode::Backspace if e.field == 2 => {
                    e.port.pop();
                }
                KeyCode::Char(c) if e.field == 2 && c.is_ascii_digit() && e.port.len() < 5 => {
                    e.port.push(c)
                }
                KeyCode::Enter => {
                    let route = TunnelRoute {
                        gateway_id: profiles[e.gateway].id.clone(),
                        device_id: profiles[e.device].id.clone(),
                        service_port: e.port.parse().unwrap_or(0),
                    };
                    match self.config.save_tunnel(e.original, route) {
                        Ok(i) => {
                            desk.selected = i;
                            desk.editor = None;
                        }
                        Err(e) => self.set_error(e),
                    }
                }
                _ => {}
            }
            return;
        }
        if desk.delete {
            if key.code == KeyCode::Char('y') {
                match self.config.remove_tunnel(desk.selected) {
                    Ok(()) => {
                        desk.selected = desk
                            .selected
                            .min(self.config.tunnels().len().saturating_sub(1))
                    }
                    Err(e) => self.set_error(e),
                }
            }
            if matches!(key.code, KeyCode::Char('y' | 'n') | KeyCode::Esc) {
                self.tunnels.delete = false;
            }
            return;
        }
        let selected = desk.selected;
        let current = cfg.tunnels().get(selected).cloned();
        match key.code {
            KeyCode::Esc => self.workspace_page = Page::Settings,
            KeyCode::Up | KeyCode::Char('k') => desk.selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                desk.selected = (selected + 1).min(cfg.tunnels().len().saturating_sub(1))
            }
            KeyCode::Char('e' | 'd')
                if current
                    .as_ref()
                    .is_some_and(|r| desk.runs.contains_key(&r.device_id)) =>
            {
                self.set_error(AppError::ValidationError(
                    "Stop the tunnel before changing its route.".into(),
                ));
            }
            KeyCode::Char('d') if current.is_some() => desk.delete = true,
            KeyCode::Char('n' | 'N' | 'e') => {
                if profiles.len() < 2 {
                    self.set_error(AppError::ValidationError("First save two SSH profiles in Devices: the VPS and the device at 127.0.0.1. Ctrl+S saves a profile offline.".into()));
                    return;
                }
                let old = current.as_ref().filter(|_| key.code == KeyCode::Char('e'));
                desk.editor = Some(Editor {
                    original: old.map(|_| selected),
                    field: 0,
                    gateway: old
                        .and_then(|r| profiles.iter().position(|c| c.id == r.gateway_id))
                        .unwrap_or(0),
                    device: old
                        .and_then(|r| profiles.iter().position(|c| c.id == r.device_id))
                        .unwrap_or_else(|| {
                            profiles
                                .iter()
                                .enumerate()
                                .skip(1)
                                .find(|(_, c)| c.host == "127.0.0.1")
                                .map_or(1, |(i, _)| i)
                        }),
                    port: old.map(|r| r.service_port.to_string()).unwrap_or_default(),
                });
            }
            KeyCode::Enter if let Some(route) = current => self.start_tunnel(route),
            KeyCode::Char('x') if let Some(route) = current => self.stop_tunnel(&route.device_id),
            KeyCode::Char('o') if let Some(route) = current => {
                if let Some(i) = profiles.iter().position(|c| c.id == route.device_id) {
                    self.begin_connection(
                        profiles[i].clone(),
                        i,
                        ConnectingSource::ConnectionList {
                            file_explorer: false,
                        },
                    );
                } else {
                    self.set_error(AppError::ConfigError("Device profile is missing.".into()));
                }
            }
            _ => {}
        }
    }

    fn start_tunnel(&mut self, route: TunnelRoute) {
        if let Some(run) = self.tunnels.runs.get_mut(&route.device_id) {
            if let Some(e) = run.pending.take() {
                let gateway = run.gateway.clone();
                self.offer_host_key_or_error(gateway, e);
                return;
            }
            if run.phase != Phase::Blocked {
                return;
            }
        }
        let result = self
            .config
            .tunnel_endpoints(&route)
            .and_then(|(gateway, listen)| {
                let (worker, rx) = HomeTunnel::start(gateway.clone(), listen, route.service_port)?;
                Ok((gateway, worker, rx))
            });
        let (gateway, worker, mut rx) = match result {
            Ok(v) => v,
            Err(e) => {
                self.set_error(e);
                return;
            }
        };
        let Some(tx) = self.get_event_sender() else {
            return;
        };
        self.tunnels.generation += 1;
        let generation = self.tunnels.generation;
        let id = route.device_id;
        self.tunnels.runs.insert(
            id.clone(),
            Run {
                generation,
                worker: Some(worker),
                gateway,
                phase: Phase::Connecting,
                message: "Connecting to VPS…".into(),
                pending: None,
            },
        );
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if tx
                    .send(AppEvent::HomeTunnel {
                        device: id.clone(),
                        generation,
                        event,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn stop_tunnel(&mut self, id: &str) {
        let Some(r) = self.tunnels.runs.get_mut(id) else {
            return;
        };
        if r.phase == Phase::Blocked {
            self.tunnels.runs.remove(id);
            return;
        }
        let Some(worker) = r.worker.take() else {
            return;
        };
        r.phase = Phase::Stopping;
        r.message = "Stopping…".into();
        let generation = r.generation;
        let id = id.to_owned();
        let sender = self.get_event_sender();
        tokio::spawn(async move {
            worker.stop().await;
            // The worker may have had a full event queue at shutdown. A joined
            // worker is authoritative even then; old generations are ignored.
            if let Some(tx) = sender {
                let _ = tx
                    .send(AppEvent::HomeTunnel {
                        device: id,
                        generation,
                        event: HomeEvent::Stopped,
                    })
                    .await;
            }
        });
    }

    pub(crate) fn handle_tunnel_event(&mut self, id: String, generation: u64, event: HomeEvent) {
        let Some(r) = self.tunnels.runs.get_mut(&id) else {
            return;
        };
        if r.generation != generation {
            return;
        }
        if r.phase == Phase::Stopping && !matches!(event, HomeEvent::Stopped) {
            return;
        }
        match event {
            HomeEvent::Connecting => {
                r.phase = Phase::Connecting;
                r.message = "Connecting to VPS…".into();
            }
            HomeEvent::Ready => {
                r.phase = Phase::Ready;
                r.message = "Forward ready · Open a shell to verify device SSH.".into();
            }
            HomeEvent::ServiceUnavailable(reason) => {
                r.phase = Phase::Ready;
                r.message = format!("Device service unavailable: {reason}");
            }
            HomeEvent::Retrying { seconds, reason } => {
                r.phase = Phase::Retry;
                r.message = format!("VPS offline · Retry in {seconds}s: {reason}");
            }
            HomeEvent::Blocked(error) => {
                r.phase = Phase::Blocked;
                r.worker = None;
                r.message = format!("Blocked · {error}")
                    .lines()
                    .next()
                    .unwrap_or("Blocked")
                    .to_owned();
                let focused = self.workspace_page == Page::Tunnels
                    && matches!(self.mode, AppMode::ConnectionList(_))
                    && self.tunnels.editor.is_none()
                    && !self.tunnels.delete
                    && self.error.is_none()
                    && self.info.is_none()
                    && self
                        .config
                        .tunnels()
                        .get(self.tunnels.selected)
                        .is_some_and(|route| route.device_id == id);
                if focused {
                    let gateway = r.gateway.clone();
                    self.offer_host_key_or_error(gateway, error);
                } else {
                    r.pending = Some(error);
                }
            }
            HomeEvent::Stopped => {
                self.tunnels.runs.remove(&id);
            }
        }
        self.mark_redraw();
    }
}
