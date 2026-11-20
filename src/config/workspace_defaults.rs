use super::{AppError, ConfigManager, MAX_TERMINAL_SCROLLBACK_LINES, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogPreference {
    #[default]
    Off,
    Info,
    Debug,
}

impl LogPreference {
    pub(super) fn is_off(&self) -> bool {
        *self == Self::Off
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
    pub(crate) fn next(self, backwards: bool) -> Self {
        match (self, backwards) {
            (Self::Off, false) | (Self::Debug, true) => Self::Info,
            (Self::Info, false) | (Self::Off, true) => Self::Debug,
            _ => Self::Off,
        }
    }
}

impl ConfigManager {
    pub(crate) fn log_preference(&self) -> LogPreference {
        self.config.settings.startup_log
    }
    /// Saved logging preference. An explicit CLI enable/level takes priority.
    pub fn startup_log_level(&self) -> Option<&'static str> {
        let log = self.log_preference();
        (!log.is_off()).then(|| log.label())
    }

    pub(crate) fn save_defaults(
        &mut self,
        port: u16,
        history: usize,
        rclone: &str,
        log: LogPreference,
    ) -> Result<()> {
        if port == 0 {
            return Err(AppError::ValidationError(
                "SSH port must be between 1 and 65535.".into(),
            ));
        }
        if !(1..=MAX_TERMINAL_SCROLLBACK_LINES).contains(&history) {
            return Err(AppError::ValidationError(format!(
                "Terminal history must be between 1 and {MAX_TERMINAL_SCROLLBACK_LINES} lines."
            )));
        }
        let mut candidate = self.config.clone();
        let s = &mut candidate.settings;
        s.default_port = port;
        s.terminal_scrollback_lines = history;
        s.startup_log = log;
        s.rclone_path = match rclone.trim() {
            "" | "rclone" => None,
            path => Some(PathBuf::from(path)),
        };
        self.persist(&candidate)?;
        self.config = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_defaults_preserve_old_config_and_validate_before_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let old = "connections=[]\n[settings]\ndefault_port=2202\nconnection_timeout=15\nhave_nerd_font=true\n";
        std::fs::write(&path, old).unwrap();
        let mut cfg = ConfigManager::with_path(&path).unwrap();
        assert_eq!(cfg.startup_log_level(), None);
        assert!(
            cfg.save_defaults(2204, 5001, "tool", LogPreference::Debug)
                .is_err()
        );
        assert!(
            cfg.save_defaults(0, 4000, "tool", LogPreference::Info)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        cfg.save_defaults(2204, 5000, "", LogPreference::Debug)
            .unwrap();
        let reloaded = ConfigManager::with_path(&path).unwrap();
        assert_eq!(reloaded.default_port(), 2204);
        assert_eq!(reloaded.terminal_scrollback_lines(), 5000);
        assert_eq!(reloaded.startup_log_level(), Some("debug"));
        assert!(reloaded.have_nerd_font());
        assert_eq!(reloaded.config.settings.connection_timeout, 15);
        assert_eq!(reloaded.rclone_path(), std::path::Path::new("rclone"));
    }
}
