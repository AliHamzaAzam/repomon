//! Rendering for the views. Flat, brutalist layout — light/heavy rules, single-char glyphs,
//! two-space indents — with a semantic color palette over the top (status colors + a
//! configurable accent; see [`crate::theme`]). `accent = "mono"` restores the no-color look.

use chrono::{DateTime, Local, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use repomon_core::model::{Commit, DirtyState, Lane, LaneId, RepoId};

use crate::app::{AgField, App, ClickZone};
use crate::keybinds::View;
use crate::notify::NotifKind;
use crate::theme;

const FLEET_KEYS: &str =
    "↑↓ ↵ open · click select · dbl terminal  ·  n new · e spawn · t term  ·  a add-repo · A agents · , settings · d del  ·  / filter · f find · ! urgent · g needs-you · C auto-cont  ·  2 timeline · 3 sessions · 4 search  ·  spc grid · q";
const SPLIT_KEYS: &str =
    "↑↓ lane · tab session  ·  click focus · dbl terminal · ↵ open · → focus · i quick-type  ·  e spawn · o adopt · C auto-cont  ·  ←/esc back";
const SPLIT_INSERT_KEYS: &str = "keys → agent (esc · ⇧⇥ · ^C sent)  ·  ^O / click-out blur";
const FOCUS_CMD_KEYS: &str =
    "↵/→ open (real terminal) · i quick-type · tab agent · PgUp scroll  ·  e spawn · o adopt · t term · s stop  ·  g next · f find  ·  ←/esc back";
const FOCUS_INSERT_KEYS: &str = "keys → agent (esc · ⇧⇥ · ^C sent)  ·  ^O command-mode";
const GRID_KEYS: &str =
    "←→ move · click focus (type in place) · dbl terminal · ↵ open  ·  e spawn · s stop · p pin · g next · f find  ·  spc/esc fleet · q quit";
const GRID_INSERT_KEYS: &str = "keys → agent (esc · ⇧⇥ · ^C sent)  ·  ^O / click-out blur";
const NEWLANE_KEYS: &str =
    "↑↓ repo · tab agent · ^a manage  ·  type branch · ↵ create + spawn  ·  esc cancel";

/// Record a clickable lane region for the mouse handler to hit-test (see `App::handle_click`).
fn click_zone(app: &App, rect: Rect, lane: LaneId, interactive: bool) {
    app.click_zones.borrow_mut().push(ClickZone {
        rect,
        lane,
        interactive,
    });
}

/// Render the current view.
pub fn render(f: &mut Frame, app: &App) {
    // Clickable lane regions are recomputed each frame by the per-view renderers below.
    app.click_zones.borrow_mut().clear();
    match app.view {
        View::Fleet => render_fleet(f, app),
        View::Split => render_split(f, app),
        View::Focus => render_focus(f, app),
        View::Grid => render_grid(f, app),
        View::NewLane => render_new_lane(f, app),
        View::Timeline => render_timeline(f, app),
        View::Sessions => render_sessions(f, app),
        View::Search => render_search(f, app),
        View::AddRepo => render_addrepo(f, app),
        View::Agents => render_agents(f, app),
        View::Settings => render_settings(f, app),
        View::Notifications => render_notifications(f, app),
        View::SpawnPick => render_spawn_pick(f, app),
        View::LaneJump => render_lane_jump(f, app),
    }
}

const AGENTS_KEYS: &str = "↑↓ select  ·  n new · e edit · d delete · * default  ·  esc back";
const AGENTS_EDIT_KEYS: &str = "tab switch field · type  ·  ↵ save · esc cancel";

/// The agent manager: a list of agents (built-ins read-only, customs editable) with an
/// inline add/edit form. `★` marks the default; `✓`/`✗` is PATH detection.
fn render_agents(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, "REPOMON · AGENTS", &fmt_clock(), app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );

    if app.ag_editing {
        let title = if app.ag_is_new {
            "new agent"
        } else {
            "edit agent"
        };
        let name_cur = if app.ag_field == AgField::Name {
            "_"
        } else {
            ""
        };
        let cmd_cur = if app.ag_field == AgField::Command {
            "_"
        } else {
            ""
        };
        let mut lines = vec![
            Line::raw(""),
            Line::from(Span::styled(format!("  {title}"), app.theme.bold())),
            Line::raw(""),
            Line::raw(format!("  name      {}{name_cur}", app.ag_name)),
            Line::raw(format!("  command   {}{cmd_cur}", app.ag_command)),
            Line::raw(""),
            Line::from(Span::styled(
                "  the command runs in the lane's worktree; the name is what you pick in New Lane"
                    .to_string(),
                app.theme.dim(),
            )),
        ];
        if !app.status.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!("  {}", app.status)));
        }
        f.render_widget(Paragraph::new(lines), rows[1]);
        f.render_widget(footer(AGENTS_EDIT_KEYS, app), rows[2]);
        return;
    }

    let h = (rows[1].height as usize).max(1);
    let start = app.agents_selected.saturating_sub(h.saturating_sub(1));
    let mut lines = Vec::new();
    if app.agents.is_empty() {
        lines.push(Line::raw(
            "  (no agents — press n to add a custom launch command)".to_string(),
        ));
    }
    for (i, a) in app.agents.iter().enumerate().skip(start).take(h) {
        let star = if a.default { "★" } else { " " };
        let mark = if a.detected { "✓" } else { "✗" };
        let kind = if a.custom { "custom " } else { "builtin" };
        // Spell out a failed PATH check so the bare ✗ isn't cryptic.
        let note = if a.detected { "" } else { "   not on PATH" };
        let mut line = Line::raw(format!(
            "  {star} {mark} {:<16} {kind}  $ {}{note}",
            trunc(&a.name, 16),
            a.command
        ));
        if i == app.agents_selected {
            line = line.style(app.theme.selected());
        }
        lines.push(line);
    }
    if !app.status.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::raw(format!("  {}", app.status)));
    }
    f.render_widget(Paragraph::new(lines), rows[1]);
    f.render_widget(footer(AGENTS_KEYS, app), rows[2]);
}

const SETTINGS_KEYS: &str = "↑↓ / click a row  ·  ←/→ change · space/↵ toggle/edit  ·  esc back";
const SETTINGS_EDIT_KEYS: &str = "type the continue message  ·  ↵ save · esc cancel";

