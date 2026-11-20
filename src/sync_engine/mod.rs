//! A read-only manifest followed by an explicitly approved, one-way transfer.
use crate::{AppError, Result, config::manager::Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Upload,
    Download,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncSpec {
    pub local_root: PathBuf,
    pub remote_root: String,
    pub direction: Direction,
    pub excludes: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    New,
    Conflict,
}
#[derive(Debug)]
pub struct Change {
    pub path: String,
    pub bytes: u64,
    pub kind: ChangeKind,
    pub can_overwrite: bool,
}
#[derive(Debug)]
pub struct Preview {
    pub entries: Vec<Change>,
    pub unchanged: usize,
    pub skipped: usize,
    spec: SyncSpec,
    device: Connection,
    rclone: PathBuf,
    versions: BTreeMap<String, (Version, Option<Version>)>,
}
#[derive(Debug, Clone, Eq, PartialEq)]
struct Version {
    size: u64,
    digest: [u8; 32],
}
#[derive(Clone, Debug)]
pub struct SyncProgress {
    pub message: String,
    pub completed: usize,
    pub total: usize,
    pub bytes: u64,
}
#[derive(Debug)]
pub struct SyncResult {
    pub copied: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub message: String,
}

mod fs;
mod local_dir;
mod private_dir;
mod publish;
mod scope;
pub use scope::{RemoteScope, ScopeMode};
mod rclone;
use fs::{Fs, check_cancel, err, hash_reader, join};
use rclone::Rclone;

#[cfg(test)]
async fn preview(
    rclone: &Path,
    connection: &Connection,
    spec: SyncSpec,
    cancel: CancellationToken,
    progress: mpsc::Sender<SyncProgress>,
) -> Result<Preview> {
    preview_scoped(rclone, connection, spec, None, cancel, progress).await
}
pub async fn preview_scoped(
    rclone: &Path,
    connection: &Connection,
    spec: SyncSpec,
    scope: Option<RemoteScope>,
    cancel: CancellationToken,
    progress: mpsc::Sender<SyncProgress>,
) -> Result<Preview> {
    if connection.public_key.is_none() {
        // Request identity only. No SFTP subsystem or directory operation is
        // available until the scope and the independently verified pin match.
        crate::async_ssh_client::SshSession::new_session_with_timeout(
            connection,
            Some(std::time::Duration::from_secs(10)),
            &cancel,
        )
        .await?;
        return Err(err(
            "Verify the SSH host key before authorizing the remote scope.",
        ));
    }
    scope.ok_or_else(||err("Remote directory authorization required. Confirm the server jail or exclusive access before scanning."))?.claim(connection,&spec)?;
    let job = async {
        let files = Fs::open(connection, &cancel).await?;
        files.verify_roots(&spec).await?;
        let tool = Rclone::new(rclone, connection, &cancel).await?;
        let (paths, mut skipped) = files.scan(&spec, &cancel, &progress).await?;
        let upload = spec.direction == Direction::Upload;
        let mut versions = BTreeMap::new();
        let mut entries = Vec::new();
        let mut unchanged = 0;
        let mut bytes = 0;
        for (idx, path) in paths.iter().enumerate() {
            check_cancel(&cancel)?;
            let src = files
                .version(&spec, upload, path, &cancel)
                .await?
                .ok_or_else(|| err(format!("Source disappeared: {path}")))?;
            let dst = match files.version(&spec, !upload, path, &cancel).await {
                Ok(v) => v,
                Err(AppError::ValidationError(msg))
                    if msg.starts_with("Not a real directory:")
                        || msg.starts_with("Not a regular file:") =>
                {
                    skipped += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };
            bytes += src.size;
            let _ = progress.try_send(SyncProgress {
                message: format!("Comparing {path}"),
                completed: idx + 1,
                total: paths.len(),
                bytes,
            });
            if dst.as_ref() == Some(&src) {
                unchanged += 1;
                continue;
            }
            let kind = if dst.is_some() {
                ChangeKind::Conflict
            } else {
                ChangeKind::New
            };
            entries.push(Change {
                path: path.clone(),
                bytes: src.size,
                kind,
                can_overwrite: kind == ChangeKind::Conflict && !upload && cfg!(windows),
            });
            versions.insert(path.clone(), (src, dst));
        }
        let remote = format!("device:{}", spec.remote_root);
        let local = spec
            .local_root
            .to_str()
            .ok_or_else(|| err("Local root is not UTF-8."))?;
        let (src, dst) = if upload {
            (local, remote.as_str())
        } else {
            (remote.as_str(), local)
        };
        let manifest: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        tool.copy(src, dst, &manifest, true, &cancel, None).await?;
        Ok(Preview {
            entries,
            unchanged,
            skipped,
            spec,
            versions,
            device: connection.clone(),
            rclone: rclone.into(),
        })
    };
    // The copy wrapper owns/reaps its child; cancellation is selected inside
    // subprocesses and each bounded SSH operation.
    job.await
}

pub async fn execute(
    plan: Preview,
    approved_overwrites: BTreeSet<String>,
    cancel: CancellationToken,
    progress: mpsc::Sender<SyncProgress>,
) -> Result<SyncResult> {
    let mut published = (0, 0);
    match execute_inner(plan, approved_overwrites, cancel, progress, &mut published).await {
        Err(e) if published.0 > 0 => Err(err(format!(
            "Published {} file(s), {} bytes before stopping. {e}",
            published.0, published.1
        ))),
        other => other,
    }
}
async fn execute_inner(
    plan: Preview,
    approved_overwrites: BTreeSet<String>,
    cancel: CancellationToken,
    progress: mpsc::Sender<SyncProgress>,
    published: &mut (usize, u64),
) -> Result<SyncResult> {
    let spec = &plan.spec;
    let fs = Fs::open(&plan.device, &cancel).await?;
    fs.verify_roots(spec).await?;
    let tool = Rclone::new(&plan.rclone, &plan.device, &cancel).await?;
    let upload = spec.direction == Direction::Upload;
    let mut result = SyncResult {
        copied: 0,
        skipped: plan.skipped,
        bytes: 0,
        message: String::new(),
    };
    let approved = plan
        .entries
        .iter()
        .filter(|e| {
            e.kind == ChangeKind::New || (e.can_overwrite && approved_overwrites.contains(&e.path))
        })
        .collect::<Vec<_>>();
    let total = approved.len();
    result.skipped += plan.entries.len() - total;
    // Check every approved destination before publishing the first file, and
    // verify each one again during publish. A new source is never in this list.
    for e in &approved {
        check_cancel(&cancel)?;
        if fs.version(spec, !upload, &e.path, &cancel).await? != plan.versions[&e.path].1 {
            return Err(err(format!(
                "Destination changed since preview: {}. Preview again.",
                e.path
            )));
        }
    }
    for e in approved {
        check_cancel(&cancel)?;
        let _ = progress.try_send(SyncProgress {
            message: format!("Copying {}", e.path),
            completed: result.copied,
            total,
            bytes: result.bytes,
        });
        let base = SyncProgress {
            message: format!("Copying {}", e.path),
            completed: result.copied,
            total,
            bytes: result.bytes,
        };
        let (srcver, old) = &plan.versions[&e.path];
        // Snapshot/verify the bytes exactly once before publication. Check the
        // path again here so a directory replaced by a link is never traversed.
        if !fs.parent_ok(spec, upload, &e.path, false).await?
            || (!upload
                && !fs
                    .remote
                    .symlink_metadata(join(&spec.remote_root, &e.path))
                    .await?
                    .file_type()
                    .is_file())
        {
            return Err(err(format!(
                "Source path changed since preview: {}",
                e.path
            )));
        }
        if upload {
            upload_one(
                &fs,
                &tool,
                spec,
                &e.path,
                srcver,
                &cancel,
                (&progress, base.clone()),
            )
            .await?;
        } else {
            let stage = private_dir::PrivateDir::new_in(tool.temp.path())?;
            let local = stage
                .path()
                .to_str()
                .ok_or_else(|| err("Local staging path is not UTF-8."))?;
            tool.copy(
                &format!("device:{}", spec.remote_root),
                local,
                &[&e.path],
                false,
                &cancel,
                Some((&progress, base.clone())),
            )
            .await?;
            let tmp = stage.path().join(&e.path);
            if hash_reader(
                tokio::fs::File::from_std(fs::open_local_file(&tmp)?),
                &cancel,
            )
            .await?
                != *srcver
            {
                return Err(err(format!("Source changed during copy: {}", e.path)));
            }
            check_cancel(&cancel)?;
            publish::publish_local(&tmp, &spec.local_root.join(&e.path), old.as_ref(), &cancel)
                .await?;
        }
        result.copied += 1;
        result.bytes += e.bytes;
        *published = (result.copied, result.bytes);
        let _ = progress.try_send(SyncProgress {
            message: format!("Copied {}", e.path),
            completed: result.copied,
            total,
            bytes: result.bytes,
        });
    }
    result.message = format!(
        "Copied {} file(s), {} bytes; kept/skipped {}.",
        result.copied, result.bytes, result.skipped
    );
    Ok(result)
}

async fn upload_one(
    fs: &Fs,
    tool: &Rclone,
    spec: &SyncSpec,
    rel: &str,
    srcver: &Version,
    c: &CancellationToken,
    progress: (&mpsc::Sender<SyncProgress>, SyncProgress),
) -> Result<()> {
    // Snapshot the approved local file first. rclone never uploads from a mutable user tree.
    let local = private_dir::PrivateDir::new_in(tool.temp.path())?;
    let snapshot = local.path().join("payload");
    let mut input = tokio::fs::File::from_std(fs::open_local_file(&spec.local_root.join(rel))?);
    let mut out = tokio::fs::File::create(&snapshot).await?;
    tokio::select! { _=c.cancelled()=>return Err(err("Cancelled")), r=tokio::io::copy(&mut input,&mut out)=>{r?;} }
    drop(out);
    if hash_reader(tokio::fs::File::open(&snapshot).await?, c).await? != *srcver {
        return Err(err(format!("Source changed during copy: {rel}")));
    }
    let stage = join(
        &spec.remote_root,
        &format!(".rrw-sync-{}", uuid::Uuid::new_v4()),
    );
    let file = join(&stage, "payload");
    fs.remote_dir(&spec.remote_root, false)
        .await?
        .then_some(())
        .ok_or_else(|| err("Remote root disappeared."))?;
    fs.private_staging(&stage).await?;
    let outcome=async{
        let local=local.path().to_str().ok_or_else(||err("Cannot represent temporary path."))?;
        tool.copy(local,&format!("device:{stage}"),&["payload"],false,c,Some(progress)).await?;
        check_cancel(c)?;
        if !fs.remote_dir(&stage,false).await? || !fs.remote.symlink_metadata(file.clone()).await?.file_type().is_file() {
            return Err(err("Staged file is missing or has changed type."));
        }
        if hash_reader(fs.remote.open(file.clone()).await?,c).await? != *srcver{return Err(err("Staged content no longer matches the approved file."));}
        fs.parent_ok(spec,false,rel,true).await?;
        let dest=join(&spec.remote_root,rel);
        // SSH_FXP_HARDLINK fails if the target already exists, including a dangling link.
        if !fs.remote.hardlink(file.clone(),dest.clone()).await? {
            // SFTP v3 fallback for servers without the hardlink extension. EXCL
            // preserves a racing target. On disconnect an incomplete *new*
            // file may remain and is reported; do not delete by a stale path.
            use russh_sftp::protocol::OpenFlags;
            use tokio::io::AsyncWriteExt;
            let mut dst=fs.remote.open_with_flags(dest,OpenFlags::CREATE|OpenFlags::EXCLUDE|OpenFlags::WRITE).await?;
            let mut src=tokio::fs::File::open(&snapshot).await?;
            let write=async{tokio::io::copy(&mut src,&mut dst).await?;dst.shutdown().await};
            tokio::select!{
                _=c.cancelled()=>return Err(err(format!("Cancelled; incomplete new file may remain: {rel}"))),
                r=write=>r.map_err(|_|err(format!("Disconnected/write failed; incomplete new file may remain: {rel}")))?,
            }
        }
        Ok(())
    }.await;
    // Only this randomly named, exclusively created job directory is cleaned.
    // Recheck parents on reconnect; never recurse through an untrusted tree.
    let clean = async {
        if !fs.remote_dir(&stage, false).await? {
            return Ok(());
        }
        for attempt in 0..5 {
            match fs.remote.remove_file(file.clone()).await {
                Ok(()) => break,
                Err(e) if fs::not_found(&e) => break,
                Err(_) if attempt < 4 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100 << attempt)).await
                }
                Err(e) => return Err(e.into()),
            }
        }
        fs.remote.remove_dir(stage.clone()).await?;
        Ok::<_, AppError>(())
    }
    .await;
    if clean.is_err() {
        return Err(err(format!(
            "{}. Check unfinished staging directory: {stage}",
            outcome
                .err()
                .map_or("Copied target".into(), |e: AppError| e.to_string())
        )));
    }
    outcome
}
#[cfg(test)]
mod tests;
