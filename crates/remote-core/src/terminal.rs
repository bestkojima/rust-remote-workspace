//! One interactive PTY on an authenticated SSH transport. Front ends own their
//! screen buffers; reading this object neither paints a screen nor starts a task.
use crate::SessionId;
use russh::{Channel, ChannelMsg, CryptoVec, client};
use tokio::io::AsyncWriteExt;

pub struct Terminal {
    id: SessionId,
    reader: Option<Output>,
    writer: russh::ChannelWriteHalf<client::Msg>,
}

impl Terminal {
    /// Open a shell on this channel. Closing it leaves sibling channels alone.
    /// The caller owns the enclosing transport and its authentication policy.
    pub async fn open(
        channel: Channel<client::Msg>,
        cols: u16,
        rows: u16,
        directory: Option<&str>,
    ) -> Result<Self, russh::Error> {
        let _ = channel.set_env(false, "LC_CTYPE", "C.UTF-8").await;
        channel
            .request_pty(true, "xterm-256color", cols.into(), rows.into(), 0, 0, &[])
            .await?;
        if let Some(dir) = directory {
            let escaped = dir.replace('\'', "'\\''");
            channel
                .exec(true, format!("cd '{escaped}' && exec $SHELL -l").as_bytes())
                .await?;
        } else {
            channel.request_shell(true).await?;
        }
        let id = SessionId::allocate();
        let (reader, writer) = channel.split();
        Ok(Self {
            id,
            reader: Some(Output { id, reader }),
            writer,
        })
    }
    pub fn id(&self) -> SessionId {
        self.id
    }
    pub fn take_output(&mut self) -> Option<Output> {
        self.reader.take()
    }
    pub async fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.make_writer().write_all(bytes).await
    }
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), russh::Error> {
        self.writer
            .window_change(cols.into(), rows.into(), 0, 0)
            .await
    }
    pub async fn close(&self) -> Result<(), russh::Error> {
        self.writer.close().await
    }
}

/// A single consumer of one PTY's bytes. EOF is reported as `None`.
pub struct Output {
    id: SessionId,
    reader: russh::ChannelReadHalf,
}
impl Output {
    pub fn id(&self) -> SessionId {
        self.id
    }
    pub fn into_raw(self) -> russh::ChannelReadHalf {
        self.reader
    }
    pub async fn read(&mut self) -> Option<CryptoVec> {
        loop {
            match self.reader.wait().await? {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    return Some(data);
                }
                ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => return None,
                _ => {}
            }
        }
    }
}