/// The settings view: accent color (cycles, live preview), auto-continue on/off, and the
/// continue message — each persisted to `~/.config/repomon/config.toml` via the daemon.
fn render_settings(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, "REPOMON · SETTINGS", &fmt_clock(), app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );

    let s = &app.settings;
    let editing = app.settings_editing;
    let cur = |i: usize| {
        if editing && app.settings_idx == i {
            "_"
        } else {
            ""
        }
    };
    let default_agent = if s.default_agent.is_empty() {
        "(first listed)".to_string()
    } else {
        s.default_agent.clone()
    };
    let onoff = |b: bool| if b { "on" } else { "off" }.to_string();
    let items: [(&str, String, &str); 12] = [
        ("accent", s.accent.clone(), "←/→ cycle · live"),
        ("default agent", default_agent, "←/→ cycle"),
        ("auto-continue", onoff(s.auto_continue), "space toggles"),
        (
            "continue message",
            format!("{}{}", s.auto_continue_message, cur(3)),
            "↵ edit",
        ),
        (
            "worktree template",
            format!("{}{}", s.worktree_template, cur(4)),
            "↵ edit",
        ),
        (
            "ask which agent on spawn",
            onoff(s.spawn_prompt),
            "space toggles",
        ),
        // Notifications group — the master switch then per-trigger toggles (indented).
        ("notifications", onoff(s.notify_enabled), "master · space"),
        ("  · needs you", onoff(s.notify_needs_you), "space toggles"),
        (
            "  · usage limit",
            onoff(s.notify_rate_limited),
            "space toggles",
        ),
        ("  · auto-resumed", onoff(s.notify_resumed), "space toggles"),
        ("  · went idle", onoff(s.notify_idle), "space toggles"),
        ("  · sound", onoff(s.notify_sound), "space toggles"),
    ];
    // Size the columns to the content so values and hints line up no matter how long a label is:
    // the name column fits the widest label (+gap), the value column fits the default worktree
    // template (the longest value). Longer user-typed values just overflow gracefully.
    let name_w = items
        .iter()
        .map(|(n, _, _)| n.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let val_w = 28usize;
    // Items start one row below the body top (after a leading blank) — record it for clicks.
    app.settings_geom.set(rows[1].y + 1);
    let mut lines = vec![Line::raw("")];
    for (i, (name, value, hint)) in items.iter().enumerate() {
        let selected = i == app.settings_idx;
        let marker = if selected { "▸" } else { " " };
        let row = if selected {
            Line::from(Span::styled(
                format!("  {marker} {name:<name_w$}{value:<val_w$}  {hint}"),
                app.theme.selected(),
            ))
        } else {
            Line::from(vec![
                Span::raw(format!("  {marker} ")),
                Span::styled(format!("{name:<name_w$}"), app.theme.muted()),
                Span::styled(format!("{value:<val_w$}"), app.theme.accented()),
                Span::styled(format!("  {hint}"), app.theme.muted()),
            ])
        };
        lines.push(row);
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ or click a row to change it · accent also takes a #hex in config.toml".to_string(),
        app.theme.muted(),
    )));
    if !app.status.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::raw(format!("  {}", app.status)));
    }
    f.render_widget(Paragraph::new(lines), rows[1]);
    let keys = if app.settings_editing {
        SETTINGS_EDIT_KEYS
    } else {
        SETTINGS_KEYS
    };
    f.render_widget(footer(keys, app), rows[2]);
}

const ADDREPO_KEYS: &str =
    "↑↓ select · ↵/→ enter · ←/h up  ·  a add repo · d discover here · x x remove (+ only)  ·  esc back";

fn render_addrepo(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    let path_line = if app.status.is_empty() {
        format!("  {}", app.browse_path)
    } else {
        format!("  {}    [{}]", app.browse_path, app.status)
    };
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, "REPOMON · ADD REPO", &fmt_clock(), app),
            rule(area.width, true, app),
            Line::from(Span::styled(path_line, app.theme.dim())),
        ]),
        rows[0],
    );

    let h = (rows[1].height as usize).max(1);
    let start = app.browse_selected.saturating_sub(h.saturating_sub(1));
    let mut lines = Vec::new();
    if app.browse_entries.is_empty() {
        lines.push(Line::raw("  (no subdirectories here)".to_string()));
    }
    for (i, e) in app.browse_entries.iter().enumerate().skip(start).take(h) {
        let (marker, suffix) = if e.added {
            ("+", " (added)")
        } else if e.is_repo {
            (theme::DIRTY, " (repo)")
        } else {
            (" ", "/")
        };
        let mut line = Line::raw(format!("  {marker} {}{suffix}", e.name));
        if i == app.browse_selected {
            line = line.style(app.theme.selected());
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), rows[1]);
    f.render_widget(footer(ADDREPO_KEYS, app), rows[2]);
}

const DASH_KEYS: &str = "1 fleet · 2 timeline · 3 sessions · 4 search  ·  ←/esc fleet · q quit";

fn render_timeline(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let mut lines = vec![
        header_line(area.width, "REPOMON · TIMELINE", &fmt_clock(), app),
        rule(area.width, true, app),
        Line::raw(""),
    ];
    match &app.timeline {
        Some(t) if !t.rows.is_empty() => {
            let (zoom, axis_fmt) = match app.timeline_zoom {
                crate::app::Zoom::Day => ("day", "%H:%M"),
                crate::app::Zoom::Week => ("week", "%a %d"),
                crate::app::Zoom::Month => ("month", "%b %d"),
            };
            let (from, to) = (t.from.with_timezone(&Local), t.to.with_timezone(&Local));
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{zoom} · {} – {}   ",
                        from.format("%b %d %H:%M"),
                        to.format("%b %d %H:%M")
                    ),
                    app.theme.dim(),
                ),
                Span::styled("[d]ay [w]eek [m]onth", app.theme.muted()),
            ]));
            lines.push(Line::raw(""));

            let label_w = t
                .rows
                .iter()
                .map(|r| r.repo_name.len())
                .max()
                .unwrap_or(8)
                .min(20);
            // The strip fills the terminal: resample each row to the available width (peaks
            // survive shrinking), so the chart adapts instantly to resizes between refetches.
            let avail = (area.width as usize).saturating_sub(label_w + 6).max(10);
            for row in &t.rows {
                let levels = repomon_core::analytics::resample_max(&row.density, avail);
                let active = levels.iter().any(|&l| l > 0);
                let mut spans = vec![Span::styled(
                    format!("  {:<label_w$}  ", trunc(&row.repo_name, label_w)),
                    if active {
                        app.theme.accented()
                    } else {
                        app.theme.muted()
                    },
                )];
                spans.extend(density_spans(&levels, app));
                lines.push(Line::from(spans));
            }
            // Time axis: start, midpoint, and "now" labels under the strip.
            let mid = from + (to - from) / 2;
            let (l, m, r) = (
                from.format(axis_fmt).to_string(),
                mid.format(axis_fmt).to_string(),
                to.format(axis_fmt).to_string(),
            );
            let mut axis = l.clone();
            let mid_start = avail.saturating_sub(m.chars().count()) / 2;
            while axis.chars().count() < mid_start {
                axis.push(' ');
            }
            axis.push_str(&m);
            let right_start = avail.saturating_sub(r.chars().count());
            while axis.chars().count() < right_start {
                axis.push(' ');
            }
            axis.push_str(&r);
            lines.push(Line::from(Span::styled(
                format!("  {:<label_w$}  {axis}", ""),
                app.theme.muted(),
            )));

            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled("CORRELATIONS", app.theme.bold())));
            lines.push(rule(area.width, false, app));
            lines.push(Line::raw(""));
            if t.correlations.is_empty() {
                lines.push(Line::raw("  (none above threshold)".to_string()));
            }
            let pair_w = t
                .correlations
                .iter()
                .take(8)
                .flat_map(|c| [c.a.len(), c.b.len()])
                .max()
                .unwrap_or(8)
                .min(20);
            for c in t.correlations.iter().take(8) {
                // A 10-cell meter in the accent ramp makes overlap comparable at a glance.
                let filled = ((c.overlap * 10.0).round() as usize).min(10);
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<pair_w$}", trunc(&c.a, pair_w)),
                        app.theme.accented(),
                    ),
                    Span::styled(" ↔ ", app.theme.muted()),
                    Span::styled(
                        format!("{:<pair_w$}", trunc(&c.b, pair_w)),
                        app.theme.accented(),
                    ),
                    Span::styled(format!("  {:>3} windows  ", c.windows), app.theme.muted()),
                    Span::styled("█".repeat(filled), app.theme.density(5)),
                    Span::styled("░".repeat(10 - filled), app.theme.muted()),
                    Span::raw(format!(" {:.2} overlap", c.overlap)),
                ]));
            }
        }
        _ => lines.push(Line::raw(
            "  no commit history yet (the indexer runs in the background)".to_string(),
        )),
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(footer(DASH_KEYS, app), rows[1]);
}

