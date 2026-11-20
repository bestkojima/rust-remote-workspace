mod async_ssh_client;
mod config;
mod error;
mod file_edit;
mod filesystem;
mod key_event;
mod mode_state;
mod search_state;
mod sync_engine;
mod ui;
mod workspace;

// New modules from refactoring
mod app;
mod events;
mod terminal;
mod transfer;
mod tunnel_home;
mod utils;

// Re-export commonly used types
pub use app::{
    ActivePane, App, AppMode, ConnectingSource, CopyDirection, CopyOperation, FileExplorerPane,
    LeftExplorer, ScpReturnMode, create_search_textarea,
};
pub use async_ssh_client::expand_tilde;
pub use error::{AppError, Result};
pub use events::{AppEvent, TickControl};
pub use mode_state::{
    ConnectionSelectorState, DeleteConfirmationState, FormWithConnectionSelector,
    ListSelectionState, SourceSelectorState,
};
pub use search_state::SearchState;
pub use transfer::{
    ScpFileProgress, ScpFileResult, ScpProgress, ScpResult, ScpTransferProgress, ScpTransferSpec,
    TransferState,
};
pub use tunnel_home::HomeEvent;
pub use utils::{init_panic_hook, init_scoped_tracing, init_tracing};

/// The SSH adapter used by the workspace, usable without constructing a TUI.
pub mod session {
    pub use crate::async_ssh_client::SshSession;
    pub use crate::config::manager::{AuthMethod, ConfigManager, Connection};
    pub use remote_core::{SessionId, terminal::Output};
}

/// Single-file transfers using validated SSH endpoints, without a TUI.
pub mod files {
    pub use crate::filesystem::single_file::{FileLocation, FileProgress, SftpEndpoint, copy_file};
}

/// Manual directory copying without a TUI. Each execution consumes its preview.
pub mod sync {
    pub use crate::sync_engine::{
        Change, ChangeKind, Direction, Preview, RemoteScope, ScopeMode, SyncProgress, SyncResult,
        SyncSpec, execute, preview_scoped,
    };
}

// Implement ByteProcessor for TerminalState
impl async_ssh_client::ByteProcessor for ui::TerminalState {
    fn process_bytes(&mut self, bytes: &[u8]) {
        self.process_bytes(bytes);
    }
}
