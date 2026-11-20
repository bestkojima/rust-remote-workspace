//! One file, two explicit endpoints, one publication boundary.
use super::{SftpFileSystem, sftp_file};
use crate::{
    AppError, Result,
    async_ssh_client::{HostFile, ProgressReporter, SshClient, SshSession},
    session::Connection,
};
use russh::{
    Channel,
    client::{Handle, Msg},
};
use russh_sftp::{
    client::{RawSftpSession, SftpSession, error::Error as SftpError},
    protocol::{OpenFlags, Packet, StatusCode},
};
use std::{cell::Cell, path::Path, sync::Arc, time::Duration};
use tokio::{fs, sync::Mutex};
use tokio_util::sync::CancellationToken;

/// An authenticated SSH transport plus its SFTP session. Clones share the transport.
#[derive(Clone)]
pub struct SftpEndpoint {
    pub(super) fs: SftpFileSystem,
    ssh: Arc<Mutex<Handle<SshClient>>>,
}
impl SftpEndpoint {
    /// Open SFTP without requesting a PTY or shell. Supply the effective Host Key
    /// of the device; untrusted or changed keys fail before user authentication.
    pub async fn connect(
        device: &Connection,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let open = async {
            let session =
                SshSession::new_session_with_timeout(device, Some(timeout), cancel).await?;
            let ch = session.channel_open_session().await?;
            ch.request_subsystem(true, "sftp").await?;
            let sftp = SftpSession::new(ch.into_stream()).await?;
            Ok(Self::from_parts(
                SftpFileSystem::new(sftp),
                Arc::new(Mutex::new(session)),
            ))
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AppError::TransferCancelled),
            out = tokio::time::timeout(timeout, open) => out.map_err(|_| fail("SFTP connection timed out"))?,
        }
    }
    pub(crate) fn from_parts(fs: SftpFileSystem, ssh: Arc<Mutex<Handle<SshClient>>>) -> Self {
        Self { fs, ssh }
    }
    /// Interpret the string on this server (POSIX syntax, including on Windows clients).
    pub fn file<'a>(&'a self, path: &'a str) -> FileLocation<'a> {
        FileLocation::Remote(self, path)
    }
    async fn channel(&self) -> Result<Channel<Msg>> {
        Ok(self.ssh.lock().await.channel_open_session().await?)
    }
}

/// Local paths keep their native representation; remote paths are sent as UTF-8.
#[derive(Clone, Copy)]
pub enum FileLocation<'a> {
    Local(&'a Path),
    Remote(&'a SftpEndpoint, &'a str),
}
/// Bytes streamed so far. Receiving the last byte does not yet mean the target was published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileProgress {
    pub transferred: u64,
    pub total: Option<u64>,
}
struct Report<F> {
    notify: F,
    total: Cell<Option<u64>>,
}
impl<F: Fn(FileProgress)> ProgressReporter for Report<F> {
    fn set_total_bytes(&self, total: Option<u64>) {
        self.total.set(total);
    }
    fn report_progress(&self, transferred: u64) {
        (self.notify)(FileProgress {
            transferred,
            total: self.total.get(),
        });
    }
}
fn fail(msg: impl Into<String>) -> AppError {
    AppError::SftpError(msg.into())
}
fn sibling(path: &str, tag: &str) -> String {
    let dir = path.rsplit_once('/').map_or(".", |(dir, _)| dir);
    format!("{dir}/.rrw-{tag}-{}", uuid::Uuid::new_v4())
}

/// Copy one file between local and SFTP, or between two SFTP endpoints. `Ok`
/// means the finished file was published. Acknowledged cancellation returns
/// `AppError::TransferCancelled`; a cleanup failure reports the recovery path.
/// Run on the caller's Tokio runtime and keep the progress callback non-blocking.
pub async fn copy_file(
    source: FileLocation<'_>,
    destination: FileLocation<'_>,
    cancel: &CancellationToken,
    notify: impl Fn(FileProgress),
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(AppError::TransferCancelled);
    }
    let transport = match (source, destination) {
        (_, FileLocation::Remote(peer, _)) | (FileLocation::Remote(peer, _), _) => peer,
        _ => {
            return Err(AppError::ValidationError(
                "At least one file must be on an SFTP endpoint.".into(),
            ));
        }
    };
    let open = async { SshSession::open_sftp_channel(transport.channel().await?).await };
    let raw = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AppError::TransferCancelled),
        s = open => s?,
    };
    let progress = Report {
        notify,
        total: Cell::new(None),
    };
    if let FileLocation::Local(dest) = destination {
        let FileLocation::Remote(_, from) = source else {
            unreachable!()
        };
        let (output, tmp) = local_stage(dest)?.into_parts();
        SshSession::receive_sftp(raw, from, fs::File::from_std(output), cancel, progress).await?;
        if cancel.is_cancelled() {
            return Err(AppError::TransferCancelled);
        }
        tmp.persist(dest).map_err(|e| AppError::IOError(e.error))?;
        return Ok(());
    }
    let FileLocation::Remote(dst, dest) = destination else {
        unreachable!()
    };
    let session = dst.fs.session();
    let tmp = sibling(dest, "part");
    let mut reserve = session
        .open_with_flags_and_attributes(
            &tmp,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            russh_sftp::protocol::FileAttributes {
                permissions: Some(0o600),
                ..russh_sftp::protocol::FileAttributes::empty()
            },
        )
        .await?;
    let streamed: Result<()> = async {
        tokio::io::AsyncWriteExt::shutdown(&mut reserve).await?;
        let mode = match source {
            FileLocation::Local(path) => {
                let file = fs::File::open(path).await?;
                let meta = file.file_metadata().await?;
                SshSession::send_sftp(raw, file, meta, &tmp, cancel, progress).await?;
                meta.permissions
            }
            FileLocation::Remote(src, path) => {
                let file = sftp_file::open_for_read(src.fs.session(), path).await?;
                let meta = file.file_metadata().await?;
                SshSession::send_sftp(raw, file, meta, &tmp, cancel, progress).await?;
                meta.permissions
            }
        };
        if cancel.is_cancelled() {
            return Err(AppError::TransferCancelled);
        }
        publish_remote(dst, &tmp, dest, mode).await
    }
    .await;
    if let Err(error) = streamed {
        return match session.remove_file(&tmp).await {
            Ok(()) => Err(error),
            Err(SftpError::Status(s)) if s.status_code == StatusCode::NoSuchFile => Err(error),
            Err(e) => Err(fail(format!(
                "{error}; check unfinished file {tmp} after reconnect: {e}"
            ))),
        };
    }
    Ok(())
}

