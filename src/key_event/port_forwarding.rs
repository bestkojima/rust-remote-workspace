use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;

use super::KeyFlow;
use super::table_handler::{handle_navigation_keys, handle_search_keys};
use crate::SearchState;
use crate::app::{App, AppMode};
use crate::config::manager::{PortForward, PortForwardStatus};
use crate::error::AppError;
use crate::ui::table::TableListState;
use crate::ui::{PortForwardingForm, file_explorer::filter_connection_indices};

pub async fn handle_port_forwarding_list_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Handle search keys using shared handler
    if let AppMode::PortForwardingList(state) = &mut app.mode {
        let mut table_state = TableListState::from_parts(state.selected, state.search.clone());
        if handle_search_keys(&mut table_state, key) {
            state.selected = table_state.selected;
            state.search = table_state.search;
            app.mark_redraw();
            return KeyFlow::Continue;
        }
    }

    // Resolve the highlighted row through the same filter as the rendered list.
    let AppMode::PortForwardingList(state) = &app.mode else {
        return KeyFlow::Continue;
    };
    let visible: Vec<usize> = {
        if state.search.query().is_empty() {
            (0..app.config.port_forwards().len()).collect()
        } else {
            // Filter port forwards using same logic as the component
            let query_lower = state.search.query().to_lowercase();
            app.config
                .port_forwards()
                .iter()
                .enumerate()
                .filter(|(_, pf)| {
                    let connection_name = app
                        .config
                        .connections()
                        .iter()
                        .find(|c| c.id == pf.connection_id)
                        .map(|c| c.display_name.as_str())
                        .unwrap_or("Unknown");

                    let forward_type = match pf.forward_type {
                        crate::config::manager::PortForwardType::Local => "Local",
                        crate::config::manager::PortForwardType::Remote => "Remote",
                        crate::config::manager::PortForwardType::Dynamic => "Dynamic",
                    };

                    pf.get_display_name().to_lowercase().contains(&query_lower)
                        || connection_name.to_lowercase().contains(&query_lower)
                        || pf.local_address().to_lowercase().contains(&query_lower)
                        || pf.service_address().to_lowercase().contains(&query_lower)
                        || forward_type.to_lowercase().contains(&query_lower)
                })
                .map(|(index, _)| index)
                .collect()
        }
    };
    let len = visible.len();
    let selected = visible
        .get(state.selected.min(len.saturating_sub(1)))
        .copied();

    // Handle navigation keys using shared handler
    if let AppMode::PortForwardingList(state) = &mut app.mode {
        let mut table_state = TableListState::from_parts(state.selected, state.search.clone());
        if handle_navigation_keys(&mut table_state, key, len) {
            state.selected = table_state.selected;
            state.search = table_state.search;
            app.mark_redraw();
            return KeyFlow::Continue;
        }
    }

    // Handle component-specific actions
    match key.code {
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.go_to_port_forwarding_form_new();
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if let Some(original) = selected.map(|i| &app.config.port_forwards()[i]) {
                let form = PortForwardingForm::from(original);
                app.go_to_port_forwarding_form_edit(form);
            }
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(pf) = selected.map(|i| &app.config.port_forwards()[i]) {
                let pf_name = pf.get_display_name();
                let pf_id = pf.id.clone();
                let current_selected = app.current_selected();
                app.go_to_port_forward_delete_confirmation(pf_name, pf_id, current_selected);
            }
        }
        KeyCode::Enter => {
            if let Some(rule) = selected.map(|i| app.config.port_forwards()[i].clone()) {
                if rule.status == PortForwardStatus::Running {
                    match app
                        .port_forwarding_runtime
                        .stop_port_forward(&rule.id)
                        .await
                    {
                        Ok(()) => {
                            app.config.find_port_forward_mut(&rule.id).unwrap().status =
                                PortForwardStatus::Stopped;
                            app.set_info(format!(
                                "Port forward '{}' stopped",
                                rule.get_display_name()
                            ));
                        }
                        Err(e) => app.set_error(e),
                    }
                } else {
                    return KeyFlow::StartForward(Box::new(rule));
                }
            } else {
                app.set_info("No port forwards available".to_string());
            }
        }
        KeyCode::Char('q') => {
            // Return to connection list
            app.go_to_connection_list_with_selected(0);
        }
        _ => {}
    }
    KeyFlow::Continue
}

