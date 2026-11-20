use super::local_dir::LocalDir;
use super::*;
use crate::async_ssh_client::{SshClient, SshSession};
use futures::TryStreamExt;
use ring::digest::{Context, SHA256};
use russh_sftp::{
    client::{SftpSession, error::Error},
    protocol::StatusCode,
};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

pub(super) fn err(msg: impl Into<String>) -> AppError {
    AppError::ValidationError(msg.into())
}
pub(super) fn check_cancel(c: &CancellationToken) -> Result<()> {
    if c.is_cancelled() {
        Err(err("Cancelled; preview again before retrying."))
    } else {
        Ok(())
    }
}
pub(super) fn component(s: &str) -> bool {
    let reserved = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.starts_with(".rrw-")
        && !s.ends_with([' ', '.'])
        && !s
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        && !matches!(
            reserved.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "CONIN$"
                | "CONOUT$"
                | "CLOCK$"
                | "COM¹"
                | "COM²"
                | "COM³"
                | "LPT¹"
                | "LPT²"
                | "LPT³"
        )
        && !(reserved.len() == 4
            && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
            && reserved.as_bytes()[3].is_ascii_digit())
}
pub(super) fn join(root: &str, rel: &str) -> String {
    if rel.is_empty() {
        root.to_owned()
    } else {
        format!("{}/{rel}", root.trim_end_matches('/'))
    }
}
pub(super) fn local_is_link(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}

