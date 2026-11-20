use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::error::{AppError, Result};
#[path = "workspace_defaults.rs"]
mod workspace_defaults;
pub(crate) use workspace_defaults::LogPreference;
#[path = "tunnels.rs"]
mod tunnels;
pub use tunnels::TunnelRoute;
#[path = "sync_profiles.rs"]
mod sync_profiles;
pub(crate) use sync_profiles::SyncProfile;

pub const DEFAULT_TERMINAL_SCROLLBACK_LINES: usize = 2000;
pub const MAX_TERMINAL_SCROLLBACK_LINES: usize = 5000;

fn default_terminal_scrollback_lines() -> usize {
    DEFAULT_TERMINAL_SCROLLBACK_LINES
}

// A sibling created with directory defaults could expose a private config file.
// Apply its existing DACL at creation, before another process can open the temp file.
#[cfg(windows)]
pub(crate) fn replacement_file(
    parent: &Path,
    source: Option<&fs::File>,
) -> std::io::Result<tempfile::NamedTempFile> {
    use std::{
        io,
        mem::size_of,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle},
        },
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::{DACL_SECURITY_INFORMATION, GetKernelObjectSecurity, SECURITY_ATTRIBUTES},
        Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE},
    };
    let Some(source) = source else {
        return tempfile::NamedTempFile::new_in(parent);
    };
    let mut len = 0;
    // The first query reports the size of a self-relative SECURITY_DESCRIPTOR.
    unsafe {
        GetKernelObjectSecurity(
            source.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            null_mut(),
            0,
            &mut len,
        )
    };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut descriptor = vec![0usize; (len as usize).div_ceil(size_of::<usize>())];
    let sd = descriptor.as_mut_ptr().cast();
    // The handle is live; sd points to len writable, suitably aligned bytes.
    if unsafe {
        GetKernelObjectSecurity(
            source.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            sd,
            len,
            &mut len,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    tempfile::Builder::new().make_in(parent, |path| {
        let path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        // The descriptor and NUL-terminated name live through CreateFileW; CREATE_NEW
        // refuses collisions, and FILE_SHARE_DELETE permits the atomic rename.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_DELETE,
                &security,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { fs::File::from_raw_handle(handle) })
    })
}

/// Immutable secret with a cached protected envelope. Keep old upstream envelopes
/// only so background metadata writes can preserve them until an explicit save.
#[derive(Clone)]
pub struct SensitiveString {
    plaintext: String,
    sealed: std::sync::OnceLock<String>,
    legacy: Option<String>,
}

impl SensitiveString {
    pub fn new(plaintext: String) -> Self {
        Self {
            plaintext,
            sealed: Default::default(),
            legacy: None,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.plaintext
    }
}

impl std::ops::Deref for SensitiveString {
    type Target = str;
    fn deref(&self) -> &str {
        &self.plaintext
    }
}

impl From<String> for SensitiveString {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl AsRef<str> for SensitiveString {
    fn as_ref(&self) -> &str {
        &self.plaintext
    }
}

impl PartialEq for SensitiveString {
    fn eq(&self, other: &Self) -> bool {
        self.plaintext == other.plaintext
    }
}

impl fmt::Debug for SensitiveString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for SensitiveString {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let Some(sealed) = self.sealed.get() {
            return serializer.serialize_str(sealed);
        }
        let encrypted = crate::config::encryption::PasswordEncryption::new()
            .encrypt_password(&self.plaintext)
            .map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(self.sealed.get_or_init(|| encrypted))
    }
}

impl<'de> Deserialize<'de> for SensitiveString {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encrypted = String::deserialize(deserializer)?;
        let enc = crate::config::encryption::PasswordEncryption::new();
        let plaintext = enc
            .decrypt_password(&encrypted)
            .map_err(serde::de::Error::custom)?;
        let mut secret = Self::new(plaintext);
        if encrypted.contains(':') {
            secret.sealed = encrypted.into();
        } else {
            secret.legacy = Some(encrypted);
        }
        Ok(secret)
    }
}

/// Application settings
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppSettings {
    #[serde(default, skip_serializing_if = "LogPreference::is_off")]
    pub(crate) startup_log: LogPreference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rclone_path: Option<PathBuf>,
    pub default_port: u16,
    pub connection_timeout: u64,
    #[serde(default = "default_terminal_scrollback_lines")]
    pub terminal_scrollback_lines: usize,
    #[serde(default)]
    pub have_nerd_font: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            startup_log: LogPreference::Off,
            rclone_path: None,
            default_port: 22,
            connection_timeout: 20,
            terminal_scrollback_lines: DEFAULT_TERMINAL_SCROLLBACK_LINES,
            have_nerd_font: false,
        }
    }
}

/// Represents an SSH connection configuration
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Connection {
    pub id: String,
    pub display_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
    pub public_key: Option<String>,
}

/// Type of port forwarding
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default, Hash)]
pub enum PortForwardType {
    /// Local port forwarding (ssh -L)
    #[default]
    Local,
    /// Remote port forwarding (ssh -R)
    Remote,
    /// Dynamic SOCKS5 proxy (ssh -D)
    Dynamic,
}

/// Status of a port forwarding session
#[derive(Clone, Debug, PartialEq, Default, Hash)]
pub enum PortForwardStatus {
    #[default]
    Stopped,
    Running,
    Failed(String),
}

/// Represents a port forwarding configuration
#[derive(Serialize, Deserialize, Clone, Debug, Hash)]
pub struct PortForward {
    pub id: String,
    pub connection_id: String,
    #[serde(default)] // Default to Local for backward compatibility
    pub forward_type: PortForwardType,
    pub local_addr: String,
    pub local_port: u16,
    #[serde(default)]
    pub service_host: String,
    #[serde(default)]
    pub service_port: u16,
    #[serde(default)]
    pub remote_bind_addr: Option<String>,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip)] // Runtime status, not persisted
    pub status: PortForwardStatus,
}

