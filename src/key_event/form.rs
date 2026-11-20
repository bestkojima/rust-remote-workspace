use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Backend;

use super::KeyFlow;
use crate::config::manager::AuthMethod;
use crate::config::manager::Connection;
use crate::error::AppError;
use crate::{App, AppMode};

pub async fn handle_form_new_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    let save_offline = key.code == KeyCode::Char('s') && key.modifiers == KeyModifiers::CONTROL;
    match key.code {
        KeyCode::Esc => {
            if let AppMode::FormNew {
                current_selected, ..
            } = &app.mode
            {
                let current_selected = *current_selected;
                app.go_to_connection_list_with_selected(current_selected);
            }
        }
        KeyCode::Tab | KeyCode::Down => {
            if let AppMode::FormNew { form, .. } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let AppMode::FormNew { form, .. } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+L: Load from SSH config
            if let AppMode::FormNew {
                auto_auth, form, ..
            } = &mut app.mode
            {
                let host_pattern = form.get_host_value().trim().to_string();

                if host_pattern.is_empty() {
                    app.error = Some(AppError::ValidationError(
                        "Please enter a hostname to import".to_string(),
                    ));
                    return KeyFlow::Continue;
                }

                match crate::config::ssh_config::query_ssh_config(&host_pattern) {
                    Ok(ssh_host) => {
                        // Populate form fields from SSH config
                        // Clear and set hostname
                        form.host.delete_line_by_head();
                        form.host.delete_line_by_end();
                        form.host.insert_str(&ssh_host.hostname);

                        // Set port if available
                        if let Some(port) = ssh_host.port {
                            form.port.delete_line_by_head();
                            form.port.delete_line_by_end();
                            form.port.insert_str(port.to_string());
                        } else {
                            app.error = Some(AppError::ValidationError(
                                "Host not found in SSH config".to_string(),
                            ));
                            return KeyFlow::Continue;
                        }

                        // Set username if available
                        if let Some(user) = &ssh_host.user {
                            form.username.delete_line_by_head();
                            form.username.delete_line_by_end();
                            form.username.insert_str(user);
                        } else {
                            app.error = Some(AppError::ValidationError(
                                "Host not found in SSH config".to_string(),
                            ));
                            return KeyFlow::Continue;
                        }

                        // A reimport replaces the previous identity. Without an explicit
                        // key, return to auto-auth instead of keeping an old key and phrase.
                        let identity = ssh_host
                            .identity_file
                            .as_ref()
                            .and_then(|paths| paths.first());
                        *auto_auth = identity.is_none();
                        form.set_key_path(
                            identity
                                .map(|p| p.to_string_lossy())
                                .as_deref()
                                .unwrap_or(""),
                        );

                        // Set display name to the original host pattern
                        form.display_name.delete_line_by_head();
                        form.display_name.delete_line_by_end();
                        form.display_name.insert_str(&host_pattern);

                        app.info = Some("SSH config loaded successfully".to_string());
                    }
                    Err(e) => {
                        app.error = Some(e);
                    }
                }
            }
        }
        code if code == KeyCode::Enter || save_offline => {
            if let AppMode::FormNew {
                auto_auth, form, ..
            } = &mut app.mode
            {
                match form.validate() {
                    Ok(_) => {
                        let user = form.get_username_value().trim().to_string();

                        let auth_method = form.entered_auth().unwrap_or(if *auto_auth {
                            AuthMethod::AutoLoadKey
                        } else {
                            AuthMethod::None
                        });

                        let mut conn = Connection::new(
                            form.get_host_value().trim().to_string(),
                            form.get_port_value()
                                .parse::<u16>()
                                .unwrap_or(app.config.default_port()),
                            user,
                            auth_method,
                        );
                        if !form.get_display_name_value().trim().is_empty() {
                            conn.set_display_name(form.get_display_name_value().trim().to_string());
                        }

                        if save_offline {
                            match app.config.add_connection(conn) {
                                Ok(()) => app.go_to_connection_list_with_selected(
                                    app.config.connections().len() - 1,
                                ),
                                Err(e) => app.set_error(e),
                            }
                            return KeyFlow::Continue;
                        }
                        let return_from = crate::ConnectingSource::FormNew {
                            auto_auth: *auto_auth,
                            form: form.clone(),
                        };
                        app.begin_connection(conn, 0, return_from);
                    }
                    Err(msg) => {
                        app.error = Some(AppError::ValidationError(msg));
                    }
                }
            }
        }
        _ => {
            // Use textarea's built-in input handling for all other keys
            if let AppMode::FormNew { form, .. } = &mut app.mode {
                form.input(key);
            }
        }
    }
    KeyFlow::Continue
}