/// Density levels → styled block spans, adjacent equal levels merged into one span. The blocks
/// use shades of the configured accent (see `Theme::density`) so the chart matches the UI.
fn density_spans(levels: &[u8], app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < levels.len() {
        let lvl = levels[i];
        let mut j = i;
        while j < levels.len() && levels[j] == lvl {
            j += 1;
        }
        spans.push(Span::styled(
            analytics_char(lvl).repeat(j - i),
            app.theme.density(lvl),
        ));
        i = j;
    }
    spans
}

fn render_sessions(f: &mut Frame, app: &App) {
    use repomon_core::model::SessionKind;
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let total: i64 = app.sessions.iter().map(|s| s.duration_minutes()).sum();
    let mut lines = vec![
        header_line(area.width, "REPOMON · SESSIONS", &fmt_clock(), app),
        rule(area.width, true, app),
        Line::raw(""),
        Line::from(Span::styled(
            format!(
                "last 7 days · {} sessions · {}h {}m active",
                app.sessions.len(),
                total / 60,
                total % 60
            ),
            app.theme.dim(),
        )),
        Line::raw(""),
    ];
    if app.sessions.is_empty() {
        lines.push(Line::raw("  no sessions detected yet".to_string()));
    }
    for s in &app.sessions {
        let from = s.from.with_timezone(&Local).format("%a %H:%M");
        let to = s.to.with_timezone(&Local).format("%H:%M");
        let kind = match s.kind {
            SessionKind::Parallel => "parallel",
            SessionKind::Focused => "focused",
        };
        lines.push(Line::raw(format!(
            "  {from} – {to}   {:>3}m   {:<9}  {}  ({} commits)",
            s.duration_minutes(),
            kind,
            s.repo_names.join(", "),
            s.commit_count
        )));
    }
    if !app.status.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::raw(format!("  {}", app.status)));
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(
        footer(
            "e export-md  ·  1 fleet · 2 timeline · 3 sessions · 4 search  ·  q quit",
            app,
        ),
        rows[1],
    );
}

fn notif_style(app: &App, kind: NotifKind) -> Style {
    match kind {
        NotifKind::NeedsYou => app.theme.needs_you(),
        NotifKind::RateLimited => app.theme.rate_limited(),
        NotifKind::Resumed => app.theme.running(),
        NotifKind::Idle => app.theme.muted(),
    }
}

fn render_notifications(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let mut lines = vec![
        header_line(area.width, "REPOMON · NOTIFICATIONS", &fmt_clock(), app),
        rule(area.width, true, app),
        Line::raw(""),
        Line::from(Span::styled(
            format!("{} event(s) · newest first", app.notifications.len()),
            app.theme.dim(),
        )),
        Line::raw(""),
    ];
    if app.notifications.is_empty() {
        lines.push(Line::raw(
            "  no notifications yet — agent state changes show up here".to_string(),
        ));
    }
    // Two lines per event (title + detail); reserve room for the header block already pushed.
    let budget = (rows[0].height as usize).saturating_sub(lines.len()) / 2;
    for (i, ev) in app
        .notifications
        .iter()
        .rev()
        .skip(app.notif_scroll)
        .take(budget)
        .enumerate()
    {
        // The top row is the one ↵ opens (see `App::notifications_key`).
        let mark = if i == 0 { "▸ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(mark, app.theme.accented()),
            Span::styled(ev.when.format("%H:%M:%S").to_string(), app.theme.muted()),
            Span::raw("  "),
            Span::styled(ev.title.clone(), notif_style(app, ev.kind)),
        ]));
        lines.push(Line::from(vec![
            Span::raw("            "),
            Span::styled(ev.body.clone(), app.theme.dim()),
        ]));
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(
        footer(
            "↑↓ scroll · ↵ open lane · c clear  ·  1 fleet · ←/esc back · q quit",
            app,
        ),
        rows[1],
    );
}

/// The quick agent picker shown when spawning onto a lane (when "ask which agent on spawn" is
/// on). Lists the same agents as the Agents view, with the default highlighted; ↑↓ or a number
/// picks, ↵ spawns, esc cancels.
fn render_spawn_pick(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    let lane_label = app
        .spawn_pick_lane
        .and_then(|id| app.lanes.iter().find(|l| l.id == id))
        .map(|l| format!("{} / {}", l.repo.name, lane_branch(l)))
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, "REPOMON · SPAWN AGENT", &lane_label, app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );

    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  choose an agent to spawn:".to_string(),
            app.theme.dim(),
        )),
        Line::raw(""),
    ];
    if app.nl_agents.is_empty() {
        lines.push(Line::raw(
            "  no agents detected — press A to manage launch commands".to_string(),
        ));
    }
    for (i, a) in app.nl_agents.iter().enumerate() {
        let num = i + 1;
        let cursor = if i == app.spawn_pick_idx { "‣" } else { " " };
        // A short tag: the default star, or a PATH warning for an undetected command.
        let tag = if a.default {
            "★ default"
        } else if !a.detected {
            "✗ not on PATH"
        } else {
            ""
        };
        if i == app.spawn_pick_idx {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {cursor} {num}  {:<18} {:<14} {}",
                    trunc(&a.name, 18),
                    tag,
                    trunc(&a.command, 40)
                ),
                app.theme.selected(),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::raw(format!("  {cursor} {num}  ")),
                Span::styled(format!("{:<18}", trunc(&a.name, 18)), app.theme.accented()),
                Span::styled(format!(" {tag:<14}"), app.theme.muted()),
                Span::styled(format!(" {}", trunc(&a.command, 40)), app.theme.dim()),
            ]));
        }
    }
    if !app.status.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::raw(format!("  {}", app.status)));
    }
    f.render_widget(Paragraph::new(lines), rows[1]);
    f.render_widget(
        footer("↑↓ pick · 1-9 jump · ↵ spawn · esc cancel", app),
        rows[2],
    );
}