pub(crate) async fn start_forward<B: Backend + Write>(app: &mut App<B>, rule: &PortForward) {
    let Some(mut connection) = app.config.find_connection(&rule.connection_id).cloned() else {
        app.set_error(AppError::ConfigError(format!(
            "Device for '{}' not found",
            rule.get_display_name()
        )));
        return;
    };
    if let Err(e) = app.tunnels.prepare_device(&app.config, &mut connection) {
        app.set_error(e);
        return;
    }
    let result = app
        .port_forwarding_runtime
        .start_port_forward(rule, &connection)
        .await;
    let stored = app.config.find_port_forward_mut(&rule.id).unwrap();
    match result {
        Ok(()) => {
            stored.status = PortForwardStatus::Running;
            app.set_info(format!(
                "Port forward '{}' started",
                rule.get_display_name()
            ));
        }
        Err(e) => {
            stored.status = if matches!(e, AppError::UnknownHostKey { .. }) {
                PortForwardStatus::Stopped
            } else {
                PortForwardStatus::Failed(e.to_string())
            };
            app.offer_host_key_or_error(connection, e);
        }
    }
}

pub async fn handle_port_forwarding_form_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    use crate::config::manager::PortForwardType;
    use crate::ui::port_forwarding::FocusField;

    match key.code {
        KeyCode::Tab => {
            if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                state.form.next();
            } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                state.form.next();
            }
        }
        KeyCode::BackTab => {
            if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                state.form.prev();
            } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                state.form.prev();
            }
        }
        KeyCode::Down => {
            // Check if ForwardType is focused
            let is_forward_type_focused = if let AppMode::PortForwardingFormNew(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else if let AppMode::PortForwardingFormEdit(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else {
                false
            };

            if is_forward_type_focused {
                // Cycle forward through types
                if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                }
            } else {
                // Normal navigation
                if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                    state.form.next();
                } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                    state.form.next();
                }
            }
        }
        KeyCode::Up => {
            // Check if ForwardType is focused
            let is_forward_type_focused = if let AppMode::PortForwardingFormNew(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else if let AppMode::PortForwardingFormEdit(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else {
                false
            };

            if is_forward_type_focused {
                // Cycle backward through types
                if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                }
            } else {
                // Normal navigation
                if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                    state.form.prev();
                } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                    state.form.prev();
                }
            }
        }
        KeyCode::Left => {
            // Check if ForwardType is focused - cycle backward
            let is_forward_type_focused = if let AppMode::PortForwardingFormNew(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else if let AppMode::PortForwardingFormEdit(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else {
                false
            };

            if is_forward_type_focused {
                if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                }
            }
        }
        KeyCode::Right => {
            // Check if ForwardType is focused - cycle forward
            let is_forward_type_focused = if let AppMode::PortForwardingFormNew(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else if let AppMode::PortForwardingFormEdit(state) = &app.mode {
                state.form.focus == FocusField::ForwardType
            } else {
                false
            };

            if is_forward_type_focused {
                if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                }
            }
        }
        KeyCode::Enter => {
            // Save the port forward
            if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                let form_clone = state.form.clone();
                match save_port_forward(app, &form_clone, true).await {
                    Ok(_) => {
                        // After creating a new port forward, go to the end of the list
                        let new_index = app.config.port_forwards().len().saturating_sub(1);
                        let rule = app.config.port_forwards()[new_index].clone();
                        app.go_to_port_forwarding_list_with_selected(new_index)
                            .await;
                        return KeyFlow::StartForward(Box::new(rule));
                    }
                    Err(e) => {
                        app.error = Some(e);
                    }
                }
            } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                let form_clone = state.form.clone();
                let saved_selected = state.current_selected;
                match save_port_forward(app, &form_clone, false).await {
                    Ok(_) => {
                        // After editing, return to the same position
                        app.go_to_port_forwarding_list_with_selected(saved_selected)
                            .await;
                    }
                    Err(e) => {
                        app.error = Some(e);
                    }
                }
            }
        }
        KeyCode::Esc => {
            if let AppMode::PortForwardingFormNew(state) = &app.mode {
                app.go_to_port_forwarding_list_with_selected(state.current_selected)
                    .await;
            } else if let AppMode::PortForwardingFormEdit(state) = &app.mode {
                app.go_to_port_forwarding_list_with_selected(state.current_selected)
                    .await;
            } else {
                app.go_to_port_forwarding_list().await;
            }
        }
        KeyCode::Char(' ') => {
            if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                if state.form.focus == FocusField::ForwardType {
                    // Cycle through forward types with Space
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if state.form.focus == FocusField::Connection {
                    if app.config.connections().is_empty() {
                        app.error = Some(AppError::ConfigError(
                            "No connections available".to_string(),
                        ));
                    } else {
                        // Find the index of the currently selected connection, or default to 0
                        state.connection_selector.selected = app
                            .config
                            .connections()
                            .iter()
                            .position(|c| c.id == state.form.connection_id)
                            .unwrap_or(0);
                        state.connection_selector.show();
                        state.connection_selector.search.deactivate();
                    }
                } else {
                    // If not focused on Connection or ForwardType field, pass the key to text input
                    if let Some(textarea) = state.form.focused_textarea_mut() {
                        textarea.input(key);
                    }
                }
            } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode {
                if state.form.focus == FocusField::ForwardType {
                    // Cycle through forward types with Space
                    state.form.forward_type = match state.form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if state.form.focus == FocusField::Connection {
                    if app.config.connections().is_empty() {
                        app.error = Some(AppError::ConfigError(
                            "No connections available".to_string(),
                        ));
                    } else {
                        // Find the index of the currently selected connection, or default to 0
                        state.connection_selector.selected = app
                            .config
                            .connections()
                            .iter()
                            .position(|c| c.id == state.form.connection_id)
                            .unwrap_or(0);
                        state.connection_selector.show();
                        state.connection_selector.search.deactivate();
                    }
                } else {
                    // If not focused on Connection or ForwardType field, pass the key to text input
                    if let Some(textarea) = state.form.focused_textarea_mut() {
                        textarea.input(key);
                    }
                }
            } else {
                // Debug: We're not in the right mode
                app.info = Some("Not in port forwarding form mode".to_string());
            }
        }
        _ => {
            // Handle text input for focused field
            if let AppMode::PortForwardingFormNew(state) = &mut app.mode {
                if let Some(textarea) = state.form.focused_textarea_mut() {
                    textarea.input(key);
                }
            } else if let AppMode::PortForwardingFormEdit(state) = &mut app.mode
                && let Some(textarea) = state.form.focused_textarea_mut()
            {
                textarea.input(key);
            }
        }
    }
    KeyFlow::Continue
}

