//! Headless Unix store: a random per-user AES key, never alongside the device TOML.
use super::{KEY_LEN, Result, error};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};
use zeroize::Zeroizing;

pub(super) const SCHEME: &str = "unix-key-v1";

#[cfg(not(test))]
fn key_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or_else(|| error("Cannot find the current user's data directory."))?
        .join("rust-remote")
        .join("credentials"))
}

// Unit tests must never open the user's own store.
#[cfg(test)]
fn key_dir() -> Result<PathBuf> {
    thread_local! { static ROOT: tempfile::TempDir = tempfile::tempdir().unwrap(); }
    Ok(ROOT.with(|root| root.path().canonicalize().unwrap().join("credentials")))
}

fn directory(path: &Path, create: bool) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    if !path.is_absolute() {
        return Err(error("Credential directory must be absolute."));
    }
    let mut current = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => continue,
            Component::Normal(name) => current.push(name),
            _ => return Err(error("Invalid credential directory.")),
        }
        if create {
            match fs::DirBuilder::new().mode(0o700).create(&current) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(error("Cannot create private credential directory.")),
            }
        }
        let meta = fs::symlink_metadata(&current).map_err(|_| {
            error("Cannot open credential directory. Restore the original user key.")
        })?;
        // A directory writable by another uid can be renamed out from under us.
        // Root-owned sticky ancestors such as /tmp still protect user-owned children.
        if !meta.is_dir()
            || (meta.uid() != uid && meta.uid() != 0)
            || (meta.mode() & 0o022 != 0 && !(meta.uid() == 0 && meta.mode() & 0o1000 != 0))
            || (current == path && (meta.uid() != uid || meta.mode() & 0o077 != 0))
        {
            return Err(error(
                "Credential directory ownership or permissions are unsafe. Require a private directory (0700).",
            ));
        }
    }
    Ok(())
}

fn read_key(path: &Path) -> io::Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut file = fs::File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.len() != KEY_LEN as u64
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Private key file requires its original owner, length and 0600 permissions.",
        ));
    }
    let mut key = Zeroizing::new([0; KEY_LEN]);
    file.read_exact(&mut key[..])?;
    Ok(key)
}

fn master(dir: &Path, create: bool) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    directory(dir, create)?;
    let path = dir.join("master.key");
    match read_key(&path) {
        Ok(key) => return Ok(key),
        Err(e) if create && e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(error(
                "Cannot unlock credentials. Restore the original private master.key with owner-only permissions.",
            ));
        }
    }
    let mut key = Zeroizing::new([0; KEY_LEN]);
    SystemRandom::new()
        .fill(&mut key[..])
        .map_err(|_| error("Cannot generate a credential key."))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(&key[..])?;
    tmp.as_file().sync_all()?;
    // Publish a complete 0600 key without replacement. Concurrent first-time users
    // either publish their key or read the winner; no one can observe a partial key.
    match tmp.persist_noclobber(&path) {
        Ok(_) => {
            fs::File::open(dir)?.sync_all()?;
            Ok(key)
        }
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
            read_key(&path).map_err(|_| error("Cannot read the existing credential key."))
        }
        Err(_) => Err(error("Cannot save a private credential key.")),
    }
}

