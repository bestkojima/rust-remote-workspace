//! Plan in the background, stream with TermiRs' bounded SFTP pipeline, and
//! publish each finished file separately. Esc never exposes a partial destination.
use super::{
    dir_walker,
    single_file::{FileLocation, SftpEndpoint, copy_file},
};
use crate::{
    AppError, AppEvent, CopyDirection, CopyOperation, Result, ScpFileProgress, ScpFileResult,
    ScpResult, ScpTransferProgress, ScpTransferSpec, ui::ScpMode,
};
use std::path::Path;
use tokio::{fs, sync::mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) enum Side {
    Local,
    Remote(SftpEndpoint),
}
fn fail(msg: impl Into<String>) -> AppError {
    AppError::SftpError(msg.into())
}
fn cancelled() -> AppError {
    AppError::TransferCancelled
}
fn join(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}
impl Side {
    fn file<'a>(&'a self, path: &'a str) -> FileLocation<'a> {
        match self {
            Self::Local => FileLocation::Local(Path::new(path)),
            Self::Remote(endpoint) => endpoint.file(path),
        }
    }
    async fn ensure_dir(&self, path: &str) -> Result<()> {
        match self {
            Self::Local => match fs::create_dir(path).await {
                Ok(()) => Ok(()),
                Err(e)
                    if e.kind() == std::io::ErrorKind::AlreadyExists
                        && fs::symlink_metadata(path).await?.is_dir() =>
                {
                    Ok(())
                }
                Err(e) => Err(e.into()),
            },
            Self::Remote(peer) => match peer.fs.session().create_dir(path).await {
                Ok(()) => Ok(()),
                Err(_)
                    if peer
                        .fs
                        .session()
                        .symlink_metadata(path)
                        .await
                        .is_ok_and(|a| a.is_dir()) =>
                {
                    Ok(())
                }
                Err(e) => Err(e.into()),
            },
        }
    }
}

pub(crate) async fn run(
    items: Vec<CopyOperation>,
    dest_dir: String,
    left: Side,
    right: Side,
    cancel: CancellationToken,
    events: mpsc::Sender<AppEvent>,
) {
    let (tx, mut rx) = mpsc::channel::<ScpResult>(32);
    let forward = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if events.send(AppEvent::SftpProgress(msg)).await.is_err() {
                break;
            }
        }
    });
    let end = match batch(items, &dest_dir, &left, &right, &cancel, &tx).await {
        Ok(end) => end,
        Err(AppError::TransferCancelled) => ScpResult::Cancelled(vec![]),
        Err(e) => ScpResult::Error {
            error: e.to_string(),
        },
    };
    let _ = tx.send(end).await;
    drop(tx);
    let _ = forward.await;
}
async fn batch(
    items: Vec<CopyOperation>,
    dest_dir: &str,
    left: &Side,
    right: &Side,
    cancel: &CancellationToken,
    tx: &mpsc::Sender<ScpResult>,
) -> Result<ScpResult> {
    let upload = items
        .first()
        .is_some_and(|i| i.direction == CopyDirection::LeftToRight);
    let (src, dst) = if upload { (left, right) } else { (right, left) };
    let mode = if upload {
        ScpMode::Send
    } else {
        ScpMode::Receive
    };
    let mut dirs = Vec::new();
    let mut specs = Vec::new();
    for item in items {
        let name = item.source_name.trim_end_matches('/');
        dir_walker::validate_name(name, matches!(dst, Side::Local))?;
        let destination = if matches!(dst, Side::Local) {
            Path::new(dest_dir)
                .join(name)
                .to_string_lossy()
                .into_owned()
        } else {
            join(dest_dir, name)
        };
        let files = if item.is_dir {
            let walk = async {
                match src {
                    Side::Local => {
                        dir_walker::walk_local_dir(&item.source_path, &destination).await
                    }
                    Side::Remote(peer) => {
                        dir_walker::walk_remote_dir(
                            &peer.fs,
                            &item.source_path,
                            &destination,
                            matches!(dst, Side::Local),
                        )
                        .await
                    }
                }
            };
            let manifest = tokio::select! { biased; _=cancel.cancelled()=>return Err(cancelled()),res=walk=>res? };
            dirs.extend(manifest.directories);
            manifest.files
        } else {
            vec![(item.source_path, destination, None)]
        };
        for (source, destination, _) in files {
            let name = if matches!(dst, Side::Local) {
                Path::new(&destination)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                destination.rsplit('/').next().unwrap_or("").to_owned()
            };
            let (local_path, remote_path) = if upload {
                (source, destination)
            } else {
                (destination, source)
            };
            specs.push(ScpTransferSpec {
                mode,
                local_path,
                remote_path,
                display_name: name.clone(),
                destination_filename: name,
                is_ssh_to_ssh: matches!(left, Side::Remote(..)),
            });
        }
    }
    if specs.is_empty() && dirs.is_empty() {
        return Err(fail("No files selected."));
    }
    tx.send(ScpResult::Planned(
        specs.iter().map(ScpFileProgress::from_spec).collect(),
    ))
    .await
    .map_err(|_| cancelled())?;
    for dir in dirs {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        dst.ensure_dir(&dir).await?;
    }
    let mut results = Vec::new();
    let mut any_cancelled = false;
    let mut any_failed = false;
    for (index, spec) in specs.into_iter().enumerate() {
        let (from, to) = if upload {
            (left.file(&spec.local_path), right.file(&spec.remote_path))
        } else {
            (right.file(&spec.remote_path), left.file(&spec.local_path))
        };
        let result = copy_file(from, to, cancel, |update| {
            let _ = tx.try_send(ScpResult::Progress(ScpTransferProgress {
                file_index: index,
                transferred_bytes: update.transferred,
                total_bytes: update.total,
            }));
        })
        .await;
        match &result {
            Err(AppError::TransferCancelled) => any_cancelled = true,
            Err(_) => any_failed = true,
            Ok(_) => {}
        }
        results.push(ScpFileResult {
            mode,
            local_path: spec.local_path,
            remote_path: spec.remote_path,
            destination_filename: spec.destination_filename,
            success: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
            completed_at: Some(std::time::Instant::now()),
        });
    }
    Ok(if any_cancelled && !any_failed {
        ScpResult::Cancelled(results)
    } else {
        ScpResult::Completed(results)
    })
}