pub async fn handle_port_forwarding_form_connection_select_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    let Some(selector) = connection_selector_state(&mut app.mode) else {
        return KeyFlow::Continue;
    };

    let connections = app.config.connections();
    let mut filtered_indices =
        filter_connection_indices(connections, None, selector.search.query());
    let mut total_items = filtered_indices.len();

    if selector.search.is_on() {
        match key.code {
            KeyCode::Char(c) => {
                if let Some(query) = selector.search.query_mut() {
                    query.push(c);
                }
                *selector.connection_selected = 0;
                app.mark_redraw();
            }
            KeyCode::Backspace => {
                if let Some(query) = selector.search.query_mut() {
                    query.pop();
                }
                filtered_indices =
                    filter_connection_indices(connections, None, selector.search.query());
                total_items = filtered_indices.len();
                if total_items == 0 {
                    *selector.connection_selected = 0;
                } else if *selector.connection_selected >= total_items {
                    *selector.connection_selected = total_items - 1;
                }
                app.mark_redraw();
            }
            KeyCode::Esc => {
                if !selector.search.query().is_empty() {
                    selector.search.clear_query();
                    filtered_indices =
                        filter_connection_indices(connections, None, selector.search.query());
                    total_items = filtered_indices.len();
                    if total_items == 0 {
                        *selector.connection_selected = 0;
                    } else if *selector.connection_selected >= total_items {
                        *selector.connection_selected = total_items - 1;
                    }
                } else {
                    selector.search.deactivate();
                }
                app.mark_redraw();
            }
            KeyCode::Enter => {
                selector.search.deactivate();
                if total_items == 0 {
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                let idx = (*selector.connection_selected).min(total_items.saturating_sub(1));
                if let Some(conn_idx) = filtered_indices.get(idx)
                    && let Some(connection) = connections.get(*conn_idx)
                {
                    selector.form.connection_id = connection.id.clone();
                    selector.form.next();
                    *selector.select_connection_mode = false;
                    selector.search.deactivate();
                }
                app.mark_redraw();
            }
            _ => {}
        }
        return KeyFlow::Continue;
    }

    match key.code {
        KeyCode::Char('/') => {
            selector.search.activate();
            app.mark_redraw();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if total_items > 0 {
                if *selector.connection_selected == 0 {
                    *selector.connection_selected = total_items - 1;
                } else {
                    *selector.connection_selected -= 1;
                }
            }
            app.mark_redraw();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if total_items > 0 {
                *selector.connection_selected = (*selector.connection_selected + 1) % total_items;
            }
            app.mark_redraw();
        }
        KeyCode::Enter => {
            if total_items == 0 {
                app.mark_redraw();
                return KeyFlow::Continue;
            }

            let idx = (*selector.connection_selected).min(total_items.saturating_sub(1));
            if let Some(conn_idx) = filtered_indices.get(idx)
                && let Some(connection) = connections.get(*conn_idx)
            {
                selector.form.connection_id = connection.id.clone();
                selector.form.next();
                *selector.select_connection_mode = false;
                selector.search.deactivate();
                app.mark_redraw();
            }
        }
        KeyCode::Esc => {
            *selector.select_connection_mode = false;
            selector.search.deactivate();
            app.mark_redraw();
        }
        _ => {}
    }
    KeyFlow::Continue
}

