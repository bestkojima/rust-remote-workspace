use super::{AppError, Config, ConfigManager, Result};
use crate::sync_engine::{Direction, RemoteScope, ScopeMode, SyncSpec};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SyncLog {
    pub at: DateTime<Utc>,
    pub outcome: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) enum CopyMode {
    #[default]
    ManualCopy,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) enum DeletePolicy {
    #[default]
    KeepTarget,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) enum ConflictPolicy {
    #[default]
    ReviewEach,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SyncProfile {
    pub id: String,
    pub name: String,
    pub device_id: String,
    #[serde(flatten)]
    pub spec: SyncSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_jail: Option<RemoteScope>,
    #[serde(default)]
    pub mode: CopyMode,
    #[serde(default)]
    pub deletion: DeletePolicy,
    #[serde(default)]
    pub conflicts: ConflictPolicy,
    #[serde(default)]
    pub history: Vec<SyncLog>,
}

impl SyncProfile {
    pub fn new(device_id: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            device_id,
            spec: SyncSpec {
                local_root: PathBuf::new(),
                remote_root: String::new(),
                direction: Direction::Upload,
                excludes: [".git", "build", "target", "out"]
                    .map(String::from)
                    .to_vec(),
            },
            mode: Default::default(),
            saved_jail: None,
            deletion: Default::default(),
            conflicts: Default::default(),
            history: Vec::new(),
        }
    }
}

impl ConfigManager {
    pub(crate) fn sync_profiles(&self) -> &[SyncProfile] {
        &self.config.sync_profiles
    }
    pub(crate) fn rclone_path(&self) -> &Path {
        self.config
            .settings
            .rclone_path
            .as_deref()
            .unwrap_or(Path::new("rclone"))
    }

    fn save_sync_config(&mut self, config: Config) -> Result<()> {
        self.persist(&config)?;
        self.config = config;
        Ok(())
    }
    pub(crate) fn set_rclone_path(&mut self, path: PathBuf) -> Result<()> {
        let mut c = self.config.clone();
        c.settings.rclone_path = if path.as_os_str().is_empty() {
            None
        } else {
            Some(path)
        };
        self.save_sync_config(c)
    }
    pub(crate) fn save_sync_profile(&mut self, mut p: SyncProfile) -> Result<usize> {
        if p.name.trim().is_empty()
            || self.find_connection(&p.device_id).is_none()
            || !p.spec.local_root.is_absolute()
            || !p.spec.remote_root.starts_with('/')
        {
            return Err(AppError::ValidationError("Enter a name, choose a saved device, and supply absolute local and remote directories.".into()));
        }
        if let Some(jail) = &p.saved_jail {
            let mut device = self
                .find_connection(&p.device_id)
                .cloned()
                .expect("device validated above");
            if jail.mode() != ScopeMode::JailedRoot
                || self.apply_host_pin(&mut device).is_err()
                || !jail.matches(&device, &p.spec)
            {
                p.saved_jail = None;
            }
        }
        let mut c = self.config.clone();
        let i = c
            .sync_profiles
            .iter()
            .position(|v| v.id == p.id)
            .unwrap_or(c.sync_profiles.len());
        if i == c.sync_profiles.len() {
            c.sync_profiles.push(p);
        } else {
            c.sync_profiles[i] = p;
        }
        self.save_sync_config(c)?;
        Ok(i)
    }
    pub(crate) fn delete_sync_profile(&mut self, id: &str) -> Result<()> {
        let mut c = self.config.clone();
        c.sync_profiles.retain(|v| v.id != id);
        self.save_sync_config(c)
    }
    pub(crate) fn log_sync_result(&mut self, id: &str, outcome: &str) -> Result<()> {
        let mut c = self.config.clone();
        let p = c
            .sync_profiles
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or_else(|| AppError::ConfigError("Sync profile no longer exists.".into()))?;
        if p.history.len() >= 12 {
            p.history.drain(..p.history.len() - 11);
        }
        let outcome = outcome
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(512)
            .collect();
        p.history.push(SyncLog {
            at: Utc::now(),
            outcome,
        });
        self.persist_metadata(&c)?;
        self.config = c;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::manager::{AuthMethod, Connection};

    #[test]
    fn profiles_keep_separate_bounded_histories_after_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("devices.toml");
        let mut cfg = ConfigManager::with_path(&file).unwrap();
        let dev = Connection::new("127.0.0.1".into(), 2222, "tester".into(), AuthMethod::None);
        cfg.add_connection(dev.clone()).unwrap();
        let mut a = SyncProfile::new(dev.id.clone());
        a.name = "A".into();
        a.spec.local_root = tmp.path().to_owned();
        a.spec.remote_root = "/tmp/a".into();
        cfg.save_sync_profile(a.clone()).unwrap();
        let mut b = a.clone();
        b.id = "second".into();
        b.name = "B".into();
        b.spec.direction = Direction::Download;
        cfg.save_sync_profile(b.clone()).unwrap();
        for i in 0..14 {
            cfg.log_sync_result(&a.id, &format!("run {i}")).unwrap();
        }
        cfg.log_sync_result(&b.id, "Failed: wrong directory")
            .unwrap();
        let mut reopened = ConfigManager::with_path(&file).unwrap();
        let profiles = reopened.sync_profiles();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].history.len(), 12);
        assert_eq!(profiles[0].history[0].outcome, "run 2");
        assert_eq!(profiles[1].history.len(), 1);
        assert_eq!(profiles[1].spec.direction, Direction::Download);
        assert!(reopened.remove_connection(&dev.id).is_err());
        reopened.delete_sync_profile(&a.id).unwrap();
        assert_eq!(reopened.sync_profiles()[0].id, b.id);
        assert!(reopened.find_connection(&dev.id).is_some());
    }
}
