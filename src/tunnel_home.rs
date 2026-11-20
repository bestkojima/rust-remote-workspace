//! Home side of a managed route. The device's own SSH client uses the local
//! socket; its host key and authentication never belong to the gateway.
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use russh::{Disconnect, client};
use tokio::{
    net::TcpListener,
    sync::mpsc,
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;

use crate::{
    AppError, Result,
    async_ssh_client::{SshClient, SshSession},
    config::manager::Connection,
};

const SSH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum HomeEvent {
    Connecting,
    /// Gateway authenticated and loopback listener bound. The device has not been probed.
    Ready,
    /// The gateway is still up; a client may retry opening the fixed service.
    ServiceUnavailable(String),
    Retrying {
        seconds: u64,
        reason: String,
    },
    /// Identity, authentication, or local configuration requires user intervention.
    Blocked(AppError),
    Stopped,
}

pub(crate) struct HomeTunnel {
    cancel: CancellationToken,
    worker: Option<JoinHandle<()>>,
}

impl HomeTunnel {
    pub(crate) fn start(
        gateway: Connection,
        listen: SocketAddr,
        service_port: u16,
    ) -> Result<(Self, mpsc::Receiver<HomeEvent>)> {
        if listen.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || listen.port() == 0 || service_port == 0
        {
            return Err(AppError::ValidationError(
                "Use 127.0.0.1 and nonzero ports for this VPS tunnel.".into(),
            ));
        }
        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::channel(16);
        let worker = tokio::spawn(supervise(gateway, listen, service_port, cancel.clone(), tx));
        Ok((
            Self {
                cancel,
                worker: Some(worker),
            },
            rx,
        ))
    }

    pub(crate) async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }
}

impl Drop for HomeTunnel {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct Events {
    cancel: CancellationToken,
    tx: mpsc::Sender<HomeEvent>,
}

impl Events {
    async fn send(&self, event: HomeEvent) -> bool {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => false,
            sent = self.tx.send(event) => sent.is_ok(),
        }
    }
}

async fn supervise(
    gateway: Connection,
    listen: SocketAddr,
    service_port: u16,
    cancel: CancellationToken,
    tx: mpsc::Sender<HomeEvent>,
) {
    let events = Events { cancel, tx };
    let mut delay = 1;
    while events.send(HomeEvent::Connecting).await {
        let connect =
            SshSession::new_session_with_timeout(&gateway, Some(SSH_TIMEOUT), &events.cancel);
        let session = tokio::select! {
            biased;
            _ = events.cancel.cancelled() => break,
            res = timeout(SSH_TIMEOUT, connect) => res.unwrap_or_else(|_| {
                Err(AppError::SshConnectionError("VPS SSH handshake or authentication timed out".into()))
            }),
        };
        let error = match session {
            Ok(mut session) => {
                let listener = match TcpListener::bind(listen).await {
                    Ok(listener) => listener,
                    Err(e) => {
                        events
                            .send(HomeEvent::Blocked(AppError::PortForwardingError(format!(
                                "Cannot listen on {listen}: {e}"
                            ))))
                            .await;
                        disconnect(&mut session).await;
                        return;
                    }
                };
                let since = Instant::now();
                let result = serve(listener, &mut session, service_port, &events).await;
                if since.elapsed() >= Duration::from_secs(30) {
                    delay = 1;
                }
                match result {
                    End::Stopped => {
                        disconnect(&mut session).await;
                        break;
                    }
                    End::Lost(reason) => {
                        if !session.is_closed() {
                            disconnect(&mut session).await;
                        }
                        reason
                    }
                    End::Blocked(error) => {
                        if !session.is_closed() {
                            disconnect(&mut session).await;
                        }
                        events.send(HomeEvent::Blocked(error)).await;
                        return;
                    }
                }
            }
            Err(e) if retryable(&e) => e.to_string(),
            Err(e) => {
                events.send(HomeEvent::Blocked(e)).await;
                return;
            }
        };
        if !events
            .send(HomeEvent::Retrying {
                seconds: delay,
                reason: error,
            })
            .await
        {
            break;
        }
        tokio::select! {
            _ = events.cancel.cancelled() => break,
            _ = sleep(Duration::from_secs(delay)) => {}
        }
        delay = (delay * 2).min(30);
    }
    let _ = events.tx.try_send(HomeEvent::Stopped);
}

async fn disconnect(session: &mut client::Handle<SshClient>) {
    let _ = timeout(Duration::from_secs(1), async {
        session
            .disconnect(Disconnect::ByApplication, "Tunnel stopped", "")
            .await?;
        session.await
    })
    .await;
}

fn retryable(error: &AppError) -> bool {
    match error {
        AppError::RusshError(e) => SshSession::is_transport_error(e),
        AppError::IOError(_) | AppError::SshConnectionError(_) => true,
        _ => false,
    }
}

enum End {
    Stopped,
    Lost(String),
    Blocked(AppError),
}

async fn serve(
    listener: TcpListener,
    session: &mut client::Handle<SshClient>,
    service_port: u16,
    events: &Events,
) -> End {
    if !events.send(HomeEvent::Ready).await {
        return End::Stopped;
    }
    let mut transfers = JoinSet::new();
    let mut service_available = true;
    let end = loop {
        let (mut socket, peer) = tokio::select! {
            biased;
            _ = events.cancel.cancelled() => break End::Stopped,
            closed = &mut *session => break match closed {
                Ok(()) => End::Lost("VPS disconnected".into()),
                Err(e) if retryable(&e) => End::Lost(e.to_string()),
                Err(e) => End::Blocked(e),
            },
            _ = transfers.join_next(), if !transfers.is_empty() => continue,
            accepted = listener.accept(), if transfers.len() < 64 => match accepted {
                Ok(pair) => pair,
                Err(e) => break End::Blocked(e.into()),
            },
        };
        // The requester cannot supply either the destination address or the port.
        let open = session.channel_open_direct_tcpip(
            "127.0.0.1",
            u32::from(service_port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        );
        let channel = tokio::select! {
            biased;
            _ = events.cancel.cancelled() => break End::Stopped,
            result = timeout(SSH_TIMEOUT, open) => match result {
                Ok(Ok(channel)) => Ok(channel),
                Ok(Err(e)) if session.is_closed() || SshSession::is_transport_error(&e) => break End::Lost(e.to_string()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("timed out".into()),
            }
        };
        match channel {
            Ok(channel) => {
                if !service_available && !events.send(HomeEvent::Ready).await {
                    break End::Stopped;
                }
                service_available = true;
                transfers.spawn(async move {
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut channel.into_stream())
                        .await;
                });
            }
            Err(e) => {
                if service_available
                    && !events
                        .send(HomeEvent::ServiceUnavailable(format!(
                            "VPS service 127.0.0.1:{service_port}: {e}"
                        )))
                        .await
                {
                    break End::Stopped;
                }
                service_available = false;
            }
        }
    };
    drop(listener);
    transfers.shutdown().await;
    end
}