struct ConnectionSelectorState<'a> {
    form: &'a mut PortForwardingForm,
    select_connection_mode: &'a mut bool,
    connection_selected: &'a mut usize,
    search: &'a mut SearchState,
}

fn connection_selector_state<'a>(mode: &'a mut AppMode) -> Option<ConnectionSelectorState<'a>> {
    match mode {
        AppMode::PortForwardingFormNew(state) | AppMode::PortForwardingFormEdit(state) => {
            Some(ConnectionSelectorState {
                form: &mut state.form,
                select_connection_mode: &mut state.connection_selector.showing,
                connection_selected: &mut state.connection_selector.selected,
                search: &mut state.connection_selector.search,
            })
        }
        _ => None,
    }
}

async fn save_port_forward<B: Backend + Write>(
    app: &mut App<B>,
    form: &PortForwardingForm,
    is_new: bool,
) -> Result<(), crate::error::AppError> {
    // Validate the form
    form.validate(app.config.connections())
        .map_err(crate::error::AppError::ValidationError)?;

    let local_port = form
        .get_local_port_value()
        .parse::<u16>()
        .map_err(|_| crate::error::AppError::ValidationError("Invalid local port".to_string()))?;

    // Parse service_port only if needed (not for Dynamic)
    let service_port = if matches!(
        form.forward_type,
        crate::config::manager::PortForwardType::Dynamic
    ) {
        0 // Default value for Dynamic, not used
    } else {
        form.get_service_port_value().parse::<u16>().map_err(|_| {
            crate::error::AppError::ValidationError("Invalid service port".to_string())
        })?
    };

    let display_name = if form.get_display_name_value().trim().is_empty() {
        None
    } else {
        Some(form.get_display_name_value().to_string())
    };

    let remote_bind_addr = if matches!(
        form.forward_type,
        crate::config::manager::PortForwardType::Remote
    ) {
        let addr = form.get_remote_bind_addr_value().trim();
        if addr.is_empty() {
            None
        } else {
            Some(addr.to_string())
        }
    } else {
        None
    };

    if is_new {
        // Create a new port forward with a new ID
        let port_forward = PortForward::new(
            form.connection_id.clone(),
            form.forward_type,
            form.get_local_addr_value().to_string(),
            local_port,
            form.get_service_host_value().to_string(),
            service_port,
            remote_bind_addr,
            display_name,
        );
        app.config.add_port_forward(port_forward)?;
    } else {
        // For updates, we need to use the existing ID
        let id = form.id.clone().ok_or_else(|| {
            crate::error::AppError::ValidationError(
                "Port forward ID is missing for update".to_string(),
            )
        })?;

        // Find the existing port forward to get created_at and status
        let existing = app
            .config
            .port_forwards()
            .iter()
            .find(|pf| pf.id == id)
            .ok_or_else(|| {
                crate::error::AppError::ConfigError("Port forward not found".to_string())
            })?
            .clone();

        // Create the updated port forward preserving ID, created_at, and status
        let port_forward = PortForward {
            id,
            connection_id: form.connection_id.clone(),
            forward_type: form.forward_type,
            local_addr: form.get_local_addr_value().to_string(),
            local_port,
            service_host: form.get_service_host_value().to_string(),
            service_port,
            remote_bind_addr,
            display_name,
            created_at: existing.created_at,
            status: existing.status,
        };

        app.config.save_edited_port_forward(port_forward)?;
        app.info = Some("Port forward updated successfully".to_string());
    }

    Ok(())
}