/// The fuzzy lane switcher (`f`): type to filter every lane across all repos, ↵ opens the
/// highlighted one in Focus. Matches rank by query score, then by how urgently the lane needs
/// you, then recency — so with an empty query the most pressing lanes are already on top.
fn render_lane_jump(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, "REPOMON · FIND LANE", &fmt_clock(), app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );

    let mut lines = vec![
        Line::raw(""),
        Line::raw(format!("  find: {}_", app.jump_query)),
        Line::raw(""),
    ];
    let matches = app.lane_jump_matches();
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no lanes match".to_string(),
            app.theme.dim(),
        )));
    }
    let budget = (rows[1].height as usize).saturating_sub(lines.len()).max(1);
    for (i, lane) in matches.iter().take(budget).enumerate() {
        let cursor = if i == app.jump_idx { "‣" } else { " " };
        let name = format!("{}/{}", lane.repo.name, lane.worktree.name);
        let (badge, style) = agent_badge(lane, app);
        if i == app.jump_idx {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {cursor} {:<32} {:<24} {}",
                    trunc(&name, 32),
                    trunc(&lane_branch(lane), 24),
                    badge
                ),
                app.theme.selected(),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::raw(format!("  {cursor} ")),
                Span::styled(format!("{:<32}", trunc(&name, 32)), app.theme.accented()),
                Span::styled(
                    format!(" {:<24}", trunc(&lane_branch(lane), 24)),
                    app.theme.dim(),
                ),
                Span::styled(format!(" {badge}"), style),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), rows[1]);
    f.render_widget(
        footer("type to filter · ↑↓ pick · ↵ open · esc cancel", app),
        rows[2],
    );
}

fn render_search(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let mut lines = vec![
        header_line(area.width, "REPOMON · SEARCH", &fmt_clock(), app),
        rule(area.width, true, app),
        Line::raw(""),
        Line::raw(format!("  search commits: {}_", app.search_query)),
        Line::raw(""),
    ];
    if app.search_query.trim().is_empty() {
        lines.push(Line::raw(
            "  type to search every indexed commit summary".to_string(),
        ));
    } else {
        lines.push(Line::from(Span::styled(
            format!("{} result(s)", app.search_results.len()),
            app.theme.dim(),
        )));
        lines.push(Line::raw(""));
        for c in app
            .search_results
            .iter()
            .take((rows[0].height as usize).saturating_sub(7))
        {
            let when = c.time.with_timezone(&Local).format("%Y-%m-%d %H:%M");
            let oid = c.oid.to_hex().to_string();
            lines.push(Line::raw(format!(
                "  {when}  {}  {}",
                &oid[..oid.len().min(8)],
                c.summary
            )));
        }
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(footer("type to search  ·  ←/esc fleet", app), rows[1]);
}

fn analytics_char(level: u8) -> &'static str {
    repomon_core::analytics::density_char(level)
}

fn render_fleet(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    f.render_widget(Paragraph::new(fleet_lines(app, rows[0])), rows[0]);
    f.render_widget(footer(FLEET_KEYS, app), rows[1]);
}

fn render_split(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2), // header
        Constraint::Min(0),    // sidebar + live output
        Constraint::Length(1), // mode line
        Constraint::Length(1), // footer
    ])
    .split(area);
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, "REPOMON", &fmt_clock(), app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );
    // Sidebar │ detail/output, separated by a vertical rule.
    let body = Layout::horizontal([
        Constraint::Length(26),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(rows[1]);
    f.render_widget(Paragraph::new(sidebar_lines(app, body[0])), body[0]);
    let divider: Vec<Line> = (0..body[1].height)
        .map(|_| Line::from(Span::styled(theme::VLIGHT.to_string(), app.theme.muted())))
        .collect();
    f.render_widget(Paragraph::new(divider), body[1]);
    // Live agent output if there is any, otherwise the lane's git detail.
    let id = app.selected_lane().map(|l| l.id);
    let has_output = id
        .and_then(|i| app.output.get(&i))
        .map(|p| !p.raw.trim().is_empty())
        .unwrap_or(false);
    let right = if has_output {
        output_tail(app, id, body[2].height as usize)
    } else {
        detail_lines(app)
    };
    f.render_widget(Paragraph::new(right), body[2]);
    // Clicking the pane focuses the selected agent for typing (double-click → real terminal).
    if let Some(id) = id {
        click_zone(app, body[2], id, true);
    }

    // INSERT here forwards keystrokes straight to the selected agent (no need to zoom).
    let mode = if app.focus_insert {
        Line::from(Span::styled(
            " ● INSERT — keys go to the agent (esc · ⇧⇥ · ^C all sent) · ^O to command ",
            app.theme.selected(),
        ))
    } else {
        Line::from(Span::styled(
            " ○ i type to the selected agent · ↵/→ full-screen focus ",
            app.theme.muted(),
        ))
    };
    f.render_widget(Paragraph::new(mode), rows[2]);

    let keys = if app.focus_insert {
        SPLIT_INSERT_KEYS
    } else {
        SPLIT_KEYS
    };
    f.render_widget(footer(keys, app), rows[3]);
}

fn render_focus(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2), // header
        Constraint::Min(0),    // live output
        Constraint::Length(1), // input line
        Constraint::Length(1), // footer
    ])
    .split(area);

    let lane = app.selected_lane();
    let title = match lane {
        Some(l) => format!("REPOMON · {}/{}", l.repo.name, lane_name(l)),
        None => "REPOMON".to_string(),
    };
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, &title, &fmt_clock(), app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );

    let mut body: Vec<Line> = Vec::new();
    if let Some(l) = lane {
        body.push(Line::from(Span::styled(
            focus_status_line(l, app.session_idx),
            app.theme.dim(),
        )));
    }
    let out_y0 = rows[1].y + body.len() as u16;
    let avail = (rows[1].height as usize).saturating_sub(body.len());
    body.extend(focus_output(app, lane.map(|l| l.id), avail, out_y0));
    f.render_widget(Paragraph::new(body), rows[1]);

    // Mode indicator: SCROLL while paging back, else reverse-video INSERT vs dim COMMAND.
    let mode = if app.scroll > 0 {
        Line::from(Span::styled(
            format!(
                " ↑ SCROLL +{} lines — PgUp/PgDn or wheel · ↵/esc back to live ",
                app.scroll
            ),
            app.theme.selected(),
        ))
    } else if app.focus_insert {
        Line::from(Span::styled(
            " ● INSERT — keys → agent (esc · ⇧⇥ · ^C) · PgUp/PgDn scrolls · ^O to command ",
            app.theme.selected(),
        ))
    } else {
        Line::from(Span::styled(
            " ○ COMMAND — ↵/→ open real terminal (native scroll/copy/paste) · i quick-type · PgUp scroll ",
            app.theme.muted(),
        ))
    };
    f.render_widget(Paragraph::new(mode), rows[2]);

    let keys = if app.focus_insert {
        FOCUS_INSERT_KEYS
    } else {
        FOCUS_CMD_KEYS
    };
    f.render_widget(footer(keys, app), rows[3]);
}