fn local_stage(dest: &Path) -> Result<tempfile::NamedTempFile> {
    let dir = dest
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let prior = match std::fs::symlink_metadata(dest) {
        Ok(meta) if !meta.is_file() => {
            return Err(fail(format!(
                "Destination is not a regular file: {}",
                dest.display()
            )));
        }
        Ok(_) => Some(std::fs::OpenOptions::new().write(true).open(dest)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    #[cfg(windows)]
    {
        Ok(crate::config::manager::replacement_file(
            dir,
            prior.as_ref(),
        )?)
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::Builder::new()
            .prefix(".rrw-part-")
            .tempfile_in(dir)?;
        if let Some(prior) = prior {
            let mode = prior.metadata()?.permissions().mode() & 0o777;
            tmp.as_file()
                .set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        Ok(tmp)
    }
}

async fn publish_remote(
    side: &SftpEndpoint,
    tmp: &str,
    dest: &str,
    source_mode: Option<u32>,
) -> Result<()> {
    let s = side.fs.session();
    match s.symlink_metadata(dest).await {
        Err(SftpError::Status(st)) if st.status_code == StatusCode::NoSuchFile => {
            if let Some(mode) = source_mode {
                s.set_metadata(
                    tmp,
                    russh_sftp::protocol::FileAttributes {
                        permissions: Some(mode & 0o777),
                        ..russh_sftp::protocol::FileAttributes::empty()
                    },
                )
                .await?;
            }
            return Ok(s.rename(tmp, dest).await?);
        }
        Err(e) => return Err(e.into()),
        Ok(attrs) if attrs.is_dir() => {
            return Err(fail(format!("Destination is a directory: {dest}")));
        }
        Ok(prior) => {
            if !prior.is_regular() {
                return Err(fail(format!("Destination is not a regular file: {dest}")));
            }
            // A writable parent directory alone does not authorize changing a
            // read-only destination. Open it without truncating before replacing.
            let mut check = s.open_with_flags(dest, OpenFlags::WRITE).await?;
            tokio::io::AsyncWriteExt::shutdown(&mut check).await?;
            let staged = s.symlink_metadata(tmp).await?;
            let change_owner = prior.uid.zip(prior.gid) != staged.uid.zip(staged.gid);
            s.set_metadata(
                tmp,
                russh_sftp::protocol::FileAttributes {
                    uid: if change_owner { prior.uid } else { None },
                    gid: if change_owner { prior.gid } else { None },
                    permissions: prior.permissions.map(|m| m & 0o777),
                    ..russh_sftp::protocol::FileAttributes::empty()
                },
            )
            .await?;
        }
    }
    let ch = side.channel().await?;
    ch.request_subsystem(true, "sftp").await?;
    let raw = RawSftpSession::new(ch.into_stream());
    let version = raw.init().await?;
    if version.extensions.contains_key("posix-rename@openssh.com") {
        let mut data = Vec::new();
        for p in [tmp, dest] {
            data.extend_from_slice(&(p.len() as u32).to_be_bytes());
            data.extend_from_slice(p.as_bytes());
        }
        return match raw.extended("posix-rename@openssh.com", data).await? {
            Packet::Status(s) if s.status_code == StatusCode::Ok => Ok(()),
            Packet::Status(s) => Err(SftpError::Status(s).into()),
            _ => Err(fail("Unexpected response to atomic rename.")),
        };
    }
    // SFTP v3 rename cannot replace an existing file. Keep the previous file
    // beside it through publication; name the recovery copy if the link dies.
    let backup = sibling(dest, "backup");
    s.rename(dest, &backup)
        .await
        .map_err(|e| fail(format!("Could not move original {dest} to {backup}: {e}")))?;
    if let Err(e) = s.rename(tmp, dest).await {
        return match s.rename(&backup, dest).await {
            Ok(()) => Err(fail(format!("Publish failed; previous file restored: {e}"))),
            Err(restore) => Err(fail(format!(
                "Publish failed ({e}); recover the original from {backup}: {restore}"
            ))),
        };
    }
    s.remove_file(&backup)
        .await
        .map_err(|e| fail(format!("File published; remove old backup {backup}: {e}")))
}