pub async fn handle_form_edit_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            if let AppMode::FormEdit {
                current_selected, ..
            } = &app.mode
            {
                let current_selected = *current_selected;
                app.go_to_connection_list_with_selected(current_selected);
            }
        }
        KeyCode::Tab | KeyCode::Down => {
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Enter => {
            if let AppMode::FormEdit {
                form,
                original,
                current_selected,
            } = &mut app.mode
            {
                if let Err(e) = form.validate() {
                    app.error = Some(AppError::ValidationError(e));
                    return KeyFlow::Continue;
                }

                let mut updated = original.clone();
                updated.host = form.get_host_value().trim().to_string();
                let parsed_port = match form.get_port_value().parse::<u16>() {
                    Ok(p) => p,
                    Err(_) => {
                        app.error = Some(AppError::ValidationError("Port must be a number".into()));
                        return KeyFlow::Continue;
                    }
                };
                updated.port = parsed_port;
                updated.username = form.get_username_value().trim().to_string();
                if let Some(auth) = form.entered_auth() {
                    if updated.auth_method != auth {
                        updated.auth_method = auth;
                    }
                } else if !matches!(
                    &original.auth_method,
                    AuthMethod::AutoLoadKey | AuthMethod::None
                ) {
                    app.error = Some(AppError::ValidationError(
                        "Enter a login password or a private key path.".into(),
                    ));
                    return KeyFlow::Continue;
                }
                updated.display_name = form.get_display_name_value().trim().to_string();

                if let Err(e) = updated.validate() {
                    app.error = Some(e);
                    return KeyFlow::Continue;
                }

                match app.config.save_edited_connection(updated) {
                    Ok(_) => {
                        let current_selected = *current_selected;
                        app.go_to_connection_list_with_selected(current_selected);
                    }
                    Err(e) => app.error = Some(e),
                }
            }
        }
        _ => {
            // Use textarea's built-in input handling for all other keys
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                form.input(key);
            }
        }
    }
    KeyFlow::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};

    #[tokio::test]
    async fn key_passphrase_from_the_shared_row_roundtrips_without_becoming_a_password() {
        use crate::config::manager::ConfigManager;
        use crate::key_event::handle_paste_event;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let terminal = Terminal::with_options(
            CrosstermBackend::new(Vec::<u8>::new()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap();
        let mut app = App::with_config_path(terminal, Some(&path)).unwrap();
        app.go_to_form_new();
        // Keep the original six-field Tab order: password is fourth, key path fifth.
        for field in ["localhost", "2222", "fixture", "account password only"] {
            handle_paste_event(&mut app, field).await;
            handle_form_new_key(&mut app, KeyCode::Tab.into()).await;
        }
        handle_paste_event(&mut app, "fixture-key-A").await;
        handle_form_new_key(&mut app, KeyCode::BackTab.into()).await;
        handle_paste_event(&mut app, "  鑰匙 passphrase  ").await;
        handle_form_new_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        )
        .await;
        assert!(app.error.is_none());
        let saved = || ConfigManager::with_path(&path).unwrap().connections()[0].clone();
        let first = saved();
        let check = |conn: &Connection, path: &str, expected: Option<&str>| {
            let AuthMethod::PublicKey {
                private_key_path,
                passphrase,
            } = &conn.auth_method
            else {
                panic!("Expected key authentication; private key passphrases must stay local.")
            };
            assert_eq!(private_key_path, path);
            assert!(passphrase.as_ref().map(|p| p.as_str()) == expected);
        };
        check(&first, "fixture-key-A", Some("  鑰匙 passphrase  "));
        let edit = |app: &mut App<_>, dev: Connection| app.go_to_form_edit((&dev).into(), dev);
        edit(&mut app, first);
        for _ in 0..3 {
            handle_form_edit_key(&mut app, KeyCode::Tab.into()).await;
        }
        // Clearing a bound passphrase is explicit and persists as None.
        handle_form_edit_key(
            &mut app,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL),
        )
        .await;
        handle_form_edit_key(&mut app, KeyCode::Enter.into()).await;
        check(&saved(), "fixture-key-A", None);
        edit(&mut app, saved());
        for _ in 0..3 {
            handle_form_edit_key(&mut app, KeyCode::Tab.into()).await;
        }
        handle_paste_event(&mut app, "replacement fixture").await;
        handle_form_edit_key(&mut app, KeyCode::Enter.into()).await;
        check(&saved(), "fixture-key-A", Some("replacement fixture"));
        // Editing the private key path drops the previous passphrase and its undo history.
        edit(&mut app, saved());
        for _ in 0..4 {
            handle_form_edit_key(&mut app, KeyCode::Tab.into()).await;
        }
        handle_form_edit_key(&mut app, KeyCode::Char('B').into()).await;
        handle_form_edit_key(&mut app, KeyCode::Enter.into()).await;
        check(&saved(), "fixture-key-AB", None);
    }

    #[tokio::test]
    async fn saving_the_edit_form_keeps_host_identity_and_login_method() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        let terminal = Terminal::with_options(
            CrosstermBackend::new(Vec::<u8>::new()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap();
        let mut app = App::with_config_path(terminal, Some(&path)).unwrap();
        for (i, auth) in [
            AuthMethod::AutoLoadKey,
            AuthMethod::None,
            AuthMethod::Password("  fixture with spaces  ".to_string().into()),
            AuthMethod::PublicKey {
                private_key_path: "test-key".into(),
                passphrase: Some("local fixture".to_string().into()),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let mut conn =
                Connection::new("localhost".into(), 22, format!("user-{i}"), auth.clone());
            conn.public_key = Some(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPkehTOVkSIRyzO+uCOhXCbtkWCczR3NfAIXj6HNJ5DE"
                    .into(),
            );
            app.config.add_connection(conn.clone()).unwrap();
            let mut form = crate::ui::ConnectionForm::from(&conn);
            form.display_name.insert_str("-edit");
            app.go_to_form_edit(form, conn.clone());
            handle_form_edit_key(&mut app, KeyCode::Enter.into()).await;
            assert!(app.error.is_none());
            let reloaded = crate::config::manager::ConfigManager::with_path(&path).unwrap();
            let edited = reloaded.find_connection(&conn.id).unwrap();
            assert_eq!(edited.display_name, "localhost-edit");
            assert_eq!(edited.public_key, conn.public_key);
            assert_eq!(edited.auth_method, auth);
        }
    }
}
