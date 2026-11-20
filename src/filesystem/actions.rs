//! Directory actions apply to a snapshot of the selected pane. Only a
//! successfully refreshed explorer replaces the visible one.
use crate::{ActivePane, App, AppError, AppEvent, AppMode, LeftExplorer};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    backend::Backend,
    layout::Rect,
    style::Stylize,
    text::Line,
    widgets::{Block, Clear, Paragraph, Wrap},
};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};
use tokio::sync::oneshot;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Action {
    Go,
    Mkdir,
    Rename,
    Delete,
}
impl Action {
    fn title(self) -> &'static str {
        match self {
            Self::Go => "Go to directory",
            Self::Mkdir => "Create directory",
            Self::Rename => "Rename entry",
            Self::Delete => "Delete entry",
        }
    }
}
struct Prompt {
    action: Action,
    location: String,
    text: String,
}
type Change = io::Result<(ActivePane, LeftExplorer)>;
#[derive(Default)]
pub(crate) struct FileActions {
    prompt: Option<Prompt>,
    pending: Option<oneshot::Receiver<Change>>,
}
impl FileActions {
    pub fn active(&self) -> bool {
        self.prompt.is_some() || self.pending.is_some()
    }
    pub fn paste(&mut self, data: &str) {
        if let Some(p) = &mut self.prompt {
            p.text.extend(data.chars().filter(|c| !c.is_control()));
        }
    }
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        if !self.active() {
            return;
        }
        let w = area.width.min(80);
        let h = area.height.min(8);
        let rect = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        );
        let (title, lines) = if let Some(p) = &self.prompt {
            (
                p.action.title(),
                vec![
                    Line::raw(&p.location),
                    Line::raw(""),
                    Line::from(format!("› {}▏", p.text).cyan()),
                    Line::raw("Enter  Apply    Esc  Cancel"),
                ],
            )
        } else {
            ("Files", vec![Line::raw("Updating directory…")])
        };
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(format!(" {title} "))),
            rect,
        );
    }
}

fn selected(p: &LeftExplorer) -> Option<&ratatui_async_explorer::File> {
    match p {
        LeftExplorer::Local(e) => e.filtered_files().get(e.selected_idx()),
        LeftExplorer::Remote(e) => e.filtered_files().get(e.selected_idx()),
    }
}
fn is_parent(name: &str) -> bool {
    matches!(name.trim_end_matches('/'), "" | "." | "..")
}
fn basename(name: &str, local: bool) -> io::Result<()> {
    super::dir_walker::validate_name(name, local)
}

impl<B: Backend + Write> App<B> {
    fn file_side(&self) -> Option<(ActivePane, LeftExplorer)> {
        let AppMode::FileExplorer {
            active_pane,
            left_explorer,
            remote_explorer,
            ..
        } = &self.mode
        else {
            return None;
        };
        Some((
            *active_pane,
            match active_pane {
                ActivePane::Left => left_explorer.clone(),
                ActivePane::Right => LeftExplorer::Remote(remote_explorer.clone()),
            },
        ))
    }
    pub(crate) fn prompt_file_action(&mut self, action: Action) {
        let Some((_, side)) = self.file_side() else {
            return;
        };
        let location = if action == Action::Rename {
            let Some(file) = selected(&side).filter(|f| !is_parent(f.name())) else {
                return;
            };
            format!("Current: {}", file.path().display())
        } else {
            format!("In: {}", side.cwd().display())
        };
        self.file_actions.prompt = Some(Prompt {
            action,
            location,
            text: String::new(),
        });
        self.mark_redraw();
    }
    pub(crate) fn file_action_key(&mut self, key: KeyEvent) {
        let Some(p) = self.file_actions.prompt.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.file_actions.prompt = None,
            KeyCode::Backspace => {
                p.text.pop();
            }
            KeyCode::Char(c) if !c.is_control() => p.text.push(c),
            KeyCode::Enter if !p.text.is_empty() => {
                let p = self.file_actions.prompt.take().unwrap();
                self.run_file_action(p.action, p.text);
            }
            _ => {}
        }
        self.mark_redraw();
    }
    pub(crate) fn run_file_action(&mut self, action: Action, text: String) {
        let (Some((pane, mut side)), Some(wake)) = (self.file_side(), self.get_event_sender())
        else {
            return;
        };
        let (tx, rx) = oneshot::channel();
        self.file_actions.pending = Some(rx);
        tokio::spawn(async move {
            let result = apply(&mut side, action, &text).await.map(|()| (pane, side));
            let _ = tx.send(result);
            let _ = wake.send(AppEvent::FileActionDone).await;
        });
        self.mark_redraw();
    }
    pub(crate) fn finish_file_action(&mut self) {
        let Some(mut rx) = self.file_actions.pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok((pane, side))) => {
                if let AppMode::FileExplorer {
                    left_explorer,
                    remote_explorer,
                    search,
                    ..
                } = &mut self.mode
                {
                    search.deactivate();
                    match (pane, side) {
                        (ActivePane::Left, side) => *left_explorer = side,
                        (ActivePane::Right, LeftExplorer::Remote(side)) => *remote_explorer = side,
                        _ => unreachable!(),
                    }
                    left_explorer.set_search_filter(None);
                    remote_explorer.set_search_filter(None);
                }
            }
            Ok(Err(e)) => self.set_error(AppError::SftpError(e.to_string())),
            Err(e) => self.set_error(AppError::SftpError(e.to_string())),
        }
        self.mark_redraw();
    }
}

