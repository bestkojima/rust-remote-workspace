use crossterm::event::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickControl {
    Start,       // Start sending tick events
    Stop,        // Stop sending tick events
    PauseInput,  // Stop polling stdin (for external editor)
    ResumeInput, // Resume polling stdin
}

#[derive(Debug)]
pub enum AppEvent {
    SyncUpdate,
    FileActionDone,
    HomeTunnel {
        device: String,
        generation: u64,
        event: crate::tunnel_home::HomeEvent,
    },
    Input(Event),
    Tick,
    Disconnect(remote_core::SessionId),
    TerminalUpdate(remote_core::SessionId),
    SftpProgress(crate::transfer::ScpResult), // Sent when SFTP transfer has progress/completion
}