impl PortForward {
    /// Creates a new port forward with the given parameters
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection_id: String,
        forward_type: PortForwardType,
        local_addr: String,
        local_port: u16,
        service_host: String,
        service_port: u16,
        remote_bind_addr: Option<String>,
        display_name: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            connection_id,
            forward_type,
            local_addr,
            local_port,
            service_host,
            service_port,
            remote_bind_addr,
            display_name,
            created_at: Utc::now(),
            status: PortForwardStatus::Stopped,
        }
    }

    /// Validates the port forward parameters
    pub fn validate(&self) -> Result<()> {
        if self.connection_id.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Connection ID cannot be empty".to_string(),
            ));
        }

        match self.forward_type {
            PortForwardType::Local => {
                // Local forwarding: local_addr:local_port -> service_host:service_port
                if self.local_addr.trim().is_empty() {
                    return Err(AppError::ValidationError(
                        "Local address cannot be empty".to_string(),
                    ));
                }
                if self.local_port == 0 {
                    return Err(AppError::ValidationError(
                        "Local port must be greater than 0".to_string(),
                    ));
                }
                if self.service_host.trim().is_empty() {
                    return Err(AppError::ValidationError(
                        "Service host cannot be empty".to_string(),
                    ));
                }
                if self.service_port == 0 {
                    return Err(AppError::ValidationError(
                        "Service port must be greater than 0".to_string(),
                    ));
                }
            }
            PortForwardType::Remote => {
                // Remote forwarding: remote_bind_addr:local_port -> service_host:service_port
                if self.local_port == 0 {
                    return Err(AppError::ValidationError(
                        "Remote port must be greater than 0".to_string(),
                    ));
                }
                if self.service_host.trim().is_empty() {
                    return Err(AppError::ValidationError(
                        "Local service host cannot be empty".to_string(),
                    ));
                }
                if self.service_port == 0 {
                    return Err(AppError::ValidationError(
                        "Local service port must be greater than 0".to_string(),
                    ));
                }
                // remote_bind_addr is optional, defaults to server's localhost
            }
            PortForwardType::Dynamic => {
                // Dynamic SOCKS5: only needs local_addr:local_port
                if self.local_addr.trim().is_empty() {
                    return Err(AppError::ValidationError(
                        "Local address cannot be empty".to_string(),
                    ));
                }
                if self.local_port == 0 {
                    return Err(AppError::ValidationError(
                        "Local port must be greater than 0".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Gets the display name or generates a default one
    pub fn get_display_name(&self) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| match self.forward_type {
                PortForwardType::Local => {
                    format!(
                        "{}:{} -> {}:{}",
                        self.local_addr, self.local_port, self.service_host, self.service_port
                    )
                }
                PortForwardType::Remote => {
                    let remote_bind = self.remote_bind_addr.as_deref().unwrap_or("127.0.0.1");
                    format!(
                        "{}:{} <- {}:{}",
                        remote_bind, self.local_port, self.service_host, self.service_port
                    )
                }
                PortForwardType::Dynamic => {
                    format!("SOCKS5 {}:{}", self.local_addr, self.local_port)
                }
            })
    }

    /// Gets the local address and port as a string
    pub fn local_address(&self) -> String {
        format!("{}:{}", self.local_addr, self.local_port)
    }

    /// Gets the service address and port as a string
    pub fn service_address(&self) -> String {
        format!("{}:{}", self.service_host, self.service_port)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum AuthMethod {
    #[serde(rename = "password")]
    Password(SensitiveString),
    #[serde(rename = "public_key")]
    PublicKey {
        private_key_path: String,
        #[serde(default)]
        passphrase: Option<SensitiveString>,
    },
    #[serde(rename = "auto_load_key")]
    AutoLoadKey,
    #[serde(rename = "none")]
    None,
}

impl Connection {
    /// Creates a new connection with the given parameters
    pub fn new(host: String, port: u16, username: String, auth_method: AuthMethod) -> Self {
        let display_name = host.clone(); // Default display name is the host
        Self {
            id: Uuid::new_v4().to_string(),
            display_name,
            host,
            port,
            username,
            auth_method,
            created_at: Utc::now(),
            last_used: None,
            public_key: None,
        }
    }

    pub fn host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    fn same_endpoint(&self, other: &Connection) -> bool {
        self.port == other.port && self.host.eq_ignore_ascii_case(&other.host)
    }

    /// Validates the connection parameters
    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Host cannot be empty".to_string(),
            ));
        }

        if self.port == 0 {
            return Err(AppError::ValidationError(
                "Port must be greater than 0".to_string(),
            ));
        }

        if self.username.trim().is_empty() {
            return Err(AppError::ValidationError(
                "Username cannot be empty".to_string(),
            ));
        }

        if let AuthMethod::Password(password) = &self.auth_method
            && password.trim().is_empty()
        {
            return Err(AppError::ValidationError(
                "Password cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    /// Updates the last used timestamp
    pub fn update_last_used(&mut self) {
        self.last_used = Some(Utc::now());
    }

    /// Sets a custom display name
    pub fn set_display_name(&mut self, name: String) {
        self.display_name = name;
    }
}

/// Main configuration structure
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub port_forwards: Vec<PortForward>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tunnels: Vec<TunnelRoute>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) sync_profiles: Vec<SyncProfile>,
    pub settings: AppSettings,
}

impl Config {
    fn secrets(&self) -> impl Iterator<Item = &SensitiveString> {
        self.connections
            .iter()
            .filter_map(|c| match &c.auth_method {
                AuthMethod::Password(s)
                | AuthMethod::PublicKey {
                    passphrase: Some(s),
                    ..
                } => Some(s),
                _ => None,
            })
    }

    #[cfg(unix)]
    fn protected_witness(&self) -> Option<&str> {
        self.secrets()
            .filter_map(|s| s.sealed.get())
            .map(String::as_str)
            .find(|s| s.contains(':'))
    }

    #[cfg(unix)]
    fn prepare_credentials(
        &self,
        disk_witness: Option<&str>,
    ) -> Result<Option<crate::config::encryption::StoreKey>> {
        if !self
            .secrets()
            .any(|s| s.sealed.get().is_none_or(|v| v.contains(':')))
        {
            return Ok(None); // No new or Unix-protected credentials: don't touch the key store.
        }
        let witness = disk_witness.or_else(|| self.protected_witness());
        let key = crate::config::encryption::StoreKey::open(witness.is_some())?;
        if let Some(existing) = witness {
            key.verify(existing)?;
        }
        for secret in self.secrets() {
            match secret.sealed.get() {
                Some(s) if s.contains(':') && Some(s.as_str()) != witness => key.verify(s)?,
                Some(_) => {} // Either validated above, or a legacy envelope kept for metadata.
                None => {
                    let sealed = key.envelope(secret.as_str())?;
                    if secret.sealed.set(sealed).is_err() {
                        key.verify(secret.sealed.get().expect("set by another serializer"))?;
                    }
                }
            }
        }
        Ok(Some(key))
    }

    fn insert_device(&mut self, connection: Connection) -> Result<()> {
        connection.validate()?;
        if self.connections.iter().any(|c| {
            c.id == connection.id
                || (c.host == connection.host
                    && c.port == connection.port
                    && c.username == connection.username
                    && c.display_name == connection.display_name)
        }) {
            return Err(AppError::ConfigError("Connection already exists".into()));
        }
        self.connections.push(connection);
        Ok(())
    }
}
impl AuthMethod {
    fn secret_mut(&mut self) -> Option<&mut SensitiveString> {
        match self {
            Self::Password(secret)
            | Self::PublicKey {
                passphrase: Some(secret),
                ..
            } => Some(secret),
            _ => None,
        }
    }
}

struct DiskImage {
    #[cfg(unix)]
    key_witness: Option<String>,
    raw: Option<String>,
    has_legacy_credentials: bool,
}
/// Configuration manager for handling application settings and connection storage
pub struct ConfigManager {
    config_path: PathBuf,
    config: Config,
    // Last bytes loaded/saved by this handle. Compare the actual file, not its
    // timestamp: two successful atomic replacements can share a timestamp.
    disk_image: std::sync::Mutex<DiskImage>,
}

impl ConfigManager {
    /// Create a new configuration manager
    pub fn new() -> Result<Self> {
        let config_path: PathBuf = Self::get_config_path()?;
        info!("Loading configuration from: {:?}", config_path);
        Self::with_path(config_path)
    }

    pub fn default_port(&self) -> u16 {
        self.config.settings.default_port
    }

    pub fn have_nerd_font(&self) -> bool {
        self.config.settings.have_nerd_font
    }

    /// Create a configuration manager with a custom config path (useful for testing)
    #[allow(dead_code)]
    pub fn with_path<P: AsRef<Path>>(config_path: P) -> Result<Self> {
        let config_path = config_path.as_ref().to_path_buf();
        let disk_image = Self::read_image(&config_path)?;
        let mut config: Config = match disk_image.as_deref() {
            None => Config::default(),
            Some(text) => toml::from_str(text).map_err(|e: toml::de::Error| {
                // TOML errors may quote complete secret fields, including ciphertext.
                let at = e.span().map(|s| format!(" at byte {}", s.start)).unwrap_or_default();
                AppError::ConfigError(format!("Cannot read device configuration{at}. Check the TOML and unlock saved credentials with the original account."))
            })?,
        };
        Self::normalize_settings(&mut config);
        let has_legacy_credentials = config.secrets().any(|s| s.legacy.is_some());
        #[cfg(unix)]
        let key_witness = config.protected_witness().map(str::to_owned);

        Ok(Self {
            config_path,
            config,
            disk_image: std::sync::Mutex::new(DiskImage {
                #[cfg(unix)]
                key_witness,
                raw: disk_image,
                has_legacy_credentials,
            }),
        })
    }

    /// Get the default configuration file path
    fn get_config_path() -> Result<PathBuf> {
        let home_dir = dirs::home_dir().ok_or_else(|| {
            AppError::ConfigError("Could not determine home directory".to_string())
        })?;
        let config_dir = home_dir.join(".config").join("termirs");

        // Create config directory if it doesn't exist
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).map_err(|e| {
                AppError::ConfigError(format!("Failed to create config directory: {e}"))
            })?;
        }

        Ok(config_dir.join("config.toml"))
    }

    fn read_image(path: &Path) -> Result<Option<String>> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AppError::ConfigError(format!(
                "Failed to read device configuration: {e}"
            ))),
        }
    }

    /// Persist current config to disk
    pub fn save(&self) -> Result<()> {
        self.persist(&self.config)
    }

    // A later timestamp/settings write must not truncate already accepted host keys.
    fn persist(&self, candidate: &Config) -> Result<()> {
        self.persist_config(candidate, true)
    }

    fn persist_metadata(&self, candidate: &Config) -> Result<()> {
        self.persist_config(candidate, false)
    }

    fn persist_config(&self, candidate: &Config, explicit: bool) -> Result<()> {
        // Follow an existing symlink to keep --config pointing at the same file.
        let path = match self.config_path.canonicalize() {
            Ok(path) => path,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.config_path.clone(),
            Err(e) => return Err(e.into()),
        };
        // The target inode is replaced on every save, so locking the target itself
        // cannot serialize writers. Leave the empty lock file in place across saves.
        let lock_path = path.with_added_extension("rrw-write-lock");
        let guard = match fs::File::open(&lock_path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)?,
            Err(e) => return Err(e.into()),
        };
        guard.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => AppError::ConfigError(
                "Another workspace is saving this configuration. Retry the change.".into(),
            ),
            std::fs::TryLockError::Error(e) => e.into(),
        })?;
        let mut observed = self.disk_image.lock().unwrap();
        if Self::read_image(&path)? != observed.raw {
            return Err(AppError::ConfigError("The configuration has changed on disk. Reopen the workspace before saving; no changes were written.".into()));
        }
        let existing = if observed.raw.is_some() {
            if path.metadata()?.permissions().readonly() {
                return Err(AppError::ConfigError(
                    "Device configuration is read-only.".into(),
                ));
            }
            Some(fs::OpenOptions::new().write(true).open(&path)?)
        } else {
            None
        };
        let mut metadata;
        let candidate = if !explicit && observed.has_legacy_credentials {
            metadata = candidate.clone();
            for secret in metadata
                .connections
                .iter_mut()
                .filter_map(|c| c.auth_method.secret_mut())
            {
                if let Some(legacy) = secret.legacy.take() {
                    secret.sealed = legacy.into();
                }
            }
            &metadata
        } else {
            candidate
        };
        #[cfg(unix)]
        let credential_key = candidate.prepare_credentials(observed.key_witness.as_deref())?;
        let encoded =
            toml::to_string_pretty(candidate).map_err(|e| AppError::ConfigError(e.to_string()))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        #[cfg(windows)]
        let mut tmp = replacement_file(parent, existing.as_ref())?;
        #[cfg(not(windows))]
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        drop(existing);
        tmp.write_all(encoded.as_bytes())?;
        tmp.as_file().sync_all()?;
        #[cfg(unix)]
        if let Some(key) = credential_key {
            key.verify_current()?;
        }
        tmp.persist(path).map_err(|e| {
            AppError::ConfigError(format!("Failed to save device configuration: {}", e.error))
        })?;
        #[cfg(unix)]
        {
            observed.key_witness = candidate.protected_witness().map(str::to_owned);
        }
        observed.raw = Some(encoded);
        if explicit {
            observed.has_legacy_credentials = false;
        }
        Ok(())
    }

    /// Return immutable slice of connections
    pub fn connections(&self) -> &[Connection] {
        &self.config.connections
    }

    pub fn connections_mut(&mut self) -> &mut Vec<Connection> {
        &mut self.config.connections
    }

    pub fn terminal_scrollback_lines(&self) -> usize {
        self.config.settings.terminal_scrollback_lines
    }

    /// Find a connection by ID
    pub fn find_connection(&self, id: &str) -> Option<&Connection> {
        self.config.connections.iter().find(|c| c.id == id)
    }

    /// Add a new connection and persist it
    pub fn add_connection(&mut self, connection: Connection) -> Result<()> {
        info!("Adding new connection: {}", connection.display_name);
        let mut candidate = self.config.clone();
        candidate.insert_device(connection)?;
        self.commit(candidate)
    }

    /// A host key identifies an SSH endpoint across accounts and display names.
    pub fn apply_host_pin(&self, connection: &mut Connection) -> Result<()> {
        connection.public_key = self.effective_host_pin(connection)?;
        Ok(())
    }

    fn effective_host_pin(&self, connection: &Connection) -> Result<Option<String>> {
        use russh::keys::ssh_key::PublicKey;
        #[derive(Deserialize)]
        struct Pin {
            host: String,
            port: u16,
            public_key: Option<String>,
        }
        #[derive(Deserialize)]
        struct Pins {
            connections: Vec<Pin>,
        }
        // Ordinary handshakes only read the file: a read-only config (and even
        // a read-only directory) is sufficient. The writer publishes atomically.
        let disk = Self::read_image(&self.config_path)?;
        let refreshed = if disk == self.disk_image.lock().unwrap().raw {
            None
        } else {
            let text = disk.as_deref().ok_or_else(|| {
                AppError::ConfigError(
                    "Device configuration was removed. Reopen the workspace before connecting."
                        .into(),
                )
            })?;
            Some(toml::from_str::<Pins>(text).map_err(|_| {
                AppError::ConfigError(
                    "Cannot read current host keys in device configuration.".into(),
                )
            })?)
        };
        let mut pinned = None;
        let mut encoded = None;
        for raw in connection
            .public_key
            .as_deref()
            .into_iter()
            .chain(
                self.connections()
                    .iter()
                    .filter(|c| c.same_endpoint(connection))
                    .filter_map(|c| c.public_key.as_deref()),
            )
            .chain(
                refreshed
                    .iter()
                    .flat_map(|pins| pins.connections.iter())
                    .filter(|p| {
                        p.port == connection.port && p.host.eq_ignore_ascii_case(&connection.host)
                    })
                    .filter_map(|p| p.public_key.as_deref()),
            )
        {
            let key = PublicKey::from_openssh(raw).map_err(|e| {
                AppError::ConfigError(format!(
                    "Invalid saved host key for {}: {e}",
                    connection.host_port()
                ))
            })?;
            if let Some(previous) = &pinned {
                if previous != key.key_data() {
                    return Err(AppError::ConfigError(format!(
                        "Conflicting host keys for {}. Connection blocked; verify the saved device keys.",
                        connection.host_port()
                    )));
                }
            } else {
                pinned = Some(key.key_data().clone());
                encoded = Some(raw.to_owned());
            }
        }
        Ok(encoded)
    }

    /// Recheck on receiving an unknown-key challenge and immediately before approval:
    /// another workspace may have pinned this endpoint since SSH started its handshake.
    pub(crate) fn host_key_is_trusted(
        &self,
        connection: &Connection,
        offered: &str,
    ) -> Result<bool> {
        use russh::keys::ssh_key::{HashAlg, PublicKey};
        let parse = |raw: &str| {
            PublicKey::from_openssh(raw).map_err(|_| {
                AppError::SshPublicKeyValidationError(format!(
                    "Invalid host key for {}. Connection blocked.",
                    connection.host_port()
                ))
            })
        };
        let key = parse(offered)?;
        let Some(saved) = self.effective_host_pin(connection)? else {
            return Ok(false);
        };
        let saved = parse(&saved)?;
        if saved.key_data() != key.key_data() {
            return Err(AppError::HostKeyChanged {
                endpoint: connection.host_port(),
                expected: saved.fingerprint(HashAlg::Sha256).to_string(),
                received: key.fingerprint(HashAlg::Sha256).to_string(),
            });
        }
        Ok(true)
    }

    /// Called only after explicit approval. Publish the trusted value in memory
    /// only after the whole candidate file has replaced the old one on disk.
    pub fn trust_host_key(&mut self, connection: &Connection, public_key: &str) -> Result<()> {
        let mut trusted = connection.clone();
        self.host_key_is_trusted(connection, public_key)?;
        trusted.public_key = Some(public_key.to_owned());
        let mut candidate = self.config.clone();
        if let Some(saved) = candidate
            .connections
            .iter_mut()
            .find(|c| c.id == connection.id)
        {
            if !saved.same_endpoint(connection) {
                return Err(AppError::ConfigError(
                    "Device address changed during verification; reconnect to verify its host key."
                        .into(),
                ));
            }
            saved.public_key = trusted.public_key;
        } else {
            candidate.insert_device(trusted)?;
        }

        self.commit(candidate)
    }

    /// Stage an edit for callers that save separately.
    pub fn update_connection(&mut self, connection: Connection) -> Result<()> {
        let (index, connection) = self.checked_edit(connection)?;
        self.config.connections[index] = connection;
        Ok(())
    }

    /// Device editor commit: a failed save never changes the live credentials.
    pub fn save_edited_connection(&mut self, connection: Connection) -> Result<()> {
        let (index, connection) = self.checked_edit(connection)?;
        let mut candidate = self.config.clone();
        candidate.connections[index] = connection;
        self.commit(candidate)
    }

    fn checked_edit(&self, mut connection: Connection) -> Result<(usize, Connection)> {
        // Validate the connection before updating
        connection.validate()?;

        info!("Updating connection: {}", connection.display_name);
        // Find and update the connection
        if let Some(index) = self
            .config
            .connections
            .iter()
            .position(|conn| conn.id == connection.id)
        {
            // The edit form can rename/move an existing device, not reset its identity.
            if self
                .tunnels()
                .iter()
                .any(|r| r.device_id == connection.id || r.gateway_id == connection.id)
                && !self.config.connections[index].same_endpoint(&connection)
            {
                return Err(AppError::ValidationError(
                    "Remove its tunnel route before changing this SSH endpoint.".into(),
                ));
            }
            connection.public_key = self.effective_host_pin(&self.config.connections[index])?;
            self.apply_host_pin(&mut connection)?;
            Ok((index, connection))
        } else {
            Err(AppError::ConfigError("Connection not found".to_string()))
        }
    }

    /// Stage removal for callers that save separately.
    pub fn remove_connection(&mut self, id: &str) -> Result<()> {
        self.validate_removal(id)?;
        self.config.connections.retain(|conn| conn.id != id);
        Ok(())
    }

    pub fn save_removed_connection(&mut self, id: &str) -> Result<()> {
        self.validate_removal(id)?;
        let mut candidate = self.config.clone();
        candidate.connections.retain(|conn| conn.id != id);
        self.commit(candidate)
    }

    fn commit(&mut self, candidate: Config) -> Result<()> {
        self.persist(&candidate)?;
        self.config = candidate;
        Ok(())
    }

    fn validate_removal(&self, id: &str) -> Result<()> {
        if self.sync_profiles().iter().any(|p| p.device_id == id) {
            return Err(AppError::ConfigError(
                "Remove or reassign its sync profiles before deleting this device.".into(),
            ));
        }
        if self
            .tunnels()
            .iter()
            .any(|r| r.gateway_id == id || r.device_id == id)
        {
            return Err(AppError::ConfigError(
                "Remove its VPS tunnel before deleting this SSH profile.".into(),
            ));
        }
        if self.find_connection(id).is_none() {
            Err(AppError::ConfigError("Connection not found".to_string()))
        } else {
            Ok(())
        }
    }

    /// Update last_used for a connection by id and persist
    pub fn touch_last_used(&mut self, id: &str) -> Result<()> {
        if let Some(i) = self.config.connections.iter().position(|c| c.id == id) {
            let mut candidate = self.config.clone();
            candidate.connections[i].update_last_used();
            self.persist_metadata(&candidate)?;
            self.config = candidate;
        }
        Ok(())
    }

    /// Return immutable slice of port forwards
    pub fn port_forwards(&self) -> &[PortForward] {
        &self.config.port_forwards
    }

    /// Return mutable slice of port forwards
    pub fn port_forwards_mut(&mut self) -> &mut Vec<PortForward> {
        &mut self.config.port_forwards
    }

    /// Add a new port forward and persist it
    pub fn add_port_forward(&mut self, port_forward: PortForward) -> Result<()> {
        // Validate the port forward before adding
        port_forward.validate()?;

        // Check for duplicates based on connection_id, local_addr, local_port, service_host, service_port
        if self.config.port_forwards.iter().any(|pf| {
            pf.connection_id == port_forward.connection_id
                && pf.local_addr == port_forward.local_addr
                && pf.local_port == port_forward.local_port
                && pf.service_host == port_forward.service_host
                && pf.service_port == port_forward.service_port
        }) {
            return Err(AppError::ConfigError(
                "Port forward already exists with the same configuration".to_string(),
            ));
        }

        let mut candidate = self.config.clone();
        candidate.port_forwards.push(port_forward);
        self.commit(candidate)
    }

    fn port_forward_index(&self, id: &str) -> Result<usize> {
        self.config
            .port_forwards
            .iter()
            .position(|pf| pf.id == id)
            .ok_or_else(|| AppError::ConfigError("Port forward not found".into()))
    }

    /// Stage an existing port forward for callers that save separately.
    pub fn update_port_forward(&mut self, port_forward: PortForward) -> Result<()> {
        port_forward.validate()?;
        let i = self.port_forward_index(&port_forward.id)?;
        self.config.port_forwards[i] = port_forward;
        Ok(())
    }

    pub fn save_edited_port_forward(&mut self, port_forward: PortForward) -> Result<()> {
        port_forward.validate()?;
        let i = self.port_forward_index(&port_forward.id)?;
        let mut candidate = self.config.clone();
        candidate.port_forwards[i] = port_forward;
        self.commit(candidate)
    }

    /// Stage removal for callers that save separately.
    pub fn remove_port_forward(&mut self, id: &str) -> Result<()> {
        self.port_forward_index(id)?;
        self.config.port_forwards.retain(|pf| pf.id != id);
        Ok(())
    }

    pub fn save_removed_port_forward(&mut self, id: &str) -> Result<()> {
        self.port_forward_index(id)?;
        let mut candidate = self.config.clone();
        candidate.port_forwards.retain(|pf| pf.id != id);
        self.commit(candidate)
    }

    /// Find a port forward by ID (mutable)
    pub fn find_port_forward_mut(&mut self, id: &str) -> Option<&mut PortForward> {
        self.config.port_forwards.iter_mut().find(|pf| pf.id == id)
    }

    fn normalize_settings(config: &mut Config) {
        if config.settings.terminal_scrollback_lines == 0 {
            config.settings.terminal_scrollback_lines = DEFAULT_TERMINAL_SCROLLBACK_LINES;
        }
        if config.settings.terminal_scrollback_lines > MAX_TERMINAL_SCROLLBACK_LINES {
            config.settings.terminal_scrollback_lines = MAX_TERMINAL_SCROLLBACK_LINES;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(user: &str) -> Connection {
        Connection::new(
            "localhost".into(),
            2222,
            user.into(),
            AuthMethod::AutoLoadKey,
        )
    }

    fn host_key(n: u8) -> String {
        use russh::keys::ssh_key::{PublicKey, public::Ed25519PublicKey};
        PublicKey::new(Ed25519PublicKey([n; 32]).into(), "")
            .to_openssh()
            .unwrap()
    }

    #[test]
    fn a_stale_writer_keeps_the_other_workspaces_pin_and_checks_pending_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let dev = device("alice");
        let mut a = ConfigManager::with_path(&path).unwrap();
        a.add_connection(dev.clone()).unwrap();
        let mut b = ConfigManager::with_path(&path).unwrap();
        let trusted = host_key(7);
        a.trust_host_key(&dev, &trusted).unwrap();
        let accepted_file = fs::read(&path).unwrap();
        let mut alias = device("another-user");
        b.apply_host_pin(&mut alias).unwrap();
        assert_eq!(alias.public_key.as_deref(), Some(trusted.as_str()));
        assert!(b.host_key_is_trusted(&dev, &trusted).unwrap());
        assert!(matches!(
            b.trust_host_key(&dev, &host_key(8)),
            Err(AppError::HostKeyChanged { .. })
        ));
        assert!(b.set_rclone_path("updated-tool".into()).is_err());
        assert_eq!(fs::read(&path).unwrap(), accepted_file);
        assert!(
            ConfigManager::with_path(&path)
                .unwrap()
                .host_key_is_trusted(&dev, &trusted)
                .unwrap()
        );
    }

    #[test]
    fn writers_serialize_without_locking_out_readers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let mut cfg = ConfigManager::with_path(&path).unwrap();
        let dev = device("alice");
        let pin = host_key(5);
        cfg.trust_host_key(&dev, &pin).unwrap();
        let lock = fs::File::open(path.with_added_extension("rrw-write-lock")).unwrap();
        lock.lock().unwrap();
        let mut another = ConfigManager::with_path(&path).unwrap();
        assert!(another.host_key_is_trusted(&dev, &pin).unwrap());
        assert!(another.set_rclone_path("other-tool".into()).is_err());
        drop(lock);
        another.set_rclone_path("other-tool".into()).unwrap();
    }

    #[test]
    fn trust_is_shared_by_endpoint_and_cannot_replace_a_pin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let mut config = ConfigManager::with_path(&path).unwrap();
        let a = device("alice");
        let key = host_key(1);
        config.trust_host_key(&a, &key).unwrap();
        let saved = fs::read(&path).unwrap();
        let mut config = ConfigManager::with_path(&path).unwrap();
        let mut b = device("bob");
        b.host = "LOCALHOST".into();
        config.apply_host_pin(&mut b).unwrap();
        assert_eq!(b.public_key.as_ref(), Some(&key));
        b.public_key = None;
        assert!(config.trust_host_key(&b, &host_key(2)).is_err());
        assert!(config.trust_host_key(&a, &host_key(2)).is_err());
        let mut stale = a.clone();
        stale.port += 1;
        assert!(config.trust_host_key(&stale, &key).is_err());
        assert_eq!(fs::read(&path).unwrap(), saved);
        b.port += 1;
        config.apply_host_pin(&mut b).unwrap();
        assert!(b.public_key.is_none());
    }

    #[test]
    fn edited_device_keeps_its_pin_when_fields_or_address_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let mut config = ConfigManager::with_path(&path).unwrap();
        let mut edited = device("one");
        let key = host_key(3);
        config.add_connection(edited.clone()).unwrap();
        config.trust_host_key(&edited, &key).unwrap();
        edited.display_name = "renamed".into();
        edited.host = "alias.local".into();
        edited.username = "two".into();
        config.update_connection(edited.clone()).unwrap();
        config.save().unwrap();
        let config = ConfigManager::with_path(path).unwrap();
        let actual = config.find_connection(&edited.id).unwrap();
        assert_eq!(actual.host, "alias.local");
        assert_eq!(actual.public_key.as_ref(), Some(&key));
    }

    #[test]
    fn failed_pin_write_leaves_memory_and_disk_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let a = device("alice");
        let mut config = ConfigManager::with_path(&path).unwrap();
        config.add_connection(a.clone()).unwrap();
        let before = fs::read(&path).unwrap();
        let permissions = path.metadata().unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        let result = config.trust_host_key(&a, &host_key(3));
        fs::set_permissions(&path, permissions).unwrap();
        assert!(result.is_err());
        assert!(config.find_connection(&a.id).unwrap().public_key.is_none());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn saving_host_trust_preserves_private_windows_file_acl() {
        use std::process::Command;
        fn acl(path: &Path, setup: &str) -> String {
            let script = format!(
                "$ErrorActionPreference='Stop'; $p=$env:TEST_CONFIG; {setup}; \
                $acl=[IO.File]::GetAccessControl($p); \
                if (!$acl.AreAccessRulesProtected) {{ throw 'The file inherited permissions' }}; \
                $dacl=[Security.AccessControl.RawSecurityDescriptor]::new($acl.GetSecurityDescriptorBinaryForm(),0).DiscretionaryAcl; \
                $bytes=[byte[]]::new($dacl.BinaryLength); $dacl.GetBinaryForm($bytes,0); \
                [Convert]::ToBase64String($bytes)"
            );
            let out = Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .env("TEST_CONFIG", path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().into()
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let a = device("alice");
        let mut config = ConfigManager::with_path(&path).unwrap();
        config.add_connection(a.clone()).unwrap();
        let expected = acl(&path, "
            $acl=[IO.File]::GetAccessControl($p)
            $acl.SetAccessRuleProtection($true,$false)
            $me=[Security.Principal.WindowsIdentity]::GetCurrent().User
            $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($me,'FullControl','Allow'))
            [IO.File]::SetAccessControl($p,$acl)
        ");
        assert!(!expected.is_empty());
        config.trust_host_key(&a, &host_key(5)).unwrap();
        assert_eq!(acl(&path, ""), expected);
    }

    #[test]
    fn failed_ui_mutations_are_never_published_and_port_forward_edits_survive_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devices.toml");
        let mut manager = ConfigManager::with_path(&path).unwrap();
        let dev = device("fixture");
        manager.add_connection(dev.clone()).unwrap();
        let forward = PortForward::new(
            dev.id.clone(),
            PortForwardType::Local,
            "127.0.0.1".into(),
            40123,
            "localhost".into(),
            80,
            None,
            Some("old".into()),
        );
        manager.add_port_forward(forward.clone()).unwrap();
        let mut edited = forward.clone();
        edited.display_name = Some("new".into());
        let mut extra = forward.clone();
        extra.local_port += 1;
        extra.id = Uuid::new_v4().to_string();
        let lock = fs::File::open(path.with_added_extension("rrw-write-lock")).unwrap();
        lock.lock().unwrap();
        assert!(manager.add_port_forward(extra).is_err());
        assert!(manager.save_edited_port_forward(edited.clone()).is_err());
        assert!(manager.save_removed_port_forward(&forward.id).is_err());
        assert!(manager.save_removed_connection(&dev.id).is_err());
        drop(lock);
        assert_eq!(manager.port_forwards().len(), 1);
        assert_eq!(
            manager.port_forwards()[0].display_name.as_deref(),
            Some("old")
        );
        assert!(manager.find_connection(&dev.id).is_some());
        manager.touch_last_used(&dev.id).unwrap();
        let reopened = ConfigManager::with_path(&path).unwrap();
        assert_eq!(reopened.port_forwards().len(), 1);
        assert_eq!(
            reopened.port_forwards()[0].display_name.as_deref(),
            Some("old")
        );
        assert!(reopened.find_connection(&dev.id).is_some());
        manager.save_edited_port_forward(edited).unwrap();
        assert_eq!(
            ConfigManager::with_path(&path).unwrap().port_forwards()[0]
                .display_name
                .as_deref(),
            Some("new")
        );
        manager.save_removed_port_forward(&forward.id).unwrap();
        manager.save_removed_connection(&dev.id).unwrap();
        let reopened = ConfigManager::with_path(&path).unwrap();
        assert!(reopened.port_forwards().is_empty() && reopened.find_connection(&dev.id).is_none());
    }

    #[test]
    fn explicit_save_migrates_both_legacy_secrets_but_metadata_never_does() {
        use crate::config::encryption::PasswordEncryption;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devices.toml");
        let enc = PasswordEncryption::new();
        let old_password = enc.encrypt_legacy(" fixture password ").unwrap();
        let old_passphrase = enc.encrypt_legacy("fixture passphrase").unwrap();
        let mut a = device("fixture-a");
        let mut b = device("fixture-b");
        // Construct synthetic upstream-format records directly; no legacy writer in production.
        let legacy = |value: &str| SensitiveString {
            plaintext: String::new(),
            sealed: value.to_owned().into(),
            legacy: None,
        };
        a.auth_method = AuthMethod::Password(legacy(&old_password));
        b.auth_method = AuthMethod::PublicKey {
            private_key_path: "/fixture/not-used".into(),
            passphrase: Some(legacy(&old_passphrase)),
        };
        let seed = Config {
            connections: vec![a.clone(), b],
            ..Default::default()
        };
        let original = toml::to_string(&seed).unwrap();
        fs::write(&path, &original).unwrap();

        let mut manager = ConfigManager::with_path(&path).unwrap();
        assert!(fs::read_to_string(&path).unwrap() == original);
        let AuthMethod::Password(ref old) = manager.connections()[0].auth_method else {
            panic!("missing fixture password")
        };
        assert!(old.as_str() == " fixture password " && format!("{old:?}") == "[REDACTED]");
        manager.touch_last_used(&a.id).unwrap();
        let metadata = fs::read_to_string(&path).unwrap();
        assert!(
            metadata != original
                && metadata.contains(&old_password)
                && metadata.contains(&old_passphrase)
        );

        let lock = fs::File::open(path.with_added_extension("rrw-write-lock")).unwrap();
        lock.lock().unwrap();
        let mut edit = manager.connections()[0].clone();
        edit.auth_method = AuthMethod::Password("failed edit".to_owned().into());
        assert!(manager.save_edited_connection(edit).is_err());
        assert!(fs::read_to_string(&path).unwrap() == metadata);
        let AuthMethod::Password(ref old) = manager.connections()[0].auth_method else {
            panic!("missing original credential")
        };
        assert!(old.as_str() == " fixture password ");
        drop(lock);

        manager.save().unwrap();
        let migrated = fs::read_to_string(&path).unwrap();
        let marker = if cfg!(windows) {
            "dpapi-v1:"
        } else {
            "unix-key-v1:"
        };
        assert_eq!(migrated.matches(marker).count(), 2);
        assert!(
            !migrated.contains(&old_password)
                && !migrated.contains(&old_passphrase)
                && !migrated.contains("fixture password")
                && !migrated.contains("fixture passphrase")
        );
        manager.touch_last_used(&a.id).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        let mut loaded = ConfigManager::with_path(&path).unwrap();
        for (conn, expected) in loaded
            .config
            .connections
            .iter_mut()
            .zip([" fixture password ", "fixture passphrase"])
        {
            let secret = conn.auth_method.secret_mut().unwrap();
            assert!(secret.as_str() == expected && format!("{secret:?}") == "[REDACTED]");
            let sealed = secret.sealed.get().unwrap();
            // Last-used writes after migration must keep the exact new ciphertext.
            assert!(after.contains(sealed) && migrated.contains(sealed));
        }
    }

    #[test]
    fn unreadable_credential_leaves_original_bytes_and_redacts_parse_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devices.toml");
        let bad = "invalid:opaque-fixture-value";
        let mut dev = device("fixture");
        dev.auth_method = AuthMethod::Password(SensitiveString {
            plaintext: String::new(),
            sealed: bad.to_owned().into(),
            legacy: None,
        });
        let config = Config {
            connections: vec![dev],
            ..Default::default()
        };
        let input = toml::to_string_pretty(&config).unwrap();
        fs::write(&path, &input).unwrap();
        let Err(error) = ConfigManager::with_path(&path) else {
            panic!("unreadable credentials must block opening");
        };
        assert!(!error.to_string().contains(bad));
        assert!(
            fs::read_to_string(&path).unwrap() == input
                && !path.with_added_extension("rrw-write-lock").exists()
        );
    }

    #[test]
    fn test_serialize_deserialize_connection_public_key() {
        let conn = Connection::new(
            "test".to_string(),
            22,
            "root".to_string(),
            AuthMethod::PublicKey {
                private_key_path: "path".to_string(),
                passphrase: Some("passphrase".to_string().into()),
            },
        );
        let serialized = toml::to_string(&conn).unwrap();

        let deserialized: Connection = toml::from_str(&serialized).unwrap();
        assert_eq!(conn.auth_method, deserialized.auth_method);
    }

    #[test]
    fn test_serialize_deserialize_connection_password() {
        let conn = Connection::new(
            "test".to_string(),
            22,
            "root".to_string(),
            AuthMethod::Password("password".to_string().into()),
        );
        let serialized = toml::to_string(&conn).unwrap();

        let deserialized: Connection = toml::from_str(&serialized).unwrap();
        assert_eq!(conn.auth_method, deserialized.auth_method);
    }

    #[test]
    fn test_serialize_config() {
        let conn = Connection::new(
            "test".to_string(),
            22,
            "root".to_string(),
            AuthMethod::PublicKey {
                private_key_path: "path".to_string(),
                passphrase: None,
            },
        );
        let conn1 = Connection::new(
            "test1".to_string(),
            23,
            "root".to_string(),
            AuthMethod::Password("password".to_string().into()),
        );
        let config = Config {
            connections: vec![conn, conn1],
            port_forwards: vec![],
            tunnels: vec![],
            sync_profiles: vec![],
            settings: AppSettings::default(),
        };
        let encoded = toml::to_string(&config).unwrap();
        assert!(toml::from_str::<Config>(&encoded).is_ok());
    }
}