fn render_grid(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2), // header
        Constraint::Min(0),    // live tiles
        Constraint::Length(1), // position indicator (just above the footer)
        Constraint::Length(1), // footer
    ])
    .split(area);

    let ids = app.grid_lane_ids();
    let n = ids.len();
    let header = format!(
        "REPOMON · GRID — {n} live pane{}",
        if n == 1 { "" } else { "s" }
    );
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, &header, &fmt_clock(), app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );

    if ids.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::raw(
                    "  Nothing to babysit yet — the grid tiles your most active agents".to_string(),
                ),
                Line::raw(
                    "  (pin any with p). Spawn one with e, or press esc to go back.".to_string(),
                ),
            ]),
            rows[1],
        );
        f.render_widget(footer(GRID_KEYS, app), rows[3]);
        return;
    }

    let active = app.grid_active.min(n - 1);

    // Two columns (when wide enough); rows as needed — with a vertical/horizontal rule between
    // tiles so each live pane reads as its own box.
    let cols = if area.width >= 80 { 2 } else { 1 };
    let tile_rows = n.div_ceil(cols);
    let grid_rows = Layout::vertical(interleaved(tile_rows)).split(rows[1]);

    for r in 0..tile_rows {
        let cells = Layout::horizontal(interleaved(cols)).split(grid_rows[r * 2]);
        for c in 0..cols {
            let idx = r * cols + c;
            if idx >= n {
                continue;
            }
            let cell = cells[c * 2];
            let lane = app.lanes.iter().find(|l| l.id == ids[idx]);
            click_zone(app, cell, ids[idx], true);
            f.render_widget(
                Paragraph::new(tile_lines(
                    app,
                    lane,
                    ids[idx],
                    cell.height as usize,
                    idx == active,
                    idx == active && app.focus_insert,
                    app.hover_lane == Some(ids[idx]),
                )),
                cell,
            );
            // Vertical rule between this tile and the next column's tile.
            if c + 1 < cols && idx + 1 < n {
                let vd = cells[c * 2 + 1];
                let vline: Vec<Line> = (0..vd.height)
                    .map(|_| Line::from(Span::styled(theme::VLIGHT.to_string(), app.theme.muted())))
                    .collect();
                f.render_widget(Paragraph::new(vline), vd);
            }
        }
        // Horizontal rule between this row of tiles and the next.
        if r + 1 < tile_rows {
            f.render_widget(
                Paragraph::new(rule(grid_rows[r * 2 + 1].width, false, app)),
                grid_rows[r * 2 + 1],
            );
        }
    }

    // Instagram-style position indicator at the bottom (above the footer): a dot per tile, the
    // active one filled and accent-colored, plus its name — so what's selected is clear at a glance.
    let label = app
        .lanes
        .iter()
        .find(|l| l.id == ids[active])
        .map(|l| format!("{}/{}", l.repo.name, lane_name(l)))
        .unwrap_or_default();
    let mut spans = vec![Span::raw("  ")];
    for i in 0..n {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        if i == active {
            spans.push(Span::styled("●", app.theme.accented()));
        } else {
            spans.push(Span::styled("○", app.theme.muted()));
        }
    }
    spans.push(Span::styled(
        format!("    {}/{n}  {label}", active + 1),
        app.theme.bold(),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), rows[2]);
    let keys = if app.focus_insert {
        GRID_INSERT_KEYS
    } else {
        GRID_KEYS
    };
    f.render_widget(footer(keys, app), rows[3]);
}

fn tile_lines(
    app: &App,
    lane: Option<&Lane>,
    id: LaneId,
    height: usize,
    active: bool,
    focused: bool,
    hovered: bool,
) -> Vec<Line<'static>> {
    let marker = if focused {
        "▶"
    } else if active {
        "▎"
    } else {
        " "
    };
    let (badge, badge_style) = match lane {
        Some(l) => agent_badge(l, app),
        None => (String::new(), Style::default()),
    };
    let label = match lane {
        Some(l) => format!("{marker}{}/{}  ", l.repo.name, lane_name(l)),
        None => format!("{marker}lane {id}"),
    };
    let base = if focused {
        app.theme.selected()
    } else if active {
        app.theme.bold()
    } else if hovered {
        app.theme.hover()
    } else {
        app.theme.dim()
    };
    let mut spans = vec![Span::styled(label, base)];
    if !badge.is_empty() {
        // While focused the whole header is the reverse-video bar; otherwise the badge is colored.
        let bs = if focused { base } else { badge_style };
        spans.push(Span::styled(badge, bs));
    }
    // When click-focused, the tile is capturing keystrokes — make that unmistakable.
    if focused {
        spans.push(Span::styled(
            "  ⌨ typing (^O / click-out to blur)".to_string(),
            base,
        ));
    }
    let mut lines = vec![Line::from(spans)];
    lines.extend(output_tail(app, Some(id), height.saturating_sub(1)));
    lines
}

/// The status badge text for a lane's agents, plus the style (color) for that status.
fn agent_badge(lane: &Lane, app: &App) -> (String, Style) {
    use repomon_core::model::AgentStatus;
    let sessions = &lane.agent_sessions;
    if sessions.is_empty() {
        return (String::new(), Style::default());
    }
    let n = sessions.len();
    let waiting = sessions
        .iter()
        .filter(|s| s.status == AgentStatus::Waiting)
        .count();
    let running = sessions
        .iter()
        .filter(|s| s.status == AgentStatus::Running)
        .count();
    // `ext` flags agents running in another terminal; `×N` when several share the worktree.
    let tag = if sessions.iter().any(|s| s.external) {
        " ·ext"
    } else {
        ""
    };
    let count = if n > 1 {
        format!(" ×{n}")
    } else {
        String::new()
    };
    let rate_limited = sessions
        .iter()
        .find(|s| s.status == AgentStatus::RateLimited);
    if let Some(rl) = rate_limited {
        (
            format!("⏳ rate-limited · {}{count}{tag}", fmt_resume(rl.resume_at)),
            app.theme.rate_limited(),
        )
    } else if waiting > 0 {
        (format!("⏸ needs you{count}{tag}"), app.theme.needs_you())
    } else if running > 0 {
        (format!("▶ running{count}{tag}"), app.theme.running())
    } else {
        (format!("idle{count}{tag}"), app.theme.idle())
    }
}

/// How a pending auto-continue reads in a badge: the local resume time, or "retrying" when the
/// reset time couldn't be parsed (periodic retry).
fn fmt_resume(resume_at: Option<DateTime<Utc>>) -> String {
    match resume_at {
        Some(t) => format!("resume {}", t.with_timezone(&Local).format("%-I:%M %p")),
        None => "retrying".to_string(),
    }
}

/// A compact "time since" label for a session's last activity.
fn ago(t: DateTime<Utc>) -> String {
    let mins = (Utc::now() - t).num_minutes().max(0);
    if mins < 60 {
        format!("{mins}m")
    } else if mins < 60 * 24 {
        format!("{}h", mins / 60)
    } else {
        format!("{}d", mins / (60 * 24))
    }
}

/// Parse a captured pane (with `-e` ANSI escapes) into styled lines, trimming the trailing blank
/// rows tmux pads the pane with. Called once per `event.agent.output` delta (in
/// `App::on_notification`) and cached, so the render path only slices — it never re-parses.
pub(crate) fn parse_pane(raw: &str) -> Vec<Line<'static>> {
    use ansi_to_tui::IntoText;
    let mut lines: Vec<Line<'static>> = raw
        .into_text()
        .map(|t| t.lines)
        .unwrap_or_else(|_| raw.lines().map(|l| Line::raw(l.to_string())).collect());
    while lines.last().map(line_is_blank).unwrap_or(false) {
        lines.pop();
    }
    lines
}

/// The last `height` lines of a lane's captured output, ANSI-colored, or a placeholder.
fn output_tail(app: &App, lane_id: Option<LaneId>, height: usize) -> Vec<Line<'static>> {
    match lane_id.and_then(|id| app.output.get(&id)) {
        Some(p) if !p.raw.trim().is_empty() => {
            let start = p.lines.len().saturating_sub(height.max(1));
            p.lines[start..].to_vec()
        }
        _ => vec![Line::from(Span::styled(
            "(no live output — press e to start claude here)".to_string(),
            app.theme.dim(),
        ))],
    }
}

