//! Supervise the official rathole client on the development device.
//! Process state alone does not tell us whether the home-side SSH route works.
use std::{
    fs::File,
    future::Future,
    io::{self, Read},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{process::Command, time::Instant};

#[cfg(windows)]
mod windows_acl;

#[derive(Clone, Copy, Debug)]
pub enum AgentState {
    Running { pid: u32 },
    Retrying { exit: ExitStatus, delay: Duration },
    Stopped,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn private_config(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(invalid("Use a regular rathole client configuration file."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // geteuid takes no arguments and has no failure return.
        if meta.mode() & 0o077 != 0 || (meta.uid() != unsafe { libc::geteuid() } && meta.uid() != 0)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Use a private client configuration owned by this account or root (chmod 600).",
            ));
        }
    }
    #[cfg(windows)]
    windows_acl::check(&file)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    validate_client(&contents)
}

fn validate_client(contents: &str) -> io::Result<()> {
    // Upstream syntax errors can echo the line containing a token. Never forward that text.
    let doc: toml::Table = toml::from_str(contents)
        .map_err(|_| invalid("Invalid rathole TOML; check the private configuration."))?;
    let client = doc
        .get("client")
        .and_then(|v| v.as_table())
        .ok_or_else(|| invalid("Missing [client] in the rathole configuration."))?;
    let remote = client
        .get("remote_addr")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid("Set client.remote_addr to the authorized VPS control endpoint."))?;
    let local_control = remote
        .parse::<SocketAddr>()
        .is_ok_and(|s| s.ip().is_loopback());
    let transport = client.get("transport");
    let kind = transport
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("tcp");
    if !local_control {
        let pinned_noise = transport.and_then(|v| v.get("noise")).is_some_and(|noise| {
            noise
                .get("remote_public_key")
                .and_then(|v| v.as_str())
                .is_some_and(|k| !k.trim().is_empty())
                && noise
                    .get("pattern")
                    .is_none_or(|v| v.as_str() == Some("Noise_NK_25519_ChaChaPoly_BLAKE2s"))
        });
        if kind != "tls" && !(kind == "noise" && pinned_noise) {
            return Err(invalid(
                "For a non-loopback VPS, configure TLS or Noise NK with its verified remote_public_key.",
            ));
        }
    }
    let services = client
        .get("services")
        .and_then(|v| v.as_table())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("Configure the local SSH service under [client.services.NAME]."))?;
    for service in services.values() {
        let endpoint = service
            .get("local_addr")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<SocketAddr>().ok());
        if !endpoint.is_some_and(|s| s.ip().is_loopback() && s.port() != 0)
            || service
                .get("type")
                .is_some_and(|v| v.as_str() != Some("tcp"))
        {
            return Err(invalid(
                "Each developer SSH service must use a numeric loopback TCP endpoint.",
            ));
        }
        let token = service
            .get("token")
            .or_else(|| client.get("default_token"))
            .and_then(|v| v.as_str());
        if !token.is_some_and(|t| !t.trim().is_empty()) {
            return Err(invalid(
                "Provision a private rathole token for each service.",
            ));
        }
    }
    Ok(())
}

/// The stop future resolves on the caller's shutdown signal. On return the child
/// has exited and been reaped. Each relaunch rechecks the administrator's file.
pub async fn supervise(
    exe: &Path,
    config: &Path,
    stop: impl Future<Output = io::Result<()>>,
    mut status: impl FnMut(AgentState),
) -> io::Result<()> {
    let config = config.canonicalize()?;
    // Resolve path-like arguments before setting the child's working directory.
    // A bare executable name uses the administrator's installed PATH.
    let exe = if exe.components().count() > 1 {
        exe.canonicalize()?
    } else {
        PathBuf::from(exe)
    };
    let mut delay = Duration::from_secs(1);
    tokio::pin!(stop);
    loop {
        private_config(&config)?;
        let mut child = Command::new(&exe)
            .arg("--client")
            .arg(&config)
            .current_dir(
                config
                    .parent()
                    .ok_or_else(|| invalid("Configuration must name a file."))?,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("Could not launch rathole: {e}")))?;
        status(AgentState::Running {
            pid: child.id().expect("freshly spawned child"),
        });
        let started = Instant::now();
        let exit = tokio::select! {
            biased;
            signal = &mut stop => {
                child.kill().await?;
                status(AgentState::Stopped);
                return signal;
            }
            exit = child.wait() => exit?,
        };
        if started.elapsed() >= Duration::from_secs(60) {
            delay = Duration::from_secs(1);
        }
        status(AgentState::Retrying { exit, delay });
        tokio::select! {
            biased;
            signal = &mut stop => { status(AgentState::Stopped); return signal; }
            _ = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration(remote: &str, local: &str, transport: &str) -> String {
        format!(
            "[client]\nremote_addr = '{remote}'\n[client.services.ssh]\nlocal_addr = '{local}'\ntoken = 'unit-test-placeholder'\n{transport}"
        )
    }
    #[test]
    fn limits_services_to_the_local_device_and_requires_authenticated_wan_transport() {
        assert!(validate_client(&configuration("127.0.0.1:7000", "127.0.0.1:22", "")).is_ok());
        assert!(validate_client(&configuration("127.0.0.1:7000", "192.0.2.4:22", "")).is_err());
        assert!(
            validate_client(&configuration("example.invalid:7000", "127.0.0.1:22", "")).is_err()
        );
        let noise = "[client.transport]\ntype='noise'\n[client.transport.noise]\nremote_public_key='unit-test-placeholder'";
        assert!(
            validate_client(&configuration(
                "example.invalid:7000",
                "127.0.0.1:22",
                noise
            ))
            .is_ok()
        );
        assert!(
            validate_client(&configuration(
                "example.invalid:7000",
                "127.0.0.1:22",
                &format!("{noise}\npattern='Noise_XX_25519_ChaChaPoly_BLAKE2s'")
            ))
            .is_err()
        );
        assert!(
            !validate_client("token = 'sensitive syntax error")
                .unwrap_err()
                .to_string()
                .contains("sensitive")
        );
    }
}
