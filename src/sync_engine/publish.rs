use super::fs::{err, hash_reader, local_is_link};
use super::*;

#[cfg(windows)]
fn rename_locked(file: &std::fs::File, to: &Path) -> std::io::Result<()> {
    use std::{
        mem::{offset_of, size_of},
        os::windows::{ffi::OsStrExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
    };
    let wide: Vec<u16> = to.as_os_str().encode_wide().collect();
    let len = offset_of!(FILE_RENAME_INFO, FileName) + wide.len() * 2;
    let mut memory = vec![
        0usize;
        len.max(size_of::<FILE_RENAME_INFO>())
            .div_ceil(size_of::<usize>())
    ];
    // Aligned storage owns FileName and the entire native header; zeroed flags
    // mean never replace an existing path. The handle pins the checked file.
    unsafe {
        let p = memory.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*p).FileNameLength = (wide.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), (*p).FileName.as_mut_ptr(), wide.len());
        if SetFileInformationByHandle(file.as_raw_handle(), FileRenameInfo, p.cast(), len as u32)
            == 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

async fn candidate(
    stage: &Path,
    parent: &Path,
    _original: Option<&std::fs::File>,
    cancel: &CancellationToken,
) -> Result<tempfile::TempPath> {
    #[cfg(windows)]
    let tmp = crate::config::manager::replacement_file(parent, _original)?;
    #[cfg(not(windows))]
    let tmp = tempfile::NamedTempFile::new_in(parent)?;
    let (write, path) = tmp.into_parts();
    let mut out = tokio::fs::File::from_std(write);
    let mut src = tokio::fs::File::open(stage).await?;
    tokio::select! {
        _=cancel.cancelled()=>return Err(err("Cancelled; original file kept.")),
        n=tokio::io::copy(&mut src,&mut out)=>{n?;}
    }
    out.sync_all().await?;
    Ok(path)
}
pub(super) async fn publish_local(
    stage: &Path,
    dest: &Path,
    old: Option<&Version>,
    c: &CancellationToken,
) -> Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| err("Missing destination parent."))?;
    let anchor = super::local_dir::LocalDir::open(parent, true)?
        .ok_or_else(|| err("Destination parent is missing."))?;
    let parent = anchor.path();
    let dst = parent.join(
        dest.file_name()
            .ok_or_else(|| err("Missing destination name."))?,
    );
    let dest = dst.as_path();
    let Some(old) = old else {
        // Hard-link publication fails atomically when a competing writer creates the path.
        let prepared = candidate(stage, parent, None, c).await?;
        std::fs::hard_link(&prepared, dest).map_err(|e| {
            err(format!(
                "Could not publish new file (nothing overwritten): {e}"
            ))
        })?;
        return Ok(());
    };
    #[cfg(not(windows))]
    {
        let _ = (old, c);
        return Err(err("Cannot lock a competing writer; destination kept."));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::{
            Foundation::GENERIC_READ,
            Storage::FileSystem::{DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ},
        };
        let f = std::fs::OpenOptions::new()
            .access_mode(GENERIC_READ | DELETE)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(dest)
            .map_err(|e| {
                err(format!(
                    "Could not exclusively acquire destination; file kept: {e}"
                ))
            })?;
        if local_is_link(&f.metadata()?) || !f.metadata()?.is_file() {
            return Err(err("Destination changed type; file kept."));
        }
        let mut file = tokio::fs::File::from_std(f);
        let actual = hash_reader(&mut file, c).await?;
        if &actual != old {
            return Err(err("Destination changed since preview; file kept."));
        }
        let file = file.into_std().await;
        let prepared = candidate(stage, parent, Some(&file), c).await?;
        let recovery = parent.join(format!(".rrw-recovery-{}", uuid::Uuid::new_v4()));
        rename_locked(&file, &recovery)?;
        if let Err(e) = std::fs::hard_link(&prepared, dest) {
            if rename_locked(&file, dest).is_err() {
                return Err(err(format!(
                    "Target claimed by another writer. Original retained at {}",
                    recovery.display()
                )));
            }
            return Err(err(format!("Original restored after failed publish: {e}")));
        }
        drop(file);
        std::fs::remove_file(recovery)?;
        Ok(())
    }
}