/// Hold one key for the full config transaction, including all new envelopes.
pub(crate) struct StoreKey {
    dir: PathBuf,
    raw: Zeroizing<[u8; KEY_LEN]>,
    cipher: LessSafeKey,
}
impl StoreKey {
    pub(crate) fn open(existing: bool) -> Result<Self> {
        let dir = key_dir()?;
        let raw = master(&dir, !existing)?;
        let cipher = LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, &raw[..])
                .map_err(|_| error("Invalid credential key."))?,
        );
        Ok(Self { dir, raw, cipher })
    }
    pub(crate) fn verify_current(&self) -> Result<()> {
        if master(&self.dir, false)?[..] != self.raw[..] {
            return Err(error(
                "Credential key changed during save. Restore the original user key.",
            ));
        }
        Ok(())
    }
    pub(crate) fn envelope(&self, password: &str) -> Result<String> {
        Ok(format!(
            "{SCHEME}:{}",
            STANDARD.encode(self.seal(password.as_bytes())?)
        ))
    }
    pub(crate) fn verify(&self, envelope: &str) -> Result<()> {
        let (scheme, base64) = envelope
            .split_once(':')
            .ok_or_else(|| error("Invalid credential envelope."))?;
        if scheme != SCHEME {
            return Err(error("Unsupported credential format."));
        }
        let bytes = STANDARD
            .decode(base64)
            .map_err(|_| error("Invalid protected credential encoding."))?;
        let _discard = Zeroizing::new(self.unlock(&bytes)?);
        Ok(())
    }
    fn seal(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| error("Cannot generate credential nonce."))?;
        let mut ciphertext = bytes.to_vec();
        self.cipher
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(SCHEME.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| error("Cannot protect this credential."))?;
        Ok([&nonce[..], &ciphertext].concat())
    }
    fn unlock(&self, blob: &[u8]) -> Result<Vec<u8>> {
        let (nonce, ciphertext) = blob
            .split_at_checked(NONCE_LEN)
            .ok_or_else(|| error("Invalid credential envelope."))?;
        let mut buf = Zeroizing::new(ciphertext.to_vec());
        let plaintext = self
            .cipher
            .open_in_place(
                Nonce::try_assume_unique_for_key(nonce)
                    .map_err(|_| error("Invalid credential nonce."))?,
                Aad::from(SCHEME.as_bytes()),
                &mut buf,
            )
            .map_err(|_| {
                error("Credential authentication failed. Restore the original user key.")
            })?;
        Ok(plaintext.to_vec())
    }
}

pub(super) fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    StoreKey::open(false)?.seal(bytes)
}
pub(super) fn unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    StoreKey::open(true)?.unlock(blob)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn missing_key_is_never_created_on_read_and_unsafe_key_is_not_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap().join("credentials");
        assert!(master(&dir, false).is_err());
        assert!(!dir.exists());
        let a = master(&dir, true).unwrap();
        let file = dir.join("master.key");
        assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(a[..] == master(&dir, false).unwrap()[..]);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(master(&dir, true).is_err());
        assert!(fs::read(&file).unwrap()[..] == a[..]);
    }
    #[test]
    fn an_open_config_never_generates_a_replacement_or_mixes_keys() {
        use crate::config::manager::{AuthMethod, ConfigManager, Connection};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devices.toml");
        let mut cfg = ConfigManager::with_path(&path).unwrap();
        let device = |name: &str| {
            Connection::new(
                "localhost".into(),
                2222,
                name.into(),
                AuthMethod::Password("ephemeral-credential".to_owned().into()),
            )
        };
        cfg.add_connection(device("first")).unwrap();
        let mut reopened = ConfigManager::with_path(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let key_file = key_dir().unwrap().join("master.key");
        let backup = key_file.with_extension("original");
        fs::rename(&key_file, &backup).unwrap();
        assert!(reopened.add_connection(device("second")).is_err());
        assert!(
            !key_file.exists()
                && fs::read(&path).unwrap() == before
                && reopened.connections().len() == 1
        );
        // Simulate a different, valid private key replacing the first one.
        let mut replacement = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&key_file)
            .unwrap();
        replacement.write_all(&[0x2d; 32]).unwrap();
        drop(replacement);
        assert!(reopened.add_connection(device("second")).is_err());
        assert!(fs::read(&path).unwrap() == before && reopened.connections().len() == 1);
        let mut edit = reopened.connections()[0].clone();
        edit.auth_method = AuthMethod::Password("alternate synthetic".to_owned().into());
        assert!(reopened.save_edited_connection(edit).is_err());
        assert!(fs::read(&path).unwrap() == before);
        fs::rename(backup, key_file).unwrap();
        reopened.add_connection(device("second")).unwrap();
        assert_eq!(
            ConfigManager::with_path(&path).unwrap().connections().len(),
            2
        );
    }

    #[test]
    fn concurrent_first_writers_share_one_complete_random_key() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap().join("credentials");
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        master(&dir, true).unwrap()
                    })
                })
                .collect();
            let keys: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
            assert!(keys.iter().all(|k| k[..] == keys[0][..]));
        });
    }
}
