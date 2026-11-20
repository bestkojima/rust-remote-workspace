use super::*;
use crate::config::manager::ConfigManager;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListState, Paragraph, Wrap},
};

fn display(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
fn human(size: u64) -> String {
    if size >= 1 << 30 {
        format!("{:.1} GiB", size as f64 / (1u64 << 30) as f64)
    } else if size >= 1 << 20 {
        format!("{:.1} MiB", size as f64 / (1u64 << 20) as f64)
    } else if size >= 1 << 10 {
        format!("{:.1} KiB", size as f64 / 1024.)
    } else {
        format!("{size} B")
    }
}

fn copy_notice(files: usize, bytes: u64) -> Option<String> {
    let label = if bytes >= 100 * (1 << 20) {
        format!("Large transfer ({})", human(bytes))
    } else if files >= 100 {
        format!("Batch transfer ({files} files)")
    } else {
        return None;
    };
    Some(format!(
        "{label}. A retry skips unchanged files and restarts any interrupted file."
    ))
}

fn route(p: &SyncProfile, endpoint: Option<&Connection>) -> [String; 2] {
    let local = display(&p.spec.local_root.to_string_lossy());
    let peer = endpoint
        .map(|d| {
            format!(
                "{} [{}@{}]",
                display(&d.display_name),
                display(&d.username),
                display(&d.host_port())
            )
        })
        .unwrap_or_else(|| "(missing device)".into());
    let remote = format!("{peer}:{}", display(&p.spec.remote_root));
    let (src, dst) = match p.spec.direction {
        Direction::Upload => (local, remote),
        Direction::Download => (remote, local),
    };
    [format!("From  {src}"), format!("To    {dst}")]
}
fn list(f: &mut Frame, a: Rect, rows: Vec<Line<'_>>, selected: usize) {
    f.render_stateful_widget(
        List::new(rows)
            .highlight_symbol("> ")
            .highlight_style(Style::new().reversed()),
        a,
        &mut ListState::default().with_selected(Some(selected)),
    );
}
fn wrap_fits(lines: &[Line<'_>], width: u16, height: u16) -> bool {
    let width = usize::from(width);
    width > 0
        && lines
            .iter()
            .map(|l| l.width().div_ceil(width).max(1))
            .sum::<usize>()
            <= usize::from(height)
}
impl SyncDesk {
    pub(crate) fn draw(&self, f: &mut Frame, area: Rect, cfg: &ConfigManager) {
        let a = area.inner(Margin::new(1, 0));
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .areas(a);
        let p = cfg.sync_profiles().get(self.selected);
        let device = |id: &str| {
            cfg.find_connection(id)
                .map_or("(missing device)", |d| &d.display_name)
        };
        let (heading, actions) = match &self.state {
            Screen::List => {
                if cfg.sync_profiles().is_empty() {
                    f.render_widget(Paragraph::new("No folder pairs yet. In Files, choose the local and remote directories and press F7, or add a profile here.").wrap(Wrap{trim:false}),body);
                } else {
                    let [rows, detail] =
                        Layout::vertical([Constraint::Min(2), Constraint::Length(6)]).areas(body);
                    let items = cfg
                        .sync_profiles()
                        .iter()
                        .map(|p| {
                            Line::from(vec![
                                Span::styled(
                                    format!(
                                        "{:<10}",
                                        if p.spec.direction == Direction::Upload {
                                            "Upload"
                                        } else {
                                            "Download"
                                        }
                                    ),
                                    Style::new().cyan(),
                                ),
                                Span::raw(display(&p.name)),
                            ])
                        })
                        .collect();
                    list(f, rows, items, self.selected);
                    if let Some(p) = p {
                        let [src, dst] = route(p, cfg.find_connection(&p.device_id));
                        f.render_widget(
                            Paragraph::new(vec![
                                Line::raw(src),
                                Line::raw(dst),
                                Line::raw(if p.saved_jail.is_some() {
                                    "Saved jail grant / u Revoke"
                                } else {
                                    ""
                                }),
                                Line::raw(""),
                                Line::raw(p.history.last().map_or("No runs yet", |l| &l.outcome)),
                            ])
                            .wrap(Wrap { trim: false }),
                            detail,
                        );
                    }
                }
                (
                    "Folder sync",
                    if cfg.sync_profiles().is_empty() {
                        "n  New profile    F2  Pick directories in Files    r  rclone tool"
                    } else {
                        "Enter  Preview    n/e/d  New/edit/remove    h  Run history    r  rclone tool    q  Quit"
                    },
                )
            }
            Screen::Edit(e) => {
                let names = [
                    "Name",
                    "Device  < >",
                    "Local directory",
                    "Remote directory",
                    "Direction  < >",
                    "Exclude  (;)",
                ];
                let vals = [
                    e.name.value(),
                    device(&e.profile.device_id),
                    e.local.value(),
                    e.remote.value(),
                    if e.profile.spec.direction == Direction::Upload {
                        "Upload   |   Local -> device"
                    } else {
                        "Download   |   Device -> local"
                    },
                    e.excludes.value(),
                ];
                let mut y = body.y;
                for (i, (name, val)) in names.iter().zip(vals.iter()).enumerate() {
                    if y >= body.bottom() {
                        break;
                    }
                    let [label, field] = Layout::horizontal([
                        Constraint::Length(if body.width > 52 { 19 } else { 15 }),
                        Constraint::Min(0),
                    ])
                    .areas(Rect::new(body.x, y, body.width, 1));
                    f.render_widget(
                        Paragraph::new(*name).style(if e.focus == i {
                            Style::new().cyan().bold()
                        } else {
                            Style::new().gray()
                        }),
                        label,
                    );
                    let text = match i {
                        0 => Some(&e.name),
                        2 => Some(&e.local),
                        3 => Some(&e.remote),
                        5 => Some(&e.excludes),
                        _ => None,
                    };
                    if e.focus == i
                        && let Some(text) = text
                    {
                        f.render_widget(&text.0, field);
                    } else {
                        f.render_widget(
                            Paragraph::new(display(val)).style(if e.focus == i {
                                Style::new().black().on_cyan()
                            } else {
                                Style::default()
                            }),
                            field,
                        );
                    }
                    y += if body.height >= 17 { 2 } else { 1 };
                }
                if y + 1 < body.bottom() {
                    f.render_widget(
                        Paragraph::new(
                            "Manual copy / Keeps target-only files / Reviews name conflicts",
                        )
                        .wrap(Wrap { trim: false }),
                        Rect::new(body.x, y, body.width, body.bottom() - y),
                    );
                }
                (
                    "Sync profile",
                    "Tab  Next field    Left/Right  Choose device/direction    Ctrl+A  Select text    Enter  Save    Esc  Back",
                )
            }
            Screen::Tool(field) => {
                let [label, input, _] = Layout::vertical([
                    Constraint::Length(2),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(body);
                f.render_widget(
                    Paragraph::new("rclone executable (absolute path or command on PATH)"),
                    label,
                );
                f.render_widget(&field.0, input);
                ("Sync tool", "Enter  Save    Esc  Back")
            }
            Screen::Delete => {
                f.render_widget(
                    Paragraph::new(format!(
                        "Remove profile {} and its run history? Both directories are kept.",
                        p.map_or("", |p| &p.name)
                    ))
                    .wrap(Wrap { trim: false }),
                    body,
                );
                ("Remove sync profile", "y  Remove profile    Esc  Keep")
            }
            Screen::Working(j) => {
                let v = j.progress.borrow();
                let [src, dst] = route(&j.profile, Some(&j.device));
                f.render_widget(
                    Paragraph::new(vec![
                        Line::raw(src),
                        Line::raw(dst),
                        Line::raw(""),
                        Line::styled(display(&v.message), Style::new().cyan()),
                        Line::raw(format!(
                            "{} / {} files   |   {}",
                            v.completed,
                            v.total,
                            human(v.bytes)
                        )),
                        Line::raw(if j.cancel.is_cancelled() {
                            "Stopping..."
                        } else {
                            ""
                        }),
                    ])
                    .wrap(Wrap { trim: false }),
                    body,
                );
                (
                    match j.stage {
                        Stage::Identity => "Checking SSH identity",
                        Stage::Copy => "Copy in progress",
                        Stage::Scan => "Scanning / Dry run",
                    },
                    "x / Esc  Cancel task    F1-F5  Switch page",
                )
            }
            Screen::Grant {
                profile,
                device,
                visible,
            } => {
                let jail = profile.spec.remote_root == "/";
                let mut lines = vec![
                    Line::styled(
                        format!(
                            "{}@{}   {}",
                            display(&device.username),
                            display(&device.host_port()),
                            display(&profile.spec.remote_root)
                        ),
                        Style::new().cyan(),
                    ),
                    Line::raw(""),
                    Line::styled("e  Exclusive access for this run", Style::new().bold()),
                    Line::raw(
                        "I control this remote tree, its ancestors and staging until this run ends. No other writer can change or replace them.",
                    ),
                ];
                if jail {
                    lines.extend([
                    Line::raw(""),Line::styled("j  Save administrator-verified jail",Style::new().bold()),
                    Line::raw("The administrator confined this SFTP account to exactly / for this task. Other writers cannot take over its .rrw-sync-* staging."),
                ]);
                }
                lines.extend([
                    Line::raw(""),
                    Line::styled(
                        "If neither condition holds, cancel. No directory has been read.",
                        Style::new().yellow(),
                    ),
                ]);
                let lines = hard_wrap(lines, body.width);
                let fits =
                    body.width >= 25 && body.height as usize >= lines.len() && footer.height > 0;
                visible.set(fits);
                if fits {
                    f.render_widget(Paragraph::new(lines), body);
                } else {
                    f.render_widget(
                        Paragraph::new(
                            "Enlarge the window to read the access conditions, or press Esc.",
                        )
                        .wrap(Wrap { trim: false }),
                        body,
                    );
                }
                (
                    "Authorize remote directory",
                    if jail {
                        "e  Confirm this run    j  Confirm saved jail    Esc  Cancel"
                    } else {
                        "e  Confirm this run    Esc  Cancel"
                    },
                )
            }
            Screen::Review(r) if r.confirm => {
                let [src, dst] = route(&r.profile, Some(&r.device));
                let (n, overwritten, b) = r.selection();
                let mut lines = vec![
                    Line::raw(src),
                    Line::raw(dst),
                    Line::raw(""),
                    Line::styled(
                        format!("Files: {n}   Bytes: {b}   Replacements: {overwritten}"),
                        Style::new().yellow().bold(),
                    ),
                    Line::raw(""),
                    Line::raw(
                        "Only the reviewed files will be copied. Target-only files are kept.",
                    ),
                ];
                if let Some(notice) = copy_notice(n, b) {
                    lines.push(Line::styled(notice, Style::new().yellow()));
                }
                // Hard wrap the decision lines ourselves. The approval hotkey is enabled
                // only when the source, target and summary have all actually fitted.
                let lines = hard_wrap(lines, body.width);
                let visible = body.width >= 25
                    && usize::from(body.height) >= lines.len()
                    && footer.width >= 25
                    && footer.height > 0;
                r.visible.set(visible);
                if visible {
                    f.render_widget(Paragraph::new(lines), body);
                } else {
                    f.render_widget(
                        Paragraph::new("Enlarge the window to review and confirm.")
                            .wrap(Wrap { trim: false }),
                        body,
                    );
                }
                (
                    "Confirm copy",
                    "y  Copy these files    Esc  Return to preview",
                )
            }
            Screen::Review(r) => {
                let (n, overwritten, b) = r.selection();
                let [summary, entries, focus] = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Min(1),
                    Constraint::Length(3),
                ])
                .areas(body);
                let text = format!(
                    "{} changes   |   {} unchanged   |   {} skipped\nSelected: {n}   |   {}   |   Replacements: {overwritten}",
                    r.plan.entries.len(),
                    r.plan.unchanged,
                    r.plan.skipped,
                    human(b)
                );
                f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), summary);
                let count = usize::from(entries.height).max(1);
                let start = (r.selected / count) * count;
                let rows = r
                    .plan
                    .entries
                    .iter()
                    .skip(start)
                    .take(count)
                    .map(|c| {
                        let (tag, color) = if c.kind == ChangeKind::New {
                            ("NEW", Color::Green)
                        } else if r.approved.contains(&c.path) {
                            ("REPLACE", Color::Yellow)
                        } else {
                            ("KEEP", Color::DarkGray)
                        };
                        Line::from(vec![
                            Span::styled(format!("{tag:<9}"), Style::new().fg(color)),
                            Span::raw(format!("{:>10}   {}", human(c.bytes), display(&c.path))),
                        ])
                    })
                    .collect();
                list(f, entries, rows, r.selected - start);
                let lines = if let Some(c) = r.plan.entries.get(r.selected) {
                    vec![
                        Line::raw(display(&c.path)),
                        Line::styled(
                            if c.kind == ChangeKind::New {
                                "New file"
                            } else if c.can_overwrite {
                                "Space toggles replacement of this reviewed version."
                            } else {
                                "Original kept / Safe replacement is unavailable on this target."
                            },
                            Style::new().gray(),
                        ),
                    ]
                } else {
                    vec![Line::raw("No changes to copy.")]
                };
                let lines = hard_wrap(lines, focus.width);
                r.visible.set(wrap_fits(&lines, focus.width, focus.height));
                f.render_widget(Paragraph::new(lines), focus);
                (
                    "Copy preview / No files written",
                    if n == 0 {
                        "Up/Down  Select    Space  Toggle eligible replacement    Esc  Discard preview"
                    } else {
                        "Up/Down  Select    Space  Toggle eligible replacement    Enter  Review copy    Esc  Discard"
                    },
                )
            }
            Screen::Identity(_, e) => {
                f.render_widget(
                    Paragraph::new(e.to_string()).wrap(Wrap { trim: false }),
                    body,
                );
                (
                    "SSH identity needs attention",
                    "Enter  Inspect host key    Esc  Back",
                )
            }
            Screen::Result(s) => {
                f.render_widget(Paragraph::new(s.as_str()).wrap(Wrap { trim: false }), body);
                ("Sync result", "Enter / Esc  Profiles")
            }
            Screen::History { selected } => {
                if let Some(p) = p {
                    let rows = p
                        .history
                        .iter()
                        .map(|v| {
                            Line::raw(format!(
                                "{}  {}",
                                v.at.format("%Y-%m-%d %H:%M UTC"),
                                v.outcome
                            ))
                        })
                        .collect();
                    let [l, detail] =
                        Layout::vertical([Constraint::Min(1), Constraint::Length(5)]).areas(body);
                    list(f, l, rows, *selected);
                    f.render_widget(
                        Paragraph::new(
                            p.history
                                .get(*selected)
                                .map_or("No runs yet", |l| &l.outcome),
                        )
                        .wrap(Wrap { trim: false }),
                        detail,
                    );
                }
                (
                    "Run history / Latest 12",
                    "Up/Down  Read entry    Esc  Profiles",
                )
            }
        };
        f.render_widget(
            Paragraph::new(heading).style(Style::new().bold().cyan()),
            header,
        );
        f.render_widget(Paragraph::new(actions).wrap(Wrap { trim: false }), footer);
    }
}
fn hard_wrap(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let mut out = vec![];
    let width = usize::from(width.max(1));
    for line in lines {
        let mut used = 0;
        let mut text = String::new();
        for c in line.to_string().chars() {
            let w = Span::raw(c.to_string()).width();
            if used + w > width && !text.is_empty() {
                out.push(Line::raw(std::mem::take(&mut text)).style(line.style));
                used = 0;
            }
            text.push(c);
            used += w;
        }
        out.push(Line::raw(text).style(line.style));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_transfer_notice_uses_the_approved_count_and_bytes() {
        let threshold = 100 * (1 << 20);
        assert!(copy_notice(99, threshold - 1).is_none());
        assert!(
            copy_notice(100, 300)
                .unwrap()
                .contains("Batch transfer (100 files)")
        );
        assert!(copy_notice(1, threshold).unwrap().contains("100.0 MiB"));
    }
}