fn line_is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

/// The `[start, end)` window of a `total`-line buffer showing `height` lines, scrolled up
/// `scroll` from the bottom (clamped so the top of the buffer is the limit).
fn output_window(total: usize, height: usize, scroll: usize) -> (usize, usize) {
    let h = height.max(1);
    if total <= h {
        return (0, total);
    }
    let s = scroll.min(total - h);
    let end = total - s;
    (end - h, end)
}

/// The Focus pane's visible lines: the live tail (or the scrollback window), ANSI-colored,
/// with drag-selected lines highlighted. Records the geometry on `app` so the mouse handler
/// can map a screen row back to a buffer line for selection/copy.
fn focus_output(
    app: &App,
    lane_id: Option<LaneId>,
    avail: usize,
    out_y0: u16,
) -> Vec<Line<'static>> {
    // Pre-parsed lines: the scrollback snapshot when scrolled, else the live tail. (Both are
    // parsed once on update — see `parse_pane` — so this hot path only slices.)
    let lines: &[Line<'static>] = if app.scroll > 0 {
        app.scroll_lines.as_deref().unwrap_or(&[])
    } else {
        lane_id
            .and_then(|id| app.output.get(&id))
            .map_or(&[], |p| p.lines.as_slice())
    };
    if lines.is_empty() {
        app.focus_geom.set((out_y0, 0, 0));
        return vec![Line::from(Span::styled(
            "(no live output — press e to start claude here)".to_string(),
            app.theme.dim(),
        ))];
    }
    let (start, end) = output_window(lines.len(), avail, app.scroll);
    app.focus_geom.set((out_y0, start, end - start));

    let sel = match (app.sel_anchor, app.sel_head) {
        (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
        _ => None,
    };
    lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let mut l = line.clone();
            if let Some((lo, hi)) = sel {
                if (lo..=hi).contains(&(start + i)) {
                    l = l.style(app.theme.selected());
                }
            }
            l
        })
        .collect()
}

fn focus_status_line(lane: &Lane, idx: usize) -> String {
    let s = &lane.state;
    let branch = lane_branch(lane);
    let ab = ahead_behind_str(s.ahead, s.behind);
    let n = lane.agent_sessions.len();
    let sess = lane
        .agent_sessions
        .get(idx)
        .or_else(|| lane.agent_sessions.first());
    match sess {
        Some(sess) => format!(
            "{}{} · {} {} · {} · {} calls{}",
            sess.agent.short(),
            // Several agents share this lane: show which one the keys/pane are on.
            if n > 1 {
                format!(" {}/{n} (tab switches)", idx.min(n - 1) + 1)
            } else {
                String::new()
            },
            branch,
            ab,
            sess.status.as_str(),
            sess.tool_call_count,
            if sess.external {
                " · external (o to adopt)"
            } else {
                ""
            }
        ),
        None => format!("{branch} {ab} · no agent"),
    }
}

fn render_new_lane(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let mut lines = vec![
        header_line(area.width, "REPOMON · NEW LANE", &fmt_clock(), app),
        rule(area.width, true, app),
        Line::raw(""),
    ];
    let repo_name = app
        .repos
        .get(app.nl_repo_idx)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "(no repos — add one first)".into());
    let safe_branch = app.nl_branch.replace('/', "-");
    let preview_path = if app.nl_branch.is_empty() {
        "(enter a branch name)".to_string()
    } else {
        format!("~/code/{repo_name}-wt/{safe_branch}")
    };
    lines.push(Line::raw(format!("  repo      {repo_name}")));
    match app.nl_agents.get(app.nl_agent_idx) {
        Some(a) => {
            let mark = if a.detected { "✓" } else { "✗ not on PATH" };
            let tag = if a.custom { " (custom)" } else { "" };
            let star = if a.default { " ★ default" } else { "" };
            lines.push(Line::raw(format!(
                "  agent     {}{tag}{star}   [tab to change]",
                a.name
            )));
            lines.push(Line::from(Span::styled(
                format!("            $ {}   {mark}", a.command),
                app.theme.dim(),
            )));
        }
        None => lines.push(Line::raw("  agent     (none detected)".to_string())),
    }
    lines.push(Line::raw(format!("  branch    {}_", app.nl_branch)));
    lines.push(Line::raw("  source    HEAD".to_string()));
    lines.push(Line::raw(format!("  path      {preview_path}   [auto]")));
    lines.push(Line::raw(""));
    lines.push(rule(area.width, false, app));
    lines.push(Line::raw(""));
    if !app.status.is_empty() {
        lines.push(Line::raw(format!("  {}", app.status)));
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(footer(NEWLANE_KEYS, app), rows[1]);
}

// ---- shared line builders ----------------------------------------------------

fn fleet_lines(app: &App, content: Rect) -> Vec<Line<'static>> {
    let width = content.width;
    let now = Utc::now();
    let mut lines = Vec::new();
    lines.push(header_line(width, "REPOMON", &fmt_clock(), app));
    lines.push(rule(width, true, app));
    lines.push(Line::raw(""));

    let visible = app.visible_lanes();
    let repos = distinct_repos(&visible);
    let needs = visible
        .iter()
        .filter(|l| l.agent_sessions.iter().any(|s| s.status.needs_you()))
        .count();
    let urgent = if app.urgent_only {
        " · URGENT only"
    } else {
        ""
    };
    let rest = format!(
        "  {} lanes · {repos} repos · {needs} need you{urgent}",
        visible.len()
    );
    let right = format!("today · {} commits", app.commits.len());
    lines.push(section_header(width, "FLEET", &rest, &right, app));
    lines.push(rule(width, false, app));
    lines.push(Line::from(Span::styled(
        "  +staged ~modified ?untracked · ↑ahead ↓behind".to_string(),
        app.theme.muted(),
    )));
    lines.push(Line::raw(""));

    if app.filtering || !app.filter.is_empty() {
        let cursor = if app.filtering { "_" } else { "" };
        lines.push(Line::raw(format!("  filter: {}{cursor}", app.filter)));
        lines.push(Line::raw(""));
    }

    if visible.is_empty() {
        lines.push(Line::raw(
            "  no lanes yet — press a to browse for repos to add (or `repomon add <path>`),"
                .to_string(),
        ));
        lines.push(Line::raw("  then n to create a lane.".to_string()));
    }

    let mut current: Option<RepoId> = None;
    for (i, lane) in visible.iter().enumerate() {
        if current != Some(lane.repo.id) {
            if current.is_some() {
                lines.push(Line::raw("")); // a gap between repo groups
            }
            current = Some(lane.repo.id);
            lines.push(repo_header(width, &lane.repo.name, app));
        }
        let selected = i == app.selected;
        let mut line = lane_row(lane, now, app, selected);
        if !selected && app.hover_lane == Some(lane.id) {
            line = line.style(app.theme.hover());
        }
        // Record this row's on-screen rect so a click selects the lane (no live pane here, so a
        // single click only selects; double-click opens the real terminal).
        let li = lines.len() as u16;
        if li < content.height {
            click_zone(
                app,
                Rect {
                    x: content.x,
                    y: content.y + li,
                    width: content.width,
                    height: 1,
                },
                lane.id,
                false,
            );
        }
        lines.push(line);
    }

    lines.push(Line::raw(""));
    lines.push(section_header(width, "TODAY", "", &tz_offset(), app));
    lines.push(rule(width, false, app));
    lines.push(Line::raw(""));
    for c in app.commits.iter().take(12) {
        lines.push(commit_line(c, app));
    }
    if !app.status.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::raw(format!("  {}", app.status)));
    }
    lines
}

