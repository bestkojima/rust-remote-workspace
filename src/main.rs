use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand};
use crossterm::event;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::{select, sync::mpsc, time};

use termirs::{App, AppEvent, Result, TickControl, init_panic_hook, init_scoped_tracing};

/// A modern, async SSH terminal client
#[derive(Parser, Debug)]
#[command(name = env!("CARGO_BIN_NAME"))]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Use this device configuration instead of ~/.config/termirs/config.toml
    #[arg(long, value_name = "FILE")]
    config: Option<std::path::PathBuf>,

    /// Enable logging to termirs.log file
    #[arg(short, long)]
    log: bool,

    /// Set log level (off, error, warn, info, debug, trace)
    /// Overrides the saved level when logging is enabled (default for --log: info)
    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the authorized development device's rathole client in the foreground
    TunnelAgent {
        /// Administrator-provisioned private rathole client TOML
        #[arg(long, value_name = "FILE")]
        config: std::path::PathBuf,
        /// rathole executable (defaults to the one on PATH)
        #[arg(long, value_name = "EXE", default_value = "rathole")]
        rathole: std::path::PathBuf,
        /// Stop and reap the child when the supervising process closes standard input
        #[arg(long)]
        stop_on_stdin_close: bool,
    },
}

async fn stop_agent(on_eof: bool) -> std::io::Result<()> {
    // A detached blocking stdin reader lets Ctrl+C still finish immediately while a
    // manager holds the pipe open (Tokio runtime shutdown waits for blocking tasks).
    let eof = async {
        if !on_eof {
            return std::future::pending().await;
        }
        let (send, recv) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(
                std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink()).map(|_| ()),
            );
        });
        recv.await.map_err(std::io::Error::other)?
    };
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = eof => result, result = tokio::signal::ctrl_c() => result, _ = term.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    tokio::select! { result = eof => result, result = tokio::signal::ctrl_c() => result }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();
    if let Some(Command::TunnelAgent {
        config,
        rathole,
        stop_on_stdin_close,
    }) = args.command
    {
        use remote_core::tunnel_agent::{AgentState, supervise};
        supervise(
            &rathole,
            &config,
            stop_agent(stop_on_stdin_close),
            |state| match state {
                AgentState::Running { pid } => eprintln!("agent: running (pid {pid})"),
                AgentState::Retrying { exit, delay } => {
                    eprintln!("agent: retry in {}s ({exit})", delay.as_secs())
                }
                AgentState::Stopped => eprintln!("agent: stopped"),
            },
        )
        .await?;
        return Ok(());
    }

    let start_log = |level: &str| -> Result<_> {
        let guard = init_scoped_tracing(level)?;
        tracing::info!("Starting termirs SSH client v{}", env!("CARGO_PKG_VERSION"));
        tracing::info!("Logging enabled at level: {}", level);
        Ok(guard)
    };
    // CLI logging also captures startup/config errors, before the TUI is created.
    let mut log = if args.log {
        Some(start_log(args.log_level.as_deref().unwrap_or("info"))?)
    } else {
        None
    };

    init_panic_hook();

    // Setup Crossterm terminal
    tracing::debug!("Initializing terminal backend");
    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::with_config_path(terminal, args.config.as_deref())?;
    if log.is_none()
        && let Some(saved) = app.config.startup_log_level()
    {
        log = Some(start_log(args.log_level.as_deref().unwrap_or(saved))?);
    }
    let _log_guard = log;
    app.init_terminal()?;
    tracing::info!("Terminal initialized successfully");

    // async event channel
    let (tx, mut rx) = mpsc::channel::<AppEvent>(100);
    let (tick_control_tx, mut tick_control_rx) = mpsc::channel::<TickControl>(10);

    // Set the event sender in the app
    app.set_event_sender(tx.clone());
    app.set_tick_control_sender(tick_control_tx);

    // ticker - 50ms interval, conditionally enabled
    let mut ticker = time::interval(Duration::from_millis(50));
    let tx_tick = tx.clone();

    // Shared flag: the blocking poll thread only reads events when true.
    let input_enabled = Arc::new(AtomicBool::new(true));
    let input_enabled_poller = input_enabled.clone();

    // Shared flag: set to false on shutdown so the blocking thread exits.
    let running = Arc::new(AtomicBool::new(true));
    let running_poller = running.clone();

    // Sync event polling on a dedicated blocking thread.
    // Uses crossterm's sync poll/read API directly, avoiding EventStream's
    // background thread and its global mutex deadlock.
    let tx_input = tx.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            if !running_poller.load(Ordering::Relaxed) {
                break;
            }
            if !input_enabled_poller.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            match event::poll(Duration::from_millis(10)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx_input.blocking_send(AppEvent::Input(ev)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error reading event: {:?}", e);
                        break;
                    }
                },
                Ok(false) => {} // no event within 10ms
                Err(e) => {
                    tracing::error!("Error polling event: {:?}", e);
                    break;
                }
            }
        }
    });

    // Tick and control handling
    tokio::spawn(async move {
        let mut tick_enabled = false;
        loop {
            select! {
                _ = ticker.tick(), if tick_enabled => {
                    if tx_tick.send(AppEvent::Tick).await.is_err() {
                        break;
                    }
                }
                result = tick_control_rx.recv() => {
                    match result {
                        Some(control) => match control {
                            TickControl::Start => tick_enabled = true,
                            TickControl::Stop => tick_enabled = false,
                            TickControl::PauseInput => {
                                input_enabled.store(false, Ordering::Relaxed);
                                tracing::info!("Pausing input polling");
                            },
                            TickControl::ResumeInput => {
                                input_enabled.store(true, Ordering::Relaxed);
                                tracing::info!("Resuming input polling");
                            },
                        },
                        None => break, // channel closed, shut down
                    }
                }
            }
        }
    });

    // run app loop
    tracing::info!("Starting main event loop");
    let res = app
        .run(&mut rx)
        .await
        .inspect_err(|e| tracing::error!("Error in main event loop: {}", e));

    // Signal the blocking poll thread to exit.
    running.store(false, Ordering::Relaxed);

    // app drop restores terminal
    tracing::info!("Application shutting down");
    drop(app);

    res
}
