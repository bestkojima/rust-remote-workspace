use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{addr_of_mut, null_mut},
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER,
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetTokenInformation, IsWellKnownSid,
        OWNER_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
        WinBuiltinAdministratorsSid, WinLocalSystemSid,
    },
    System::{
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

pub(super) fn check(file: &File) -> io::Result<()> {
    // All SIDs below come from the kernel. Their backing token buffer and security
    // descriptor remain live throughout the comparisons.
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token);
        let mut bytes = 0;
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut bytes);
        let mut user = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            user.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let current = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
        let trusted = |sid: PSID| {
            !sid.is_null()
                && (EqualSid(sid, current) != 0
                    || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0)
        };

        let (mut owner, mut acl, mut sd) = (null_mut(), null_mut(), null_mut());
        let err = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut sd,
        );
        if err != 0 {
            return Err(io::Error::from_raw_os_error(err as i32));
        }
        let permitted = trusted(owner)
            && !acl.is_null()
            && (0..u32::from((*acl).AceCount)).all(|i| {
                let mut ace = null_mut();
                if GetAce(acl, i, &mut ace) == 0 {
                    return false;
                }
                match u32::from((*ace.cast::<ACE_HEADER>()).AceType) {
                    ACCESS_DENIED_ACE_TYPE => true,
                    ACCESS_ALLOWED_ACE_TYPE => {
                        trusted(addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast())
                    }
                    _ => false,
                }
            });
        LocalFree(sd);
        if !permitted {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Use a private client configuration: only this Windows account, SYSTEM and Administrators may have access.",
            ));
        }
    }
    Ok(())
}