fn sidebar_lines(app: &App, content: Rect) -> Vec<Line<'static>> {
    let now = Utc::now();
    let visible = app.visible_lanes();
    let mut lines = vec![
        Line::from(Span::styled(
            format!("FLEET {}", visible.len()),
            app.theme.header_style(),
        )),
        Line::raw(""),
    ];
    let mut current: Option<RepoId> = None;
    for (i, lane) in visible.iter().enumerate() {
        if current != Some(lane.repo.id) {
            current = Some(lane.repo.id);
            lines.push(Line::from(Span::styled(
                lane.repo.name.clone(),
                app.theme.muted(),
            )));
        }
        let glyph = status_glyph(lane);
        let counts = dirty_str(&lane.state.dirty);
        let rest = format!(" {:<10} {}", trunc(&lane_name(lane), 10), counts);
        let _ = now;
        let mut line = if i == app.selected {
            Line::from(Span::styled(
                format!(" {glyph}{rest}"),
                app.theme.selected(),
            ))
        } else {
            let glyph_style = if lane.state.dirty.is_clean() {
                app.theme.muted()
            } else {
                app.theme.accented()
            };
            Line::from(vec![
                Span::raw(" "),
                Span::styled(glyph.to_string(), glyph_style),
                Span::raw(rest),
            ])
        };
        if i != app.selected && app.hover_lane == Some(lane.id) {
            line = line.style(app.theme.hover());
        }
        let li = lines.len() as u16;
        if li < content.height {
            click_zone(
                app,
                Rect {
                    x: content.x,
                    y: content.y + li,
                    width: content.width,
                    height: 1,
                },
                lane.id,
                true,
            );
        }
        lines.push(line);
    }
    lines
}

fn detail_lines(app: &App) -> Vec<Line<'static>> {
    let Some(lane) = app.selected_lane() else {
        return vec![Line::raw("  (no lane selected)".to_string())];
    };
    let s = &lane.state;
    let branch = lane_branch(lane);
    let upstream = s.upstream.clone().unwrap_or_else(|| "—".into());
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} · {}", lane.repo.name, lane_name(lane)),
            app.theme.header_style(),
        )),
        Line::raw(""),
        Line::raw(format!("  path     {}", lane.worktree.path.display())),
        Line::raw(format!(
            "  branch   {branch} → {upstream}   {}",
            ahead_behind_str(s.ahead, s.behind)
        )),
        Line::raw(format!(
            "  status   +{} staged, ~{} unstaged, ?{} untracked",
            s.dirty.staged, s.dirty.unstaged, s.dirty.untracked
        )),
    ];
    if s.locked {
        lines.push(Line::raw("  lock     this worktree is locked".to_string()));
    }
    if s.prunable {
        lines.push(Line::raw(
            "  prune    this worktree is prunable (g to prune)".to_string(),
        ));
    }
    // Agent sessions: every recently-active agent in this worktree (several can run at once),
    // with a cursor (‣) on the one `o` will adopt. External = running in another terminal.
    if lane.agent_sessions.is_empty() {
        lines.push(Line::raw(
            "  agents   none running  (e to spawn)".to_string(),
        ));
    } else {
        let n = lane.agent_sessions.len();
        let waiting = lane
            .agent_sessions
            .iter()
            .filter(|s| s.status.needs_you())
            .count();
        let adoptable = lane.agent_sessions.iter().any(|s| s.external);
        let hint = if adoptable && n > 1 {
            "  · tab to switch · o adopt"
        } else if adoptable {
            "  · o adopt"
        } else {
            ""
        };
        lines.push(Line::from(Span::styled(
            format!("  agents   {n} active · {waiting} waiting{hint}"),
            app.theme.header_style(),
        )));
        for (i, sess) in lane.agent_sessions.iter().enumerate() {
            let cursor = if i == app.session_idx { "‣" } else { " " };
            let glyph = match sess.status.as_str() {
                "waiting" => "⏸",
                "running" => "▶",
                "rate-limited" => "⏳",
                _ => "●",
            };
            let kind = if sess.external { "ext " } else { "mine" };
            let label = sess
                .title
                .clone()
                .or_else(|| sess.session_id.clone().map(|s| s.chars().take(8).collect()))
                .unwrap_or_else(|| sess.agent.short().to_string());
            // For a rate-limited agent, show when it auto-continues instead of the last-active age.
            let trailer = if sess.status == repomon_core::model::AgentStatus::RateLimited {
                format!("rate-limited · {}", fmt_resume(sess.resume_at))
            } else {
                ago(sess.last_activity_at)
            };
            lines.push(Line::from(vec![
                Span::raw(format!("  {cursor} ")),
                Span::styled(glyph.to_string(), app.theme.status(sess.status)),
                Span::raw(format!(" {kind}  {:<40}  ", trunc(&label, 40))),
                Span::styled(trailer, app.theme.muted()),
            ]));
        }
    }
    // Plain shell terminals (no agent) — open as many as you like with `t`.
    let term_line = if app.terminals.is_empty() {
        "  terminals  none  ·  t open a shell here".to_string()
    } else {
        format!(
            "  terminals  {} open  ·  t new · T re-attach  (in a terminal: exit to close · ^b d to detach)",
            app.terminals.len()
        )
    };
    lines.push(Line::raw(term_line));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "RECENT COMMITS",
        app.theme.header_style(),
    )));
    lines.push(Line::raw(""));
    // The latest commits on this worktree's branch (loaded per-selection), so a feature
    // branch or a repo with nothing today still shows its history.
    if app.recent_commits.is_empty() {
        lines.push(Line::raw("  (no commits yet)".to_string()));
    } else {
        for c in app.recent_commits.iter().take(8) {
            lines.push(commit_line(c, app));
        }
    }
    lines
}

// ---- atoms -------------------------------------------------------------------

fn footer(keys: &str, app: &App) -> Paragraph<'static> {
    use ratatui::style::Modifier;
    // A fresh notification takes over the footer line briefly (then the key hints return).
    if let Some((msg, since)) = &app.notif_banner {
        if since.elapsed() < crate::app::NOTIF_BANNER_TTL {
            return Paragraph::new(Line::from(Span::styled(
                format!("🔔 {msg}"),
                app.theme.needs_you().add_modifier(Modifier::BOLD),
            )));
        }
    }
    Paragraph::new(Line::from(Span::styled(
        keys.to_string(),
        app.theme.muted(),
    )))
}

fn header_line(width: u16, left: &str, right: &str, app: &App) -> Line<'static> {
    // The view title is the accent; the clock on the right is muted.
    let used = left.chars().count() + right.chars().count();
    let pad = (width as usize).saturating_sub(used);
    Line::from(vec![
        Span::styled(left.to_string(), app.theme.header_style()),
        Span::raw(" ".repeat(pad)),
        Span::styled(right.to_string(), app.theme.muted()),
    ])
}