pub(super) struct Fs {
    pub remote: SftpSession,
    _ssh: russh::client::Handle<SshClient>,
}
impl Fs {
    pub async fn open(c: &Connection, cancel: &CancellationToken) -> Result<Self> {
        let ssh = SshSession::new_session_with_timeout(
            c,
            Some(std::time::Duration::from_secs(10)),
            cancel,
        )
        .await?;
        let channel = ssh.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let remote = SftpSession::new(channel.into_stream()).await?;
        remote.set_timeout(15).await;
        Ok(Self { remote, _ssh: ssh })
    }
    pub async fn private_staging(&self, dir: &str) -> Result<()> {
        use russh_sftp::{client::RawSftpSession, protocol::FileAttributes};
        // SSH_FXP_MKDIR carries permissions at creation; setting them later
        // would expose an interval in which other accounts could plant entries.
        let channel = self._ssh.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let raw = RawSftpSession::new(channel.into_stream());
        raw.set_timeout(15).await;
        raw.init().await?;
        raw.mkdir(
            dir,
            FileAttributes {
                permissions: Some(0o700),
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }
    pub async fn verify_roots(&self, s: &SyncSpec) -> Result<()> {
        if !s.local_root.is_absolute()
            || !s.remote_root.starts_with('/')
            || s.remote_root.starts_with("//")
        {
            return Err(err("Choose existing absolute local and SFTP directories."));
        }
        if !verify_local_dir(&s.local_root, false).await? {
            return Err(err("Local root does not exist."));
        }
        if !self.remote_dir(&s.remote_root, false).await? {
            return Err(err("Remote root does not exist."));
        }
        Ok(())
    }
    pub async fn remote_dir(&self, path: &str, create: bool) -> Result<bool> {
        let mut cur = String::new();
        for part in path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
        {
            if part == "."
                || part == ".."
                || part.contains('\\')
                || part.chars().any(char::is_control)
            {
                return Err(err("Unsafe remote path component."));
            }
            cur.push('/');
            cur.push_str(part);
            match self.remote.symlink_metadata(cur.clone()).await {
                Ok(m) if m.file_type().is_dir() => {}
                Ok(_) => return Err(err(format!("Not a real directory: {cur}"))),
                Err(e) if not_found(&e) && create => {
                    self.remote.create_dir(cur.clone()).await?;
                }
                Err(e) if not_found(&e) => return Ok(false),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(true)
    }
    pub async fn parent_ok(
        &self,
        s: &SyncSpec,
        local: bool,
        rel: &str,
        create: bool,
    ) -> Result<bool> {
        let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
        if local {
            verify_local_dir(&s.local_root.join(parent), create).await
        } else {
            self.remote_dir(&join(&s.remote_root, parent), create).await
        }
    }
    pub async fn version(
        &self,
        s: &SyncSpec,
        local: bool,
        rel: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<Version>> {
        if !local && !self.parent_ok(s, false, rel, false).await? {
            return Ok(None);
        }
        if local {
            let intended = s.local_root.join(rel);
            let Some(dir) = LocalDir::open(
                intended
                    .parent()
                    .ok_or_else(|| err("Missing file parent."))?,
                false,
            )?
            else {
                return Ok(None);
            };
            let path = dir.path().join(
                intended
                    .file_name()
                    .ok_or_else(|| err("Missing file name."))?,
            );
            match tokio::fs::symlink_metadata(&path).await {
                Ok(m) if m.is_file() && !local_is_link(&m) => {}
                Ok(_) => return Err(err(format!("Not a regular file: {rel}"))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            let f = open_checked_file(&path)?;
            Ok(Some(
                hash_reader(tokio::fs::File::from_std(f), cancel).await?,
            ))
        } else {
            let path = join(&s.remote_root, rel);
            match self.remote.symlink_metadata(path.clone()).await {
                Ok(m) if m.file_type().is_file() => {}
                Ok(_) => return Err(err(format!("Not a regular file: {rel}"))),
                Err(e) if not_found(&e) => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            Ok(Some(
                hash_reader(self.remote.open(path).await?, cancel).await?,
            ))
        }
    }
    pub async fn scan(
        &self,
        s: &SyncSpec,
        c: &CancellationToken,
        tx: &mpsc::Sender<SyncProgress>,
    ) -> Result<(Vec<String>, usize)> {
        let rules: Vec<_> = s
            .excludes
            .iter()
            .map(|v| glob::Pattern::new(v).map_err(|_| err(format!("Invalid exclusion: {v}"))))
            .collect::<Result<_>>()?;
        let local = s.direction == Direction::Upload;
        let mut dirs = vec![String::new()];
        let mut files = Vec::new();
        let mut skipped = 0;
        while let Some(parent) = dirs.pop() {
            check_cancel(c)?;
            let mut add = |name: String, is_dir: bool, is_file: bool| {
                let rel = if parent.is_empty() {
                    name.clone()
                } else {
                    format!("{parent}/{name}")
                };
                if !component(&name)
                    || rules.iter().any(|p| {
                        p.matches(&name)
                            || p.matches(&rel)
                            || (is_dir && p.matches(&format!("{rel}/")))
                    })
                {
                    skipped += 1;
                } else if is_dir {
                    dirs.push(rel)
                } else if is_file {
                    files.push(rel)
                } else {
                    skipped += 1
                }
            };
            if local {
                let path = s.local_root.join(&parent);
                let anchor = LocalDir::open(&path, false)?
                    .ok_or_else(|| err("Source directory disappeared."))?;
                let mut iter = tokio::fs::read_dir(anchor.path()).await?;
                while let Some(ent) = iter.next_entry().await? {
                    check_cancel(c)?;
                    let m = tokio::fs::symlink_metadata(ent.path()).await?;
                    if let Some(name) = ent.file_name().to_str() {
                        let link = local_is_link(&m);
                        add(name.to_owned(), m.is_dir() && !link, m.is_file() && !link);
                    } else {
                        add(String::new(), false, false);
                    }
                }
            } else {
                let path = join(&s.remote_root, &parent);
                if !self.remote_dir(&path, false).await? {
                    return Err(err("Source directory disappeared."));
                }
                let mut iter = self.remote.read_dir_stream(path).await?;
                while let Some(ent) = iter.try_next().await? {
                    check_cancel(c)?;
                    let t = ent.file_type();
                    add(ent.file_name(), t.is_dir(), t.is_file());
                }
            }
            let _ = tx.try_send(SyncProgress {
                message: "Scanning directories".into(),
                completed: 0,
                total: files.len(),
                bytes: 0,
            });
        }
        files.sort();
        Ok((files, skipped))
    }
}
pub(super) fn not_found(e: &Error) -> bool {
    matches!(e,Error::Status(s) if s.status_code==StatusCode::NoSuchFile)
}

pub(super) async fn verify_local_dir(path: &Path, create: bool) -> Result<bool> {
    Ok(LocalDir::open(path, create)?.is_some())
}
pub(super) fn open_local_file(path: &Path) -> Result<std::fs::File> {
    let dir = LocalDir::open(
        path.parent().ok_or_else(|| err("Missing file parent."))?,
        false,
    )?
    .ok_or_else(|| err("Missing directory."))?;
    let path = dir
        .path()
        .join(path.file_name().ok_or_else(|| err("Missing file name."))?);
    open_checked_file(&path)
}
fn open_checked_file(path: &Path) -> Result<std::fs::File> {
    let mut op = std::fs::OpenOptions::new();
    op.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        op.custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        op.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = op.open(path)?;
    let m = f.metadata()?;
    if !m.is_file() || local_is_link(&m) {
        return Err(err("Refusing to read a link or special file."));
    }
    Ok(f)
}
pub(super) async fn hash_reader(
    mut r: impl AsyncRead + Unpin,
    c: &CancellationToken,
) -> Result<Version> {
    let mut ctx = Context::new(&SHA256);
    let mut buf = vec![0; 64 * 1024];
    let mut size = 0;
    loop {
        let read = tokio::select! { _=c.cancelled()=>return Err(err("Cancelled; preview again before retrying.")), n=r.read(&mut buf)=> n? };
        if read == 0 {
            break;
        }
        ctx.update(&buf[..read]);
        size += read as u64;
    }
    Ok(Version {
        size,
        digest: ctx.finish().as_ref().try_into().unwrap(),
    })
}
