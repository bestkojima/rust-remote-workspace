use super::{App, AppMode, LeftExplorer};
use crate::{
    AppError, AppEvent, Result,
    config::manager::{Connection, SyncProfile},
    sync_engine::{
        self, ChangeKind, Direction, Preview, RemoteScope, ScopeMode, SyncProgress, SyncResult,
    },
    workspace::Page,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use std::{cell::Cell, collections::BTreeSet, io::Write};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
mod editor;
mod view;
use editor::{Editor, Field};

#[derive(Default)]
pub(crate) struct SyncDesk {
    selected: usize,
    state: Screen,
}
#[derive(Default)]
enum Screen {
    #[default]
    List,
    Edit(Box<Editor>),
    Tool(Field),
    Delete,
    History {
        selected: usize,
    },
    Working(Job),
    Grant {
        profile: SyncProfile,
        device: Connection,
        visible: Cell<bool>,
    },
    Review(Box<Review>),
    Result(String),
    Identity(Connection, AppError),
}
struct Review {
    profile: SyncProfile,
    device: Connection,
    plan: Preview,
    selected: usize,
    approved: BTreeSet<String>,
    new_files: usize,
    selected_bytes: u64,
    confirm: bool,
    visible: Cell<bool>,
}
impl Review {
    fn selection(&self) -> (usize, usize, u64) {
        (
            self.new_files + self.approved.len(),
            self.approved.len(),
            self.selected_bytes,
        )
    }
}
struct Job {
    profile: SyncProfile,
    device: Connection,
    stage: Stage,
    cancel: CancellationToken,
    progress: watch::Receiver<SyncProgress>,
    result: oneshot::Receiver<Finish>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
enum Finish {
    Identity(Result<()>),
    Preview(Result<Preview>),
    Copy(Result<SyncResult>),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Identity,
    Scan,
    Copy,
}

impl SyncDesk {
    pub fn editing(&self) -> bool {
        matches!(
            self.state,
            Screen::Edit(_) | Screen::Tool(_) | Screen::Delete | Screen::Grant { .. }
        )
    }
    pub fn working(&self) -> bool {
        matches!(self.state, Screen::Working(_))
    }
    pub fn paste(&mut self, data: &str) {
        match &mut self.state {
            Screen::Edit(e) => e.paste(data),
            Screen::Tool(f) => f.paste(data),
            _ => {}
        }
    }
}

impl<B: Backend + Write> App<B> {
    pub(crate) fn sync_from_files(&mut self) {
        if self.sync.working() || matches!(self.sync.state, Screen::Review(_)) {
            self.set_error(AppError::ValidationError(
                "Finish or discard the current Sync job in F3 first.".into(),
            ));
            return;
        }
        let AppMode::FileExplorer {
            left_explorer,
            remote_explorer,
            ssh_connection,
            return_to,
            ..
        } = &self.mode
        else {
            return;
        };
        if !matches!(left_explorer, LeftExplorer::Local(_)) {
            self.set_error(AppError::ValidationError(
                "Select Local in the left pane to make a sync pair.".into(),
            ));
            return;
        }
        let Some(remote) = remote_explorer.cwd().to_str() else {
            self.set_error(AppError::ValidationError(
                "This remote path cannot be represented in a sync profile.".into(),
            ));
            return;
        };
        let mut p = SyncProfile::new(ssh_connection.id.clone());
        p.spec.local_root = left_explorer.cwd().to_path_buf();
        p.spec.remote_root = remote.to_owned();
        p.name = p
            .spec
            .local_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&ssh_connection.display_name)
            .to_owned();
        let selected = *return_to;
        self.go_to_connection_list_with_selected(selected);
        self.workspace_page = Page::Sync;
        self.sync.state = Screen::Edit(Box::new(Editor::new(p)));
        self.mark_redraw();
    }

    pub(crate) fn sync_key(&mut self, key: KeyEvent) -> bool {
        self.mark_redraw();
        let selected = self.sync.selected;
        let n = self.config.sync_profiles().len();
        match &mut self.sync.state {
            Screen::List => match key.code {
                KeyCode::Char('q') => return true,
                KeyCode::Up | KeyCode::Char('k') => self.sync.selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.sync.selected = (selected + 1).min(n.saturating_sub(1))
                }
                KeyCode::Char('n' | 'N') => {
                    if let Some(d) = self.config.connections().first() {
                        self.sync.state =
                            Screen::Edit(Box::new(Editor::new(SyncProfile::new(d.id.clone()))));
                    } else {
                        self.set_error(AppError::ValidationError(
                            "First add an SSH device in F4 Devices.".into(),
                        ));
                    }
                }
                KeyCode::Char('e') if let Some(p) = self.config.sync_profiles().get(selected) => {
                    self.sync.state = Screen::Edit(Box::new(Editor::new(p.clone())));
                }
                KeyCode::Char('r' | 'R') => {
                    self.sync.state =
                        Screen::Tool(Field::new(&self.config.rclone_path().to_string_lossy()))
                }
                KeyCode::Char('d') if selected < n => self.sync.state = Screen::Delete,
                KeyCode::Char('u')
                    if let Some(mut p) = self.config.sync_profiles().get(selected).cloned()
                        && p.saved_jail.take().is_some() =>
                {
                    if let Err(e) = self.config.save_sync_profile(p) {
                        self.set_error(e);
                    }
                }
                KeyCode::Char('h') if let Some(p) = self.config.sync_profiles().get(selected) => {
                    self.sync.state = Screen::History {
                        selected: p.history.len().saturating_sub(1),
                    }
                }
                KeyCode::Enter
                    if let Some(p) = self.config.sync_profiles().get(selected).cloned() =>
                {
                    self.sync_preview(p)
                }
                _ => {}
            },
            Screen::Edit(e) => match key.code {
                KeyCode::Esc => self.sync.state = Screen::List,
                KeyCode::Enter => match self.config.save_sync_profile(e.value()) {
                    Ok(i) => {
                        self.sync.selected = i;
                        self.sync.state = Screen::List;
                    }
                    Err(e) => self.set_error(e),
                },
                _ => e.key(key, self.config.connections()),
            },
            Screen::Tool(f) => match key.code {
                KeyCode::Esc => self.sync.state = Screen::List,
                KeyCode::Enter => match self.config.set_rclone_path(f.value().trim().into()) {
                    Ok(()) => self.sync.state = Screen::List,
                    Err(e) => self.set_error(e),
                },
                _ => f.key(key),
            },
            Screen::Delete => match key.code {
                KeyCode::Char('y') if key.modifiers.is_empty() => {
                    if let Some(p) = self.config.sync_profiles().get(selected) {
                        let id = p.id.clone();
                        if let Err(e) = self.config.delete_sync_profile(&id) {
                            self.set_error(e);
                            return false;
                        }
                    }
                    self.sync.selected = selected.min(n.saturating_sub(2));
                    self.sync.state = Screen::List;
                }
                KeyCode::Esc | KeyCode::Char('n') => self.sync.state = Screen::List,
                _ => {}
            },
            Screen::Working(j) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('x')) {
                    j.cancel.cancel();
                }
            }
            Screen::Grant {
                profile, visible, ..
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('n' | 'x') => self.sync.state = Screen::List,
                KeyCode::Char('e') if visible.get() && key.modifiers.is_empty() => {
                    self.authorize_scope(ScopeMode::ExclusiveRun)
                }
                KeyCode::Char('j')
                    if visible.get()
                        && key.modifiers.is_empty()
                        && profile.spec.remote_root == "/" =>
                {
                    self.authorize_scope(ScopeMode::JailedRoot)
                }
                _ => {}
            },
            Screen::Review(r) => {
                if r.confirm {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('n') => {
                            r.confirm = false;
                            r.visible.set(false);
                        }
                        KeyCode::Char('y') if key.modifiers.is_empty() && r.visible.get() => {
                            self.sync_execute()
                        }
                        _ => {}
                    }
                } else {
                    let last = r.plan.entries.len().saturating_sub(1);
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            r.selected = r.selected.saturating_sub(1);
                            r.visible.set(false);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            r.selected = (r.selected + 1).min(last);
                            r.visible.set(false);
                        }
                        KeyCode::PageDown => {
                            r.selected = (r.selected + 10).min(last);
                            r.visible.set(false);
                        }
                        KeyCode::PageUp => {
                            r.selected = r.selected.saturating_sub(10);
                            r.visible.set(false);
                        }
                        KeyCode::Char(' ') if r.visible.get() => {
                            if let Some(c) = r.plan.entries.get(r.selected)
                                && c.kind == ChangeKind::Conflict
                                && c.can_overwrite
                            {
                                if r.approved.remove(&c.path) {
                                    r.selected_bytes -= c.bytes;
                                } else {
                                    r.approved.insert(c.path.clone());
                                    r.selected_bytes += c.bytes;
                                }
                            }
                        }
                        KeyCode::Enter if r.selection().0 > 0 => {
                            r.confirm = true;
                            r.visible.set(false);
                        }
                        KeyCode::Esc => self.sync.state = Screen::List,
                        _ => {}
                    }
                }
            }
            Screen::Result(_) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                    self.sync.state = Screen::List;
                }
            }
            Screen::Identity(..) => {
                if matches!(key.code, KeyCode::Enter) {
                    let Screen::Identity(device, err) = std::mem::take(&mut self.sync.state) else {
                        unreachable!()
                    };
                    self.offer_host_key_or_error(device, err);
                } else if key.code == KeyCode::Esc {
                    self.sync.state = Screen::List;
                }
            }
            Screen::History { selected: i } => match key.code {
                KeyCode::Esc | KeyCode::Enter => self.sync.state = Screen::List,
                KeyCode::Up => *i = i.saturating_sub(1),
                KeyCode::Down => {
                    *i = (*i + 1).min(
                        self.config
                            .sync_profiles()
                            .get(selected)
                            .map_or(0, |p| p.history.len().saturating_sub(1)),
                    )
                }
                _ => {}
            },
        }
        false
    }

    fn sync_device(&self, p: &SyncProfile) -> Result<Connection> {
        let mut d = self
            .config
            .find_connection(&p.device_id)
            .cloned()
            .ok_or_else(|| AppError::ConfigError("Sync device no longer exists.".into()))?;
        self.tunnels.prepare_device(&self.config, &mut d)?;
        Ok(d)
    }

    fn sync_preview(&mut self, p: SyncProfile) {
        let d = match self.sync_device(&p) {
            Ok(d) => d,
            Err(e) => {
                self.set_error(e);
                return;
            }
        };
        if let Some(jail) = p
            .saved_jail
            .as_ref()
            .filter(|g| g.mode() == ScopeMode::JailedRoot && g.matches(&d, &p.spec))
            .cloned()
        {
            self.start_scoped_preview(p, d, jail);
        } else {
            let dev = d.clone();
            self.sync_job(p,d,Stage::Identity,move |cancel,_|async move {
                // KEX checks the saved Host Key before user authentication. Do not
                // open a file channel or start rclone before directory consent.
                use std::time::Duration;
                let deadline=Duration::from_secs(10);
                let handshake=crate::async_ssh_client::SshSession::new_session_with_timeout(&dev,Some(deadline),&cancel);
                let verified=tokio::select!{
                    _=cancel.cancelled()=>Err(AppError::SshConnectionError("Cancelled.".into())),
                    res=tokio::time::timeout(deadline,handshake)=>match res{
                        Ok(Ok(session))=>{let _=tokio::time::timeout(Duration::from_secs(1),session.disconnect(russh::Disconnect::ByApplication,"","")).await;Ok(())},
                        Ok(Err(e))=>Err(e),
                        Err(_)=>Err(AppError::SshConnectionError("SSH identity verification timed out.".into())),
                    }
                };
                Finish::Identity(verified)
            });
        }
    }
    fn authorize_scope(&mut self, mode: ScopeMode) {
        let Screen::Grant {
            mut profile,
            device,
            ..
        } = std::mem::take(&mut self.sync.state)
        else {
            return;
        };
        let run = (|| {
            let scope = RemoteScope::confirm(&device, &profile.spec, mode)?;
            let current = self.sync_device(&profile)?;
            if !scope.matches(&current, &profile.spec) {
                return Err(AppError::ValidationError(
                    "The SSH profile changed. Verify it again before granting access.".into(),
                ));
            }
            if mode == ScopeMode::JailedRoot {
                profile.saved_jail = Some(scope.clone());
                self.config.save_sync_profile(profile.clone())?;
            }
            Ok((current, scope))
        })();
        match run {
            Ok((d, scope)) => self.start_scoped_preview(profile, d, scope),
            Err(e) => self.set_error(e),
        }
    }
    fn start_scoped_preview(&mut self, p: SyncProfile, d: Connection, scope: RemoteScope) {
        let tool = self.config.rclone_path().to_owned();
        let spec = p.spec.clone();
        let dev = d.clone();
        self.sync_job(p, d, Stage::Scan, move |cancel, progress| async move {
            Finish::Preview(
                sync_engine::preview_scoped(&tool, &dev, spec, Some(scope), cancel, progress).await,
            )
        });
    }
    fn sync_execute(&mut self) {
        let Screen::Review(r) = std::mem::take(&mut self.sync.state) else {
            return;
        };
        let d = match self.sync_device(&r.profile) {
            Ok(d) => d,
            Err(e) => {
                self.set_error(e);
                return;
            }
        };
        if d.host != r.device.host
            || d.port != r.device.port
            || d.username != r.device.username
            || d.public_key != r.device.public_key
            || d.auth_method != r.device.auth_method
        {
            self.set_error(AppError::ValidationError("The device profile changed after this scan. Preview the new endpoint before copying.".into()));
            return;
        }
        let Review {
            profile,
            plan,
            approved,
            ..
        } = *r;
        self.sync_job(
            profile,
            d,
            Stage::Copy,
            move |cancel, progress| async move {
                Finish::Copy(sync_engine::execute(plan, approved, cancel, progress).await)
            },
        );
    }
    fn sync_job<F, U>(&mut self, p: SyncProfile, device: Connection, stage: Stage, start: F)
    where
        F: FnOnce(CancellationToken, mpsc::Sender<SyncProgress>) -> U + Send + 'static,
        U: Future<Output = Finish> + Send + 'static,
    {
        let Some(tx) = self.get_event_sender() else {
            return;
        };
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let (progress_tx, progress) = watch::channel(SyncProgress {
            message: "Starting...".into(),
            completed: 0,
            total: 0,
            bytes: 0,
        });
        let (result_tx, result) = oneshot::channel();
        tokio::spawn(async move {
            let (feed, mut rx) = mpsc::channel(16);
            let work = start(token, feed);
            tokio::pin!(work);
            let done = loop {
                tokio::select! { biased;
                    done=&mut work=>break done,
                    Some(p)=rx.recv()=>{progress_tx.send_replace(p);let _=tx.try_send(AppEvent::SyncUpdate);}
                }
            };
            while let Ok(p) = rx.try_recv() {
                progress_tx.send_replace(p);
            }
            let _ = result_tx.send(done);
            let _ = tx.send(AppEvent::SyncUpdate).await;
        });
        self.sync.state = Screen::Working(Job {
            profile: p,
            device,
            stage,
            cancel,
            progress,
            result,
        });
    }

    pub(crate) fn sync_update(&mut self) {
        let Screen::Working(job) = &mut self.sync.state else {
            return;
        };
        let finish = match job.result.try_recv() {
            Ok(value) => value,
            Err(oneshot::error::TryRecvError::Empty) => {
                self.mark_redraw();
                return;
            }
            Err(_) => Finish::Copy(Err(AppError::ConfigError(
                "Sync worker ended before reporting its result.".into(),
            ))),
        };
        let Screen::Working(job) = std::mem::take(&mut self.sync.state) else {
            unreachable!()
        };
        let id = job.profile.id.clone();
        let completed = job.progress.borrow().completed;
        let cancelled = job.cancel.is_cancelled();
        let (state, note) = match finish {
            Finish::Identity(Ok(())) if !cancelled => {
                self.sync.state = Screen::Grant {
                    profile: job.profile.clone(),
                    device: job.device.clone(),
                    visible: Cell::new(false),
                };
                self.mark_redraw();
                return;
            }
            Finish::Preview(Ok(plan)) if !cancelled => {
                let (new_files, selected_bytes) = plan
                    .entries
                    .iter()
                    .filter(|e| e.kind == ChangeKind::New)
                    .fold((0, 0u64), |(n, b), e| (n + 1, b + e.bytes));
                let note = format!(
                    "Preview / {} change(s), {} unchanged, {} skipped. No files written.",
                    plan.entries.len(),
                    plan.unchanged,
                    plan.skipped
                );
                (
                    Screen::Review(Box::new(Review {
                        profile: job.profile.clone(),
                        device: job.device.clone(),
                        plan,
                        selected: 0,
                        approved: Default::default(),
                        new_files,
                        selected_bytes,
                        confirm: false,
                        visible: Cell::new(false),
                    })),
                    note,
                )
            }
            Finish::Copy(Ok(r)) => {
                let note = format!(
                    "Finished / {} copied, {} skipped / {} bytes. {}",
                    r.copied, r.skipped, r.bytes, r.message
                );
                (Screen::Result(note.clone()), note)
            }
            Finish::Identity(Err(e)) | Finish::Preview(Err(e)) | Finish::Copy(Err(e))
                if matches!(
                    e,
                    AppError::UnknownHostKey { .. } | AppError::HostKeyChanged { .. }
                ) =>
            {
                if self.workspace_page == Page::Sync
                    && matches!(self.mode, AppMode::ConnectionList(_))
                    && self.error.is_none()
                    && self.info.is_none()
                {
                    self.offer_host_key_or_error(job.device.clone(), e);
                } else {
                    self.sync.state = Screen::Identity(job.device.clone(), e);
                }
                // A trust decision precedes the task; declining it leaves the
                // saved device and its profile exactly as the operator found them.
                self.mark_redraw();
                return;
            }
            result => {
                let err = match result {
                    Finish::Identity(Err(e)) | Finish::Preview(Err(e)) | Finish::Copy(Err(e)) => {
                        e.to_string()
                    }
                    _ => "Preview discarded.".into(),
                };
                let tag = if cancelled { "Cancelled" } else { "Failed" };
                let note = if job.stage == Stage::Copy {
                    format!("{tag} / {completed} completed. {err}  Preview again before retrying.")
                } else {
                    format!("{tag} / No files written. {err}")
                };
                (Screen::Result(note.clone()), note)
            }
        };
        self.sync.state = state;
        if let Err(e) = self.config.log_sync_result(&id, &note) {
            self.set_error(AppError::ConfigError(format!(
                "Could not save the sync log: {e}"
            )));
        }
        self.mark_redraw();
    }
}
