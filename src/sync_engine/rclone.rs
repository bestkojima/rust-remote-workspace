use super::fs::err;
use super::private_dir::PrivateDir;
use super::*;
use crate::config::manager::AuthMethod;
use std::{ffi::OsString, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

pub(super) struct Rclone {
    pub temp: PrivateDir,
    path: PathBuf,
    env: Vec<(String, OsString)>,
}

fn clean_cmd(path: &Path) -> Command {
    let mut cmd = Command::new(path);
    // Inherited RCLONE_* can silently change a filter, turn on symlink following,
    // change credentials, or redirect log output. Build each job's config alone.
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("RCLONE_")
        {
            cmd.env_remove(k);
        }
    }
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    cmd
}
async fn obscure(path: &Path, secret: &str, c: &CancellationToken) -> Result<OsString> {
    if secret.is_empty() || secret.contains(['\r', '\n']) {
        return Err(err("Unsupported empty or multiline SFTP credential."));
    }
    let mut cmd = clean_cmd(path);
    let mut child = cmd
        .args(["obscure", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().unwrap();
    input.write_all(secret.as_bytes()).await?;
    input.shutdown().await?;
    drop(input);
    let mut output = Vec::new();
    let mut out = child.stdout.take().unwrap();
    let result = tokio::select! { _=c.cancelled()=>{child.kill().await?;return Err(err("Cancelled"));}, result=out.read_to_end(&mut output)=>result? };
    if !child.wait().await?.success() || result == 0 {
        return Err(err("Cannot prepare SFTP credential."));
    }
    Ok(String::from_utf8(output)
        .map_err(|_| err("Invalid credential encoding."))?
        .trim_end()
        .into())
}
impl Rclone {
    pub async fn new(path: &Path, dev: &Connection, c: &CancellationToken) -> Result<Self> {
        let version = clean_cmd(path)
            .arg("version")
            .output()
            .await
            .map_err(|e| err(format!("Cannot start rclone 1.72.1: {e}")))?;
        if !version.status.success()
            || String::from_utf8_lossy(&version.stdout).lines().next() != Some("rclone v1.72.1")
        {
            return Err(err("Sync requires rclone v1.72.1."));
        }
        let temp = PrivateDir::new_in(&std::env::temp_dir())?;
        std::fs::write(temp.path().join("config"), b"")?;
        let known = temp.path().join("known_hosts");
        let pk = dev
            .public_key
            .as_deref()
            .ok_or_else(|| err("Verify the device Host Key before using Sync."))?;
        let key = russh::keys::ssh_key::PublicKey::from_openssh(pk)
            .map_err(|_| err("Invalid device Host Key."))?;
        if dev.host.is_empty()
            || dev
                .host
                .chars()
                .any(|ch| ch.is_whitespace() || ",*!?|[]".contains(ch))
        {
            return Err(err("Invalid literal hostname for SFTP known_hosts."));
        }
        let host = if dev.port == 22 {
            dev.host.clone()
        } else {
            format!("[{}]:{}", dev.host, dev.port)
        };
        std::fs::write(
            &known,
            format!(
                "{host} {}\n",
                key.to_openssh()
                    .map_err(|_| err("Invalid device Host Key."))?
            ),
        )?;
        let mut env = vec![
            ("TYPE".into(), "sftp".into()),
            ("HOST".into(), dev.host.clone().into()),
            ("PORT".into(), dev.port.to_string().into()),
            ("USER".into(), dev.username.clone().into()),
            ("KNOWN_HOSTS_FILE".into(), known.into_os_string()),
            ("SHELL_TYPE".into(), "none".into()),
            ("DISABLE_HASHCHECK".into(), "true".into()),
            ("SKIP_LINKS".into(), "true".into()),
        ];
        match &dev.auth_method {
            AuthMethod::PublicKey {
                private_key_path,
                passphrase,
            } => {
                env.push((
                    "KEY_FILE".into(),
                    crate::expand_tilde(private_key_path).into_os_string(),
                ));
                if let Some(pass) = passphrase {
                    env.push((
                        "KEY_FILE_PASS".into(),
                        obscure(path, pass.as_str(), c).await?,
                    ));
                }
            }
            AuthMethod::Password(pass) => {
                env.push(("PASS".into(), obscure(path, pass.as_str(), c).await?))
            }
            AuthMethod::AutoLoadKey => {
                // The explicit key is also used by rclone's in-process SFTP.
                // An external ssh grandchild would escape job cancellation.
                use crate::async_ssh_client::{STANDARD_KEY_PATHS, SshSession};
                let mut chosen = None;
                for name in STANDARD_KEY_PATHS {
                    let key = crate::expand_tilde(name);
                    if !key.is_file() || russh::keys::load_secret_key(&key, None).is_err() {
                        continue;
                    }
                    let mut candidate = dev.clone();
                    candidate.auth_method = AuthMethod::PublicKey {
                        private_key_path: key.to_string_lossy().into_owned(),
                        passphrase: None,
                    };
                    match SshSession::new_session_with_timeout(
                        &candidate,
                        Some(std::time::Duration::from_secs(10)),
                        c,
                    )
                    .await
                    {
                        Ok(handle) => {
                            drop(handle);
                            chosen = Some(key);
                            break;
                        }
                        Err(AppError::AuthenticationError(_)) => continue,
                        Err(e) => return Err(e),
                    }
                }
                let key = chosen.ok_or_else(|| {
                    err("No accepted standard SSH key. Select a private key in Devices.")
                })?;
                env.push(("KEY_FILE".into(), key.into_os_string()));
            }
            AuthMethod::None => {
                return Err(err(
                    "Select a device key or password for Sync. An unauthenticated profile must not fall back to your SSH agent.",
                ));
            }
        }
        Ok(Self {
            temp,
            path: path.into(),
            env,
        })
    }
    pub async fn copy(
        &self,
        src: &str,
        dst: &str,
        files: &[&str],
        dry: bool,
        c: &CancellationToken,
        progress: Option<(&mpsc::Sender<SyncProgress>, SyncProgress)>,
    ) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let list = self.temp.path().join("manifest");
        let mut content = files.join("\n");
        content.push('\n');
        std::fs::write(&list, content)?;
        let mut cmd = clean_cmd(&self.path);
        for (k, v) in &self.env {
            cmd.env(format!("RCLONE_CONFIG_DEVICE_{k}"), v);
        }
        cmd.arg("--config")
            .arg(self.temp.path().join("config"))
            .args(["copy", "--files-from-raw"])
            .arg(list)
            .args([
                "--ignore-times",
                "--inplace",
                "--no-traverse",
                "--transfers",
                "1",
                "--checkers",
                "1",
                "--retries",
                "1",
                "--low-level-retries",
                "1",
                "--contimeout",
                "10s",
                "--timeout",
                "20s",
                "--use-json-log",
                "--stats",
                "500ms",
                "--stats-log-level",
                "NOTICE",
                "--log-level",
                "NOTICE",
            ]);
        if dry {
            cmd.arg("--dry-run");
        }
        cmd.arg("--").arg(src).arg(dst).stdout(Stdio::null());
        let mut p = cmd.stderr(Stdio::piped()).spawn()?;
        let mut lines = BufReader::new(p.stderr.take().unwrap()).lines();
        let stats = async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some((tx, base)) = &progress
                    && let Some(bytes) = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|v| v.get("stats")?.get("bytes")?.as_u64())
                {
                    let mut update = base.clone();
                    update.bytes += bytes;
                    let _ = tx.try_send(update);
                }
            }
        };
        let status = tokio::select! { biased; _=c.cancelled()=>{p.kill().await?;let _=p.wait().await;return Err(err("Cancelled; no further files published."));}, x=async{let (s,_)=tokio::join!(p.wait(),stats);s}=>x? };
        if !status.success() {
            return Err(err(format!(
                "rclone {} failed (exit {}). Check the pinned SSH endpoint, credentials and selected directories; preview again.",
                if dry { "dry run" } else { "copy" },
                status.code().unwrap_or(-1)
            )));
        }
        Ok(())
    }
}