/// Sync port forwarding status with the runtime to ensure consistency
pub async fn sync_port_forwarding_status<B: Backend + Write>(app: &mut App<B>) {
    for pf in app.config.port_forwards_mut() {
        let is_running = app.port_forwarding_runtime.is_running(&pf.id).await;
        match pf.status {
            PortForwardStatus::Running => {
                if !is_running {
                    // Port forward was marked as running but isn't actually running
                    pf.status = PortForwardStatus::Stopped;
                }
            }
            PortForwardStatus::Stopped => {
                if is_running {
                    // Port forward is actually running but marked as stopped
                    pf.status = PortForwardStatus::Running;
                }
            }
            PortForwardStatus::Failed(_) => {
                // Keep failed status as is - user can retry manually
            }
        }
    }
}

pub async fn handle_port_forward_delete_confirmation_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    if let AppMode::PortForwardDeleteConfirmation {
        port_forward_id,
        current_selected,
        ..
    } = &app.mode
    {
        let port_forward_id = port_forward_id.clone();
        let current_selected = *current_selected;

        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                // Confirm deletion
                match app.config.save_removed_port_forward(&port_forward_id) {
                    Ok(()) => {
                        app.info = Some("Port forward deleted successfully".to_string());
                        let last = app.config.port_forwards().len().saturating_sub(1);
                        app.go_to_port_forwarding_list_with_selected(current_selected.min(last))
                            .await;
                    }
                    Err(e) => {
                        app.error = Some(e);
                        app.go_to_port_forwarding_list_with_selected(current_selected)
                            .await;
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                // Cancel deletion
                app.go_to_port_forwarding_list_with_selected(current_selected)
                    .await;
            }
            _ => {}
        }
    }
    KeyFlow::Continue
}
