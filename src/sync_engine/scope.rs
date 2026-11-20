//! SFTP v3 cannot lock a directory tree or resolve names relative to immutable
//! directory handles. Only a confirmed server-side boundary or an exclusive
//! maintenance window makes path-based access meet the selected-root contract.
use super::{Connection, Result, SyncSpec, fs::err};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeMode {
    /// Administrator verified that this SFTP account is jailed to exactly
    /// the authorized root and other writers cannot take over job staging.
    JailedRoot,
    /// The operator has exclusive control of the route to the root, all its
    /// directories and job staging until this one preview's execution ends.
    ExclusiveRun,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteScope {
    mode: ScopeMode,
    device_id: String,
    host: String,
    port: u16,
    username: String,
    key: String,
    root: String,
    // Cloned transient grants share consumption. Deserialization never recreates one.
    #[serde(skip)]
    lease: Option<Arc<AtomicBool>>,
}
fn pin(c: &Connection) -> Result<String> {
    use russh::keys::ssh_key::{HashAlg, PublicKey};
    let saved = c
        .public_key
        .as_deref()
        .ok_or_else(|| err("Verify the SSH host key before authorizing directory access."))?;
    Ok(PublicKey::from_openssh(saved)
        .map_err(|_| err("Invalid saved SSH Host Key."))?
        .fingerprint(HashAlg::Sha256)
        .to_string())
}
impl RemoteScope {
    pub fn confirm(c: &Connection, s: &SyncSpec, mode: ScopeMode) -> Result<Self> {
        if mode == ScopeMode::JailedRoot && s.remote_root != "/" {
            return Err(err(
                "A persistent jail grant requires the server-confined SFTP root /.",
            ));
        }
        if !s.remote_root.starts_with('/')
            || s.remote_root.starts_with("//")
            || s.remote_root.split('/').any(|p| {
                p == ".." || p == "." || p.chars().any(char::is_control) || p.contains('\\')
            })
        {
            return Err(err(
                "Choose a literal absolute SFTP directory without dot components.",
            ));
        }
        Ok(Self {
            mode,
            device_id: c.id.clone(),
            host: c.host.to_ascii_lowercase(),
            port: c.port,
            username: c.username.clone(),
            key: pin(c)?,
            root: s.remote_root.clone(),
            lease: if mode == ScopeMode::ExclusiveRun {
                Some(Arc::new(AtomicBool::new(false)))
            } else {
                None
            },
        })
    }
    pub fn matches(&self, c: &Connection, s: &SyncSpec) -> bool {
        self.device_id == c.id
            && self.host.eq_ignore_ascii_case(&c.host)
            && self.port == c.port
            && self.username == c.username
            && self.root == s.remote_root
            && pin(c).is_ok_and(|key| key == self.key)
            && match self.mode {
                ScopeMode::JailedRoot => self.root == "/",
                ScopeMode::ExclusiveRun => self
                    .lease
                    .as_ref()
                    .is_some_and(|b| !b.load(Ordering::Acquire)),
            }
    }
    pub fn mode(&self) -> ScopeMode {
        self.mode
    }
    pub(super) fn claim(self, c: &Connection, s: &SyncSpec) -> Result<()> {
        if !self.matches(c, s) || self.lease.is_some_and(|b| b.swap(true, Ordering::AcqRel)) {
            return Err(err(
                "Remote directory authorization expired or belongs to a different device, account, Host Key or root. Confirm the scope again.",
            ));
        }
        Ok(())
    }
}
