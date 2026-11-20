//! Local staging payloads are private from the first byte, including when
//! the caller's selected parent grants read access to a group.
use std::{
    io,
    path::{Path, PathBuf},
};
pub(super) struct PrivateDir(PathBuf);
impl PrivateDir {
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        let dir = parent.join(format!(".rrw-sync-{}", uuid::Uuid::new_v4()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
        #[cfg(windows)]
        {
            use std::{os::windows::ffi::OsStrExt, ptr::null_mut};
            use windows_sys::Win32::{
                Foundation::LocalFree,
                Security::{
                    Authorization::{
                        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
                    },
                    SECURITY_ATTRIBUTES,
                },
                Storage::FileSystem::CreateDirectoryW,
            };
            let acl: Vec<_> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)"
                .encode_utf16()
                .chain([0])
                .collect();
            let name: Vec<_> = dir.as_os_str().encode_wide().chain([0]).collect();
            let mut descriptor = null_mut();
            // Owner-rights and SYSTEM are the only inheritable access entries.
            // CreateDirectoryW consumes this descriptor before the name is visible.
            unsafe {
                if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    acl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let sa = SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor,
                    bInheritHandle: 0,
                };
                let status = CreateDirectoryW(name.as_ptr(), &sa);
                let result = if status != 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                };
                LocalFree(descriptor);
                result?;
            }
        }
        Ok(Self(dir))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