async fn apply(side: &mut LeftExplorer, action: Action, text: &str) -> io::Result<()> {
    let cwd = side.cwd().to_owned();
    let local = matches!(side, LeftExplorer::Local(_));
    let target = if local {
        cwd.join(text)
    } else if text.starts_with('/') {
        PathBuf::from(text)
    } else {
        PathBuf::from(format!(
            "{}/{}",
            cwd.to_string_lossy().trim_end_matches('/'),
            text
        ))
    };
    if action == Action::Go {
        let dir = match side {
            LeftExplorer::Local(_) => tokio::fs::canonicalize(target).await?,
            LeftExplorer::Remote(e) => PathBuf::from(
                e.filesystem()
                    .session()
                    .canonicalize(target.to_string_lossy().as_ref())
                    .await
                    .map_err(io::Error::other)?,
            ),
        };
        side.set_search_filter(None);
        return side.set_cwd(dir).await;
    }
    if matches!(action, Action::Rename | Action::Mkdir) {
        basename(text, local)?;
    }
    let entry = selected(side)
        .filter(|f| !is_parent(f.name()))
        .map(|f| f.path().to_owned());
    let old = || {
        entry
            .as_deref()
            .ok_or_else(|| io::Error::other("Select a file or directory first."))
    };
    match side {
        LeftExplorer::Local(_) => match action {
            Action::Mkdir => tokio::fs::create_dir(&target).await?,
            Action::Rename => {
                match tokio::fs::symlink_metadata(&target).await {
                    Ok(_) => {
                        return Err(io::Error::other("An entry with that name already exists."));
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                tokio::fs::rename(old()?, &target).await?;
            }
            Action::Delete => {
                let old = old()?;
                if tokio::fs::symlink_metadata(old).await?.is_dir() {
                    tokio::fs::remove_dir(old).await?
                } else {
                    tokio::fs::remove_file(old).await?
                }
            }
            _ => unreachable!(),
        },
        LeftExplorer::Remote(e) => {
            let s = e.filesystem().session();
            let remote = |path: &Path| path.to_string_lossy().into_owned();
            match action {
                Action::Mkdir => s.create_dir(remote(&target)).await,
                Action::Rename => {
                    let dst = remote(&target);
                    match s.symlink_metadata(&dst).await {
                        Ok(_) => {
                            return Err(io::Error::other(
                                "An entry with that name already exists.",
                            ));
                        }
                        Err(russh_sftp::client::error::Error::Status(st))
                            if st.status_code == russh_sftp::protocol::StatusCode::NoSuchFile => {}
                        Err(e) => return Err(io::Error::other(e)),
                    }
                    s.rename(remote(old()?), dst).await
                }
                Action::Delete => {
                    let path = remote(old()?);
                    if s.symlink_metadata(&path)
                        .await
                        .map_err(io::Error::other)?
                        .is_dir()
                    {
                        s.remove_dir(path).await
                    } else {
                        s.remove_file(path).await
                    }
                }
                _ => unreachable!(),
            }
            .map_err(io::Error::other)?;
        }
    }
    side.set_search_filter(None);
    side.set_cwd(cwd).await.map_err(|e| {
        io::Error::other(format!("Change saved, but directory refresh failed: {e}"))
    })?;
    if action != Action::Delete && !side.select_file(text) {
        side.select_file(&format!("{text}/"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_async_explorer::{FileExplorer, LocalFileSystem};
    use std::sync::Arc;
    #[tokio::test]
    async fn directory_actions_do_not_replace_an_existing_name_or_recursively_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let files = FileExplorer::with_fs(
            Arc::new(LocalFileSystem),
            root.to_string_lossy().into_owned(),
        )
        .await
        .unwrap();
        let mut left = LeftExplorer::Local(files);
        apply(&mut left, Action::Mkdir, "子 folder").await.unwrap();
        apply(&mut left, Action::Rename, "renamed").await.unwrap();
        std::fs::write(root.join("other.txt"), b"keep").unwrap();
        std::fs::write(root.join("renamed").join("inside"), b"keep").unwrap();
        assert!(apply(&mut left, Action::Rename, "other.txt").await.is_err());
        assert!(apply(&mut left, Action::Delete, "").await.is_err());
        assert_eq!(std::fs::read(root.join("other.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read(root.join("renamed/inside")).unwrap(), b"keep");
        assert!(apply(&mut left, Action::Mkdir, "../outside").await.is_err());
    }
}
