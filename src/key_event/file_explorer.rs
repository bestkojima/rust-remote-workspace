//! Key event handling for the file explorer mode.

use crossterm::event::{KeyCode, KeyEvent};

use ratatui::backend::Backend;
use ratatui_async_explorer::Input;
use std::io::Write;
use tracing::{debug, error, info};

use super::KeyFlow;

use crate::ui::file_explorer::filter_connection_indices;
use crate::{
    ActivePane, App, AppMode, CopyDirection, CopyOperation, FileExplorerPane, ScpProgress,
};

/// Handle key events in file explorer mode
pub async fn handle_file_explorer_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Handle delete confirmation popup first (highest priority)
    if let AppMode::FileExplorer {
        delete_confirmation,
        ..
    } = &mut app.mode
        && delete_confirmation.showing
    {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                delete_confirmation.hide();
                app.run_file_action(crate::filesystem::actions::Action::Delete, String::new());
                return KeyFlow::Continue;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                // Cancel deletion
                delete_confirmation.hide();
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            _ => {
                // Ignore other keys when dialog is showing
                return KeyFlow::Continue;
            }
        }
    }

    // Handle source selector popup first (needs separate scope to avoid borrow issues)
    if let AppMode::FileExplorer {
        source_selector,
        ssh_connection,
        left_pane,
        ..
    } = &mut app.mode
        && source_selector.showing
    {
        let is_left = matches!(source_selector.target_pane, ActivePane::Left);
        let local_offset: usize = if is_left { 1 } else { 0 };

        // Exclude the *other* pane's connection to prevent duplicates
        let exclude_id: Option<String> = if is_left {
            Some(ssh_connection.id.clone())
        } else {
            match left_pane {
                FileExplorerPane::RemoteSsh { connection, .. } => Some(connection.id.clone()),
                FileExplorerPane::Local => None,
            }
        };

        let mut filtered_indices = filter_connection_indices(
            app.config.connections(),
            exclude_id.as_deref(),
            source_selector.search.query(),
        );
        let mut total_items = filtered_indices.len() + local_offset;
        if source_selector.selected >= total_items {
            source_selector.selected = total_items.saturating_sub(1);
        }

        if source_selector.search.is_on() {
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(query) = source_selector.search.query_mut() {
                        query.push(c);
                    }
                    filtered_indices = filter_connection_indices(
                        app.config.connections(),
                        exclude_id.as_deref(),
                        source_selector.search.query(),
                    );
                    total_items = filtered_indices.len() + local_offset;
                    if source_selector.search.query().is_empty() {
                        source_selector.selected =
                            source_selector.selected.min(total_items.saturating_sub(1));
                    } else if filtered_indices.is_empty() {
                        source_selector.selected = 0;
                    } else {
                        source_selector.selected = local_offset;
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Backspace => {
                    if let Some(query) = source_selector.search.query_mut() {
                        query.pop();
                    }
                    filtered_indices = filter_connection_indices(
                        app.config.connections(),
                        exclude_id.as_deref(),
                        source_selector.search.query(),
                    );
                    total_items = filtered_indices.len() + local_offset;
                    if source_selector.search.query().is_empty() {
                        source_selector.selected =
                            source_selector.selected.min(total_items.saturating_sub(1));
                    } else if filtered_indices.is_empty() {
                        source_selector.selected = 0;
                    } else {
                        source_selector.selected = local_offset;
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Esc => {
                    if !source_selector.search.query().is_empty() {
                        source_selector.search.clear_query();
                        filtered_indices = filter_connection_indices(
                            app.config.connections(),
                            exclude_id.as_deref(),
                            source_selector.search.query(),
                        );
                        total_items = filtered_indices.len() + local_offset;
                        source_selector.selected =
                            source_selector.selected.min(total_items.saturating_sub(1));
                    } else {
                        source_selector.search.deactivate();
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Char('/') => {
                source_selector.search.activate();
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if source_selector.selected > 0 {
                    source_selector.selected -= 1;
                } else {
                    source_selector.selected = total_items.saturating_sub(1);
                }
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if source_selector.selected < total_items.saturating_sub(1) {
                    source_selector.selected += 1;
                } else {
                    source_selector.selected = 0;
                }
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Enter => {
                let selected_idx = source_selector.selected.min(total_items.saturating_sub(1));
                let selection = if selected_idx < local_offset {
                    None
                } else {
                    filtered_indices
                        .get(selected_idx - local_offset)
                        .and_then(|conn_idx| app.config.connections().get(*conn_idx).cloned())
                };

                let target = source_selector.target_pane;
                source_selector.hide();
                source_selector.search.deactivate();

                if let Some(conn) = selection {
                    return KeyFlow::OpenFileSource(target, Box::new(conn));
                }
                if matches!(target, ActivePane::Left) {
                    app.switch_left_pane_to_local().await;
                }

                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Esc => {
                source_selector.hide();
                source_selector.search.deactivate();
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            _ => return KeyFlow::Continue,
        }
    }

    // Main file explorer handling
    if let AppMode::FileExplorer {
        connection_name,
        left_pane,
        left_explorer,
        left_session,
        remote_explorer,
        ssh_session,
        active_pane,
        copy_buffer,
        return_to,
        search,
        source_selector,
        ssh_connection,
        delete_confirmation,
        ..
    } = &mut app.mode
    {
        // Handle search mode first
        if search.is_on() {
            match key.code {
                KeyCode::Char(c) => {
                    // Append character to search query
                    if let Some(query) = search.query_mut() {
                        query.push(c);
                    }

                    // Apply filter and select first match
                    match active_pane {
                        ActivePane::Left => {
                            left_explorer.set_search_filter(Some(search.query().to_string()));
                            // Select first match if available
                            if !left_explorer.filtered_files().is_empty() {
                                left_explorer.set_selected_idx(0);
                            }
                        }
                        ActivePane::Right => {
                            remote_explorer.set_search_filter(Some(search.query().to_string()));
                            // Select first match if available
                            if !remote_explorer.filtered_files().is_empty() {
                                remote_explorer.set_selected_idx(0);
                            }
                        }
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Backspace => {
                    // Remove last character from search query
                    if let Some(query) = search.query_mut() {
                        query.pop();
                    }

                    // Apply updated filter
                    let filter = if search.query().is_empty() {
                        None
                    } else {
                        Some(search.query().to_string())
                    };

                    match active_pane {
                        ActivePane::Left => {
                            left_explorer.set_search_filter(filter);
                            if !search.query().is_empty()
                                && !left_explorer.filtered_files().is_empty()
                            {
                                left_explorer.set_selected_idx(0);
                            }
                        }
                        ActivePane::Right => {
                            remote_explorer.set_search_filter(filter);
                            if !search.query().is_empty()
                                && !remote_explorer.filtered_files().is_empty()
                            {
                                remote_explorer.set_selected_idx(0);
                            }
                        }
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Enter => {
                    // Apply search filter
                    search.apply();
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Esc => {
                    // Exit search mode
                    search.deactivate();
                    // Clear the filter when exiting
                    left_explorer.set_search_filter(None);
                    remote_explorer.set_search_filter(None);
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                _ => {
                    return KeyFlow::Continue;
                }
            }
        }

        // Handle Esc when search filter is applied (but not actively editing)
        if matches!(search, crate::SearchState::Applied { .. }) && key.code == KeyCode::Esc {
            search.deactivate();
            // Clear the filter
            left_explorer.set_search_filter(None);
            remote_explorer.set_search_filter(None);
            app.mark_redraw();
            return KeyFlow::Continue;
        }

        if matches!(
            key.code,
            KeyCode::Char('c' | 'd' | 'o' | 'R' | 'l') | KeyCode::Enter | KeyCode::Right
        ) && match active_pane {
            ActivePane::Left => left_explorer.filtered_files().is_empty(),
            ActivePane::Right => remote_explorer.filtered_files().is_empty(),
        } {
            return KeyFlow::Continue;
        }
        // Handle copy mode cancellation first
        if key.code == KeyCode::Esc && !copy_buffer.is_empty() {
            copy_buffer.clear();
            // app.info = Some("Cleared selected files".to_string());
            app.mark_redraw();
            return KeyFlow::Continue;
        }

        // Handle quit
        if key.code == KeyCode::Char('q') {
            // Return to connection list
            let return_to_idx = *return_to;
            app.go_to_connection_list_with_selected(return_to_idx);
            return KeyFlow::Continue;
        }

        // Handle navigation and actions based on active pane
        // We need to handle each pane separately due to different types
        match key.code {
            // Navigation: Move selection up
            KeyCode::Char('k') | KeyCode::Up => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(Input::Up).await,
                    ActivePane::Right => remote_explorer.handle(Input::Up).await,
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            // Navigation: Move selection down
            KeyCode::Char('j') | KeyCode::Down => {
                let result = match active_pane {
                    ActivePane::Left => {
                        left_explorer
                            .handle(ratatui_async_explorer::Input::Down)
                            .await
                    }
                    ActivePane::Right => {
                        remote_explorer
                            .handle(ratatui_async_explorer::Input::Down)
                            .await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            // Navigation: Enter directory
            KeyCode::Right | KeyCode::Enter | KeyCode::Char('l') => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(Input::Right).await,
                    ActivePane::Right => remote_explorer.handle(Input::Right).await,
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                // Clear search filter when changing directories
                search.deactivate();
                left_explorer.set_search_filter(None);
                remote_explorer.set_search_filter(None);
                app.mark_redraw();
            }

            // Navigation: Go to parent directory
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(Input::Left).await,
                    ActivePane::Right => remote_explorer.handle(Input::Left).await,
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                // Clear search filter when changing directories
                search.deactivate();
                left_explorer.set_search_filter(None);
                remote_explorer.set_search_filter(None);
                app.mark_redraw();
            }

            // Switch pane with Tab
            KeyCode::Tab => {
                *active_pane = match active_pane {
                    ActivePane::Left => ActivePane::Right,
                    ActivePane::Right => ActivePane::Left,
                };
                app.mark_redraw();
            }

            // Switch left pane source with 's'
            KeyCode::Char('s') => {
                source_selector.show_for(*active_pane);
                source_selector.search.deactivate();

                if matches!(active_pane, ActivePane::Left) {
                    // Left pane: exclude right connection, include Local option
                    let base_indices = filter_connection_indices(
                        app.config.connections(),
                        Some(ssh_connection.id.as_str()),
                        "",
                    );
                    source_selector.selected = match left_pane {
                        FileExplorerPane::Local => 0,
                        FileExplorerPane::RemoteSsh { connection, .. } => {
                            let connections = app.config.connections();
                            base_indices
                                .iter()
                                .position(|idx| {
                                    connections
                                        .get(*idx)
                                        .map(|conn| conn.id == connection.id)
                                        .unwrap_or(false)
                                })
                                .map(|idx| idx + 1)
                                .unwrap_or(0)
                        }
                    };
                } else {
                    // Right pane: exclude left connection (if remote), no Local option
                    let exclude_id = match left_pane {
                        FileExplorerPane::RemoteSsh { connection, .. } => {
                            Some(connection.id.as_str())
                        }
                        FileExplorerPane::Local => None,
                    };
                    let base_indices =
                        filter_connection_indices(app.config.connections(), exclude_id, "");
                    let connections = app.config.connections();
                    source_selector.selected = base_indices
                        .iter()
                        .position(|idx| {
                            connections
                                .get(*idx)
                                .map(|conn| conn.id == ssh_connection.id)
                                .unwrap_or(false)
                        })
                        .unwrap_or(0);
                }
                app.mark_redraw();
            }

            KeyCode::F(7) if copy_buffer.is_empty() && !search.is_on() => {
                app.sync_from_files();
            }
            KeyCode::Char('g' | 'm' | 'R') => {
                use crate::filesystem::actions::Action;
                app.prompt_file_action(match key.code {
                    KeyCode::Char('g') => Action::Go,
                    KeyCode::Char('m') => Action::Mkdir,
                    _ => Action::Rename,
                });
            }
            // Delete file: Show delete confirmation
            KeyCode::Char('d') => {
                let (current_file, pane) = match active_pane {
                    ActivePane::Left => (left_explorer.current(), ActivePane::Left),
                    ActivePane::Right => (remote_explorer.current(), ActivePane::Right),
                };

                if matches!(current_file.name().trim_end_matches('/'), "" | "." | "..") {
                    return KeyFlow::Continue;
                }
                // Show delete confirmation dialog
                delete_confirmation.show(current_file.name().to_string(), pane);
                app.mark_redraw();
            }

            // Open file / open terminal on remote directory
            KeyCode::Char('o') => {
                let current = match active_pane {
                    ActivePane::Left => left_explorer.current(),
                    ActivePane::Right => remote_explorer.current(),
                };
                let is_dir = current.is_dir();
                let is_local = matches!(active_pane, ActivePane::Left)
                    && matches!(left_pane, FileExplorerPane::Local);

                if is_dir {
                    // On a remote directory: open SSH terminal there
                    if is_local {
                        app.info = Some("Cannot open terminal on local directory".to_string());
                        app.mark_redraw();
                        return KeyFlow::Continue;
                    }

                    let target_path = current.path().to_string_lossy().into_owned();
                    let (session_arc, conn_name) = match active_pane {
                        ActivePane::Right => (ssh_session.clone(), connection_name.clone()),
                        ActivePane::Left => match left_pane {
                            FileExplorerPane::RemoteSsh {
                                connection_name: cn,
                                ..
                            } => match left_session {
                                Some(s) => (s.clone(), cn.clone()),
                                None => {
                                    app.info =
                                        Some("Cannot open terminal: no SSH session".to_string());
                                    app.mark_redraw();
                                    return KeyFlow::Continue;
                                }
                            },
                            FileExplorerPane::Local => unreachable!(),
                        },
                    };
                    let return_to_idx = *return_to;

                    // Open terminal channel on existing session
                    let (cols, rows) = app.ssh_terminal_size().unwrap_or((80, 24));
                    let mut client = match crate::async_ssh_client::SshSession::open_terminal_on(
                        &session_arc,
                        cols,
                        rows,
                        Some(&target_path),
                    )
                    .await
                    {
                        Ok(c) => c,
                        Err(e) => {
                            app.set_error(e);
                            return KeyFlow::Continue;
                        }
                    };

                    // Save FileExplorer state
                    let old_mode = std::mem::replace(
                        &mut app.mode,
                        AppMode::ConnectionList(crate::ListSelectionState::new(0)),
                    );

                    let saved_return_mode = if let AppMode::FileExplorer {
                        connection_name,
                        left_pane,
                        left_explorer,
                        left_session,
                        remote_explorer,
                        ssh_connection,
                        channel: _,
                        ssh_session,
                        active_pane,
                        copy_buffer: _,
                        return_to,
                        search,
                        ..
                    } = old_mode
                    {
                        crate::ScpReturnMode::FileExplorer {
                            connection_name,
                            left_pane,
                            left_explorer,
                            left_session,
                            remote_explorer,
                            ssh_connection,
                            channel: None,
                            ssh_session,
                            active_pane,
                            copy_buffer: Vec::new(),
                            return_to,
                            search,
                        }
                    } else {
                        unreachable!("mode was just FileExplorer");
                    };

                    // Set up terminal + read loop
                    let scrollback = app.config.terminal_scrollback_lines();
                    let state = std::sync::Arc::new(tokio::sync::Mutex::new(
                        crate::ui::TerminalState::new_with_scrollback(rows, cols, scrollback),
                    ));
                    let cancel_token = client.start_reader(state.clone(), app.get_event_sender());

                    app.go_to_connected_with_return(
                        conn_name,
                        client,
                        state,
                        return_to_idx,
                        cancel_token,
                        Some(saved_return_mode),
                    );
                    return KeyFlow::Continue;
                }

                // File edit path — extract owned path before releasing borrow
                let file_path = current.path().to_string_lossy().into_owned();
                let pane = *active_pane;

                // Get connection for remote editing
                let remote_connection = if is_local {
                    None
                } else if matches!(active_pane, ActivePane::Left) {
                    // Left pane is remote SSH
                    if let FileExplorerPane::RemoteSsh { connection, .. } = left_pane {
                        Some(connection.clone())
                    } else {
                        None
                    }
                } else {
                    // Right pane is always remote
                    Some(ssh_connection.clone())
                };

                // Perform the edit operation outside the app.mode borrow
                do_edit_file(app, &file_path, is_local, remote_connection, pane).await;
                return KeyFlow::Continue;
            }

            // Copy file: Toggle selection
            KeyCode::Char('c') => {
                let (current_file, direction) = match active_pane {
                    ActivePane::Left => (left_explorer.current(), CopyDirection::LeftToRight),
                    ActivePane::Right => (remote_explorer.current(), CopyDirection::RightToLeft),
                };

                if matches!(current_file.name().trim_end_matches('/'), "" | "." | "..") {
                    return KeyFlow::Continue;
                }
                let is_dir = current_file.is_dir();
                let source_path = current_file.path().to_string_lossy().into_owned();
                if let Some(existing_idx) = copy_buffer
                    .iter()
                    .position(|item| item.source_path == source_path)
                {
                    copy_buffer.remove(existing_idx);
                } else {
                    if let Some(existing_direction) = copy_buffer.first().map(|item| item.direction)
                        && existing_direction != direction
                    {
                        app.info = Some(
                            "Selected files must all come from the same source pane".to_string(),
                        );
                        app.mark_redraw();
                        return KeyFlow::Continue;
                    }

                    info!(
                        "File added to copy buffer: {} (direction: {:?})",
                        source_path, direction
                    );
                    copy_buffer.push(CopyOperation {
                        source_path,
                        source_name: current_file.name().to_string(),
                        direction,
                        is_dir,
                    });
                }
                app.mark_redraw();
            }

            // Paste file: Execute transfer (batch aware)
            KeyCode::Char('v') => {
                if copy_buffer.is_empty() {
                    app.info =
                        Some("No files selected - press 'c' on files to add them".to_string());
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                info!(
                    "Initiating file paste operation with {} files",
                    copy_buffer.len()
                );
                let dest_dir = match active_pane {
                    ActivePane::Left => left_explorer.cwd().to_string_lossy().into_owned(),
                    ActivePane::Right => remote_explorer.cwd().to_string_lossy().into_owned(),
                };

                let direction = copy_buffer[0].direction;
                let same_pane = matches!(
                    (direction, active_pane),
                    (CopyDirection::LeftToRight, ActivePane::Left)
                        | (CopyDirection::RightToLeft, ActivePane::Right)
                );

                if same_pane {
                    debug!("Cannot paste in same pane");
                    app.info =
                        Some("Cannot paste in the same pane - press Tab to switch".to_string());
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                let AppMode::FileExplorer {
                    connection_name,
                    left_pane,
                    left_explorer,
                    left_session,
                    remote_explorer,
                    ssh_connection,
                    channel,
                    ssh_session,
                    active_pane,
                    copy_buffer,
                    return_to,
                    search,
                    ..
                } = std::mem::replace(
                    &mut app.mode,
                    AppMode::ConnectionList(crate::ListSelectionState::new(0)),
                )
                else {
                    unreachable!()
                };
                let Some(events) = app.get_event_sender() else {
                    unreachable!("Files requires an event sender")
                };
                let progress = ScpProgress::new(connection_name.clone(), Vec::new());
                let cancel = progress.cancel.clone();
                use crate::{files::SftpEndpoint, filesystem::file_copy::Side};
                let left = match &left_explorer {
                    crate::LeftExplorer::Local(_) => Side::Local,
                    crate::LeftExplorer::Remote(e) => Side::Remote(SftpEndpoint::from_parts(
                        e.filesystem().clone(),
                        left_session.as_ref().unwrap().clone(),
                    )),
                };
                let right = Side::Remote(SftpEndpoint::from_parts(
                    remote_explorer.filesystem().clone(),
                    ssh_session.clone(),
                ));
                let return_mode = crate::ScpReturnMode::FileExplorer {
                    connection_name,
                    left_pane,
                    left_explorer,
                    left_session,
                    remote_explorer,
                    ssh_connection,
                    channel,
                    ssh_session,
                    active_pane,
                    copy_buffer: Vec::new(),
                    return_to,
                    search,
                };
                app.go_to_scp_progress(progress, return_mode);
                app.start_ticker();
                tokio::spawn(crate::filesystem::file_copy::run(
                    copy_buffer,
                    dest_dir,
                    left,
                    right,
                    cancel,
                    events,
                ));
            }
            // Toggle hidden files
            KeyCode::Char('H') => {
                let result = match active_pane {
                    ActivePane::Left => {
                        let show_hidden = !left_explorer.show_hidden();
                        left_explorer
                            .set_show_hidden(show_hidden)
                            .await
                            .map(|_| show_hidden)
                    }
                    ActivePane::Right => {
                        let show_hidden = !remote_explorer.show_hidden();
                        remote_explorer
                            .set_show_hidden(show_hidden)
                            .await
                            .map(|_| show_hidden)
                    }
                };

                match result {
                    Ok(_show_hidden) => {
                        // app.info = Some(format!(
                        //     "Hidden files {}",
                        //     if show_hidden { "shown" } else { "hidden" }
                        // ));
                    }
                    Err(e) => {
                        app.error = Some(crate::error::AppError::SftpError(format!(
                            "Failed to toggle hidden files: {e}"
                        )));
                    }
                }
                app.mark_redraw();
            }

            // Refresh current pane
            KeyCode::Char('r') => {
                let result = match active_pane {
                    ActivePane::Left => {
                        let current_path = left_explorer.cwd().to_path_buf();
                        left_explorer.set_cwd(current_path).await
                    }
                    ActivePane::Right => {
                        let current_path = remote_explorer.cwd().to_path_buf();
                        remote_explorer.set_cwd(current_path).await
                    }
                };

                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Failed to refresh: {e}"
                    )));
                } else {
                    app.info = Some("Refreshed".to_string());
                }
                app.mark_redraw();
            }

            // Home/End for quick navigation
            KeyCode::Home => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(Input::Home).await,
                    ActivePane::Right => remote_explorer.handle(Input::Home).await,
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            KeyCode::End => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(Input::End).await,
                    ActivePane::Right => remote_explorer.handle(Input::End).await,
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            // Page up/down for faster scrolling
            KeyCode::PageUp => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(Input::PageUp).await,
                    ActivePane::Right => remote_explorer.handle(Input::PageUp).await,
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            KeyCode::PageDown => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(Input::PageDown).await,
                    ActivePane::Right => remote_explorer.handle(Input::PageDown).await,
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            // Enter search mode with '/'
            KeyCode::Char('/') => {
                search.activate();
                // Clear any existing filters to show all items
                left_explorer.set_search_filter(None);
                remote_explorer.set_search_filter(None);
                app.mark_redraw();
            }

            _ => {}
        }

        KeyFlow::Continue
    } else {
        KeyFlow::Continue
    }
}

/// Perform file editing outside the `app.mode` borrow scope.
///
/// The edit functions in [`crate::file_edit`] handle suspending/restoring the
/// terminal and running the editor on a blocking thread. This helper only
/// deals with binary checks, result handling, and app-level state updates.
async fn do_edit_file<B: Backend + Write>(
    app: &mut App<B>,
    file_path: &str,
    is_local: bool,
    remote_connection: Option<crate::config::manager::Connection>,
    pane: ActivePane,
) {
    // Binary check before opening the editor so the user sees the error inline
    let is_binary = if is_local {
        crate::file_edit::check_binary(file_path)
            .await
            .unwrap_or(false)
    } else {
        crate::file_edit::check_remote_binary(remote_connection.as_ref(), file_path)
            .await
            .unwrap_or(false)
    };
    if is_binary {
        app.error = Some(crate::error::AppError::SftpError(
            "Cannot edit binary file".to_string(),
        ));
        app.mark_redraw();
        return;
    }

    let tick_tx = match app.get_tick_control_sender() {
        Some(tx) => tx,
        None => {
            app.error = Some(crate::error::AppError::SftpError(
                "Tick control sender not available".to_string(),
            ));
            app.mark_redraw();
            return;
        }
    };

    let result = if is_local {
        crate::file_edit::edit_local_file(file_path, &tick_tx).await
    } else if let Some(conn) = remote_connection {
        crate::file_edit::edit_remote_file(file_path, &conn, &tick_tx).await
    } else {
        Err(crate::error::AppError::SftpError(
            "Could not determine connection for remote file".to_string(),
        ))
    };

    // App-level state cleanup after terminal restore
    if let Err(e) = app.post_editor_cleanup() {
        error!("Failed to clean up after editing: {}", e);
    }

    match result {
        Ok(crate::file_edit::EditResult::Modified) => {
            // Refresh the active pane to reflect changes
            let filename = std::path::Path::new(file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file_path);
            if let AppMode::FileExplorer {
                left_explorer,
                remote_explorer,
                ..
            } = &mut app.mode
            {
                let refresh_result = match pane {
                    ActivePane::Left => {
                        let cwd = left_explorer.cwd().to_path_buf();
                        left_explorer
                            .set_cwd(cwd)
                            .await
                            .map(|_| left_explorer.select_file(filename))
                    }
                    ActivePane::Right => {
                        let cwd = remote_explorer.cwd().to_path_buf();
                        remote_explorer
                            .set_cwd(cwd)
                            .await
                            .map(|_| remote_explorer.select_file(filename))
                    }
                };
                if let Err(e) = refresh_result {
                    error!("Failed to refresh pane after edit: {}", e);
                }
            }
            // Set info after refresh so the async operation doesn't cause the popup to be dismissed
            app.info = Some("File saved".to_string());
        }
        Ok(crate::file_edit::EditResult::Unchanged) => {
            // app.info = Some("File unchanged".to_string());
        }
        Err(e) => {
            app.error = Some(e);
        }
    }

    app.mark_redraw();
}
