//! Anchor path-based local operations. On Windows, prevent ancestor deletion;
//! on Linux address the opened directory through its descriptor, not its old name.
use super::{
    Result,
    fs::{err, local_is_link},
};
use std::{
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};

pub(super) struct LocalDir {
    path: PathBuf,
    _handles: Vec<File>,
}
impl LocalDir {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn open(path: &Path, create: bool) -> Result<Option<Self>> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(err(
                "Choose an absolute local directory without dot components.",
            ));
        }
        let mut current = PathBuf::new();
        #[cfg(windows)]
        let mut handles = Vec::new();
        #[cfg(unix)]
        let mut parent = File::open("/")?;
        for part in path.components() {
            current.push(part);
            let Component::Normal(name) = part else {
                continue;
            };
            #[cfg(windows)]
            let open = || {
                use std::os::windows::fs::OpenOptionsExt;
                use windows_sys::Win32::Storage::FileSystem::*;
                File::options()
                    .access_mode(FILE_READ_ATTRIBUTES)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(&current)
            };
            #[cfg(unix)]
            let open = || {
                use std::os::{
                    fd::{AsRawFd, FromRawFd},
                    unix::ffi::OsStrExt,
                };
                let n = std::ffi::CString::new(name.as_bytes())?;
                // The previous directory remains open; a symlink in this path component is never traversed.
                let fd = unsafe {
                    libc::openat(
                        parent.as_raw_fd(),
                        n.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(unsafe { File::from_raw_fd(fd) })
                }
            };
            let handle = match open() {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
                    #[cfg(windows)]
                    std::fs::create_dir(&current)?;
                    #[cfg(unix)]
                    {
                        use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
                        let n = std::ffi::CString::new(name.as_bytes())
                            .map_err(|_| err("Invalid directory name."))?;
                        if unsafe { libc::mkdirat(parent.as_raw_fd(), n.as_ptr(), 0o700) } != 0 {
                            return Err(io::Error::last_os_error().into());
                        }
                    }
                    open()?
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::NotADirectory => {
                    return Err(err(format!("Not a real directory: {}", current.display())));
                }
                Err(e) => return Err(e.into()),
            };
            let metadata = handle.metadata()?;
            if !metadata.is_dir() || local_is_link(&metadata) {
                return Err(err(format!("Not a real directory: {}", current.display())));
            }
            #[cfg(windows)]
            {
                let _ = name;
                handles.push(handle);
            }
            #[cfg(unix)]
            {
                parent = handle;
            }
        }
        #[cfg(windows)]
        {
            Ok(Some(Self {
                path: path.into(),
                _handles: handles,
            }))
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let base = if cfg!(target_os = "linux") {
                "/proc/self/fd"
            } else {
                "/dev/fd"
            };
            Ok(Some(Self {
                path: Path::new(base).join(parent.as_raw_fd().to_string()),
                _handles: vec![parent],
            }))
        }
    }
}