/// A clear section divider: an accent title, the rest of the left text muted, and an optional
/// muted right-aligned tail. Used for FLEET / TODAY / AGENTS-style headings.
fn section_header(width: u16, title: &str, rest: &str, right: &str, app: &App) -> Line<'static> {
    let used = title.chars().count() + rest.chars().count() + right.chars().count();
    let pad = (width as usize).saturating_sub(used);
    Line::from(vec![
        Span::styled(title.to_string(), app.theme.header_style()),
        Span::styled(rest.to_string(), app.theme.muted()),
        Span::raw(" ".repeat(pad)),
        Span::styled(right.to_string(), app.theme.muted()),
    ])
}

/// Layout constraints for `count` equal cells separated by single-cell dividers:
/// `[Fill(1), Length(1), Fill(1), …]`. The cell at logical index `i` is at split index `i * 2`,
/// the divider after it at `i * 2 + 1`.
fn interleaved(count: usize) -> Vec<Constraint> {
    let mut v = Vec::new();
    for i in 0..count {
        if i > 0 {
            v.push(Constraint::Length(1));
        }
        v.push(Constraint::Fill(1));
    }
    v
}

fn rule(width: u16, heavy: bool, app: &App) -> Line<'static> {
    // Heavy header rules take the accent; light section rules are muted — so dividers read as a
    // distinct layer instead of blending with the white body text.
    let c = if heavy { theme::HEAVY } else { theme::LIGHT };
    let style = if heavy {
        app.theme.accented()
    } else {
        app.theme.muted()
    };
    Line::from(Span::styled(c.to_string().repeat(width as usize), style))
}

fn repo_header(width: u16, name: &str, app: &App) -> Line<'static> {
    // "  NAME ─────…" — the repo name in the accent, the rule muted, so each group is delineated.
    let used = 2 + name.chars().count() + 1;
    let dashes = (width as usize).saturating_sub(used);
    Line::from(vec![
        Span::raw("  "),
        Span::styled(name.to_string(), app.theme.header_style()),
        Span::raw(" "),
        Span::styled(theme::LIGHT.to_string().repeat(dashes), app.theme.muted()),
    ])
}

fn lane_row(lane: &Lane, now: DateTime<Utc>, app: &App, selected: bool) -> Line<'static> {
    use ratatui::style::Modifier;
    use repomon_core::model::AgentStatus;
    let glyph = status_glyph(lane);
    // Only *real* (non-inferred) sessions drive the named status glyphs; an inferred "file
    // activity" session falls through to the soft ◐ so we never claim a specific agent.
    let any = |st: AgentStatus| {
        lane.agent_sessions
            .iter()
            .any(|s| !s.inferred && s.status == st)
    };
    let any_inferred = lane.agent_sessions.iter().any(|s| s.inferred);
    let (active, active_style) = if any(AgentStatus::RateLimited) {
        (theme::RATE_LIMITED, app.theme.rate_limited()) // ⏳ paused on a usage limit
    } else if any(AgentStatus::Waiting) {
        (theme::WAITING, app.theme.needs_you()) // ⏸ needs you
    } else if any(AgentStatus::Running) {
        (theme::AGENT_ACTIVE, app.theme.running()) // ▶ working
    } else if any_inferred {
        // ◐ active — files are changing but we can't name the agent (worktree subagent).
        (
            theme::INFERRED_ACTIVE,
            app.theme.running().add_modifier(Modifier::DIM),
        )
    } else {
        (" ", app.theme.dim())
    };
    let glyph_style = if lane.state.dirty.is_clean() {
        app.theme.muted()
    } else {
        app.theme.accented()
    };
    let agent = lane
        .agent_sessions
        .first()
        .map(|s| s.agent.short().to_string())
        .unwrap_or_default();
    let mid = format!(
        "{:<12} {:<36} {:<11} {:<7} {:<7} ",
        trunc(&lane_name(lane), 12),
        trunc(&lane_branch(lane), 36),
        dirty_str(&lane.state.dirty),
        ahead_behind_str(lane.state.ahead, lane.state.behind),
        trunc(&agent, 7),
    );
    let time = rel_time(lane.last_activity_at, now);
    if selected {
        // A clean reverse-video bar for the selection (per-cell colors would muddy it).
        let text = format!("  {glyph} {active} {mid}{time}");
        return Line::from(Span::styled(text, app.theme.selected()));
    }
    Line::from(vec![
        Span::raw("  "),
        Span::styled(glyph.to_string(), glyph_style),
        Span::raw(" "),
        Span::styled(active.to_string(), active_style),
        Span::raw(" "),
        Span::raw(mid),
        Span::styled(time, app.theme.muted()),
    ])
}

fn commit_line(c: &Commit, app: &App) -> Line<'static> {
    let time = c.time.with_timezone(&Local).format("%H:%M").to_string();
    let repo = app
        .lanes
        .iter()
        .find(|l| l.repo.id == c.repo_id)
        .map(|l| l.repo.name.clone())
        .unwrap_or_else(|| format!("repo{}", c.repo_id));
    Line::from(vec![
        Span::styled(format!("  {time}  "), app.theme.muted()),
        Span::styled(format!("{:<18} ", trunc(&repo, 18)), app.theme.muted()),
        Span::raw(c.summary.clone()),
    ])
}

fn status_glyph(lane: &Lane) -> &'static str {
    if lane.state.dirty.is_clean() {
        theme::CLEAN
    } else {
        theme::DIRTY
    }
}

fn lane_name(lane: &Lane) -> String {
    if lane.worktree.is_main {
        "main".to_string()
    } else {
        lane.worktree.name.clone()
    }
}

fn lane_branch(lane: &Lane) -> String {
    match &lane.state.branch {
        Some(b) => b.clone(),
        None => {
            let hex = lane.state.head.to_hex().to_string();
            format!("({})", &hex[..hex.len().min(8)])
        }
    }
}

fn dirty_str(d: &DirtyState) -> String {
    let mut parts = Vec::new();
    if d.staged > 0 {
        parts.push(format!("+{}", d.staged));
    }
    if d.unstaged > 0 {
        parts.push(format!("~{}", d.unstaged));
    }
    if d.untracked > 0 {
        parts.push(format!("?{}", d.untracked));
    }
    parts.join(" ")
}

fn ahead_behind_str(ahead: u32, behind: u32) -> String {
    let mut parts = Vec::new();
    if ahead > 0 {
        parts.push(format!("{}{ahead}", theme::UP));
    }
    if behind > 0 {
        parts.push(format!("{}{behind}", theme::DOWN));
    }
    parts.join(" ")
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn rel_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds();
    if secs < 60 {
        "now".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn fmt_clock() -> String {
    Local::now()
        .format("%H:%M %a %d %b %Y")
        .to_string()
        .to_lowercase()
}

fn tz_offset() -> String {
    format!("UTC{}", Local::now().format("%:z"))
}

fn distinct_repos(lanes: &[&Lane]) -> usize {
    let mut ids: Vec<RepoId> = lanes.iter().map(|l| l.repo.id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.len()
}

/// Render a buffer to plain text (for `--print-once` and tests).
pub fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}
