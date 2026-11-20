//! Windows Data Protection, scoped to the current user and used without UI.
use super::{Result, error};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
use zeroize::Zeroize;

pub(super) const SCHEME: &str = "dpapi-v1";

pub(super) fn protect(input: &[u8]) -> Result<Vec<u8>> {
    transform(input, true)
}

pub(super) fn unprotect(input: &[u8]) -> Result<Vec<u8>> {
    transform(input, false)
}

fn transform(input: &[u8], protect: bool) -> Result<Vec<u8>> {
    let src = CRYPT_INTEGER_BLOB {
        cbData: input
            .len()
            .try_into()
            .map_err(|_| error("Credential too large."))?,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut dst = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // DPAPI only reads src. No machine scope, optional entropy, prompt or description.
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &src,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut dst,
            )
        } else {
            CryptUnprotectData(
                &src,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut dst,
            )
        }
    };
    if ok == 0 {
        return Err(error(if protect {
            "Windows could not protect this credential."
        } else {
            "Windows could not unlock this credential for the current user."
        }));
    }
    let bytes = if dst.cbData != 0 {
        // On success DPAPI returns cbData initialized bytes in a LocalAlloc buffer.
        let output = unsafe { std::slice::from_raw_parts_mut(dst.pbData, dst.cbData as usize) };
        let bytes = output.to_vec();
        if !protect {
            output.zeroize();
        }
        bytes
    } else {
        Vec::new()
    };
    unsafe {
        LocalFree(dst.pbData.cast());
    }
    Ok(bytes)
}
