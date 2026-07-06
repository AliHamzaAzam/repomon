//! Rendering for the views. Flat, brutalist layout — light/heavy rules, single-char glyphs,
//! two-space indents — with a semantic color palette over the top (status colors + a
//! configurable accent; see [`crate::theme`]). `accent = "mono"` restores the no-color look.

use chrono::{DateTime, Local, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use repomon_core::agent::attention::{Attention, agent_attention};
use repomon_core::model::{AgentStatus, Commit, DirtyState, Lane, LaneId, RepoId};

use crate::app::{AgField, App, ClickZone};
use crate::keybinds::View;
use crate::notify::NotifKind;
use crate::theme;

const FLEET_KEYS: &str = "↑↓ ↵ open · click select · dbl terminal  ·  n new · e spawn · t term · R rename  ·  a add-repo · A agents · , settings · d del · X rm-repo  ·  / filter · f find · ! urgent · g/G needs-you · C auto-cont  ·  O repomind · 2 timeline · 3 sessions · 4 search  ·  spc grid · q";
const SPLIT_KEYS: &str = "↑↓ lane · tab session  ·  click focus · wheel/PgUp scroll · dbl terminal · ↵ open · → focus · i quick-type  ·  e spawn · o adopt · R rename · C auto-cont  ·  ←/esc back";
const SPLIT_INSERT_KEYS: &str =
    "keys → agent (esc · ⇧⇥ · ^C sent) · PgUp/PgDn scroll  ·  ^O / click-out blur";
const SPLIT_ORCH_KEYS: &str = "i type to repomind · ↵/→ open command-center  ·  ↑↓ lane · click focus · wheel scroll  ·  ←/esc back";
const SPLIT_ORCH_INSERT_KEYS: &str =
    "keys → repomind (esc · ⇧⇥ · ^C sent) · ↵ send  ·  ^O leave insert";
const FOCUS_CMD_KEYS: &str = "↵/→ open (real terminal) · i quick-type · tab agent · PgUp scroll  ·  e spawn · o adopt · t term · s stop  ·  g/G next · f find  ·  ←/esc back";
const FOCUS_INSERT_KEYS: &str = "keys → agent (esc · ⇧⇥ · ^C sent)  ·  ^O command-mode";
const GRID_KEYS: &str = "←→ move · click focus (type in place) · dbl terminal · ↵ open  ·  e spawn · s stop · p pin · g/G next · f find  ·  spc/esc fleet · q quit";
const GRID_INSERT_KEYS: &str = "keys → agent (esc · ⇧⇥ · ^C sent)  ·  ^O / click-out blur";
const NEWLANE_KEYS: &str =
    "↑↓ repo · tab agent · ^a manage  ·  type branch · ↵ create + spawn  ·  esc cancel";

/// Record a clickable lane region for the mouse handler to hit-test (see `App::handle_click`).
/// `session` targets one agent within the lane when the sidebar is expanded into per-agent rows.
fn click_zone(app: &App, rect: Rect, lane: LaneId, session: Option<usize>, interactive: bool) {
    app.click_zones.borrow_mut().push(ClickZone {
        rect,
        lane,
        session,
        interactive,
    });
}

/// Render the current view.
pub fn render(f: &mut Frame, app: &App) {
    // Clickable lane regions are recomputed each frame by the per-view renderers below.
    app.click_zones.borrow_mut().clear();
    // The pinned "repomind" row's click rect and repomind's pane rect are re-set only by the
    // Fleet/Split/command-center renderers each frame; clear them so a stale rect can't be hit
    // from another view.
    app.orch_click.set(None);
    app.orch_pane_zone.set(None);
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
        View::Orchestrator => render_orchestrator(f, app),
    }
    // Drawn last, over the free right end of the bottom row, so it overlays every view.
    corner(f, app);
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
        f.render_widget(footer(AGENTS_EDIT_KEYS, app, rows[2].width), rows[2]);
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
    f.render_widget(footer(AGENTS_KEYS, app, rows[2].width), rows[2]);
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
    let orch_agent = if s.orchestrator_agent.is_empty() {
        "(default claude)".to_string()
    } else {
        s.orchestrator_agent.clone()
    };
    let orch_model = if s.orchestrator_model.is_empty() {
        "default".to_string()
    } else {
        s.orchestrator_model.clone()
    };
    let items: [(&str, String, &str); 20] = [
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
        (
            "  · show agent's question",
            onoff(s.notify_show_why),
            "body = its last message",
        ),
        (
            "  · coalesce bursts",
            onoff(s.notify_coalesce),
            "one popup for N alerts",
        ),
        (
            "  · click focuses terminal",
            onoff(s.notify_click_focus),
            "needs terminal-notifier",
        ),
        (
            "  · subagents finishing",
            onoff(s.notify_subagents),
            "off = only the main agent",
        ),
        (
            "usage corner (spawns claude)",
            onoff(s.usage_probe),
            "/usage probe · space toggles",
        ),
        (
            "expand agent rows",
            onoff(s.expand_agents),
            "per-agent sidebar rows · space toggles",
        ),
        (
            "repomind agent",
            orch_agent,
            "←/→ cycle · Claude account or codex",
        ),
        (
            "repomind model",
            orch_model,
            "←/→ cycle · default/opus/sonnet",
        ),
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
    f.render_widget(footer(keys, app, rows[2].width), rows[2]);
}

const ADDREPO_KEYS: &str = "↑↓ select · ↵/→ enter · ←/h up  ·  a add repo · d d discover · x x remove (+ only)  ·  esc back";

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
    f.render_widget(footer(ADDREPO_KEYS, app, rows[2].width), rows[2]);
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
    f.render_widget(footer(DASH_KEYS, app, rows[1].width), rows[1]);
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
            rows[1].width,
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
    use ratatui::style::Modifier;
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let unread = app.unread_notifs();
    let counts = if unread > 0 {
        format!(
            "{} event(s) · {} new · newest first",
            app.notifications.len(),
            unread
        )
    } else {
        format!("{} event(s) · newest first", app.notifications.len())
    };
    let mut lines = vec![
        header_line(area.width, "REPOMON · NOTIFICATIONS", &fmt_clock(), app),
        rule(area.width, true, app),
        Line::raw(""),
        Line::from(Span::styled(counts, app.theme.dim())),
        Line::raw(""),
    ];
    if app.notifications.is_empty() {
        lines.push(Line::raw(
            "  no notifications yet — agent state changes show up here".to_string(),
        ));
    }
    // Two lines per event (title + detail); reserve room for the header block already pushed.
    let budget = ((rows[0].height as usize).saturating_sub(lines.len()) / 2).max(1);
    // Scroll the window just enough to keep the cursor (▸) on screen.
    let skip = app.notif_sel.saturating_sub(budget.saturating_sub(1));
    for (i, ev) in app
        .notifications
        .iter()
        .rev()
        .enumerate()
        .skip(skip)
        .take(budget)
    {
        let mark = if i == app.notif_sel { "▸ " } else { "  " };
        // Unseen events keep a ● until the feed visit ends (they arrived while it was open).
        let (dot, dot_style) = if ev.read {
            ("  ", app.theme.muted())
        } else {
            ("● ", app.theme.needs_you())
        };
        let title_style = if ev.read {
            notif_style(app, ev.kind)
        } else {
            notif_style(app, ev.kind).add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(vec![
            Span::styled(mark, app.theme.accented()),
            Span::styled(dot, dot_style),
            Span::styled(ev.when.format("%H:%M:%S").to_string(), app.theme.muted()),
            Span::raw("  "),
            Span::styled(ev.title.clone(), title_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw("              "),
            Span::styled(ev.body.clone(), app.theme.dim()),
        ]));
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(
        footer(
            "↑↓ move · ↵ open · t attach · d dismiss · c clear  ·  1 fleet · ←/esc back · q quit",
            app,
            rows[1].width,
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
        footer(
            "↑↓ pick · 1-9 jump · ↵ spawn · esc cancel",
            app,
            rows[2].width,
        ),
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
        footer(
            "type to filter · ↑↓ pick · ↵ open · esc cancel",
            app,
            rows[2].width,
        ),
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
    f.render_widget(
        footer("type to search  ·  ←/esc fleet", app, rows[1].width),
        rows[1],
    );
}

fn analytics_char(level: u8) -> &'static str {
    repomon_core::analytics::density_char(level)
}

fn render_fleet(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let content = rows[0];
    let (lines, selected_line, lane_rows, brain_line) = fleet_lines(app, content);
    // Scroll so the selected lane stays on screen (roughly centered), clamped to the list bounds —
    // this is what lets the fleet grow past one screenful and still be navigable with ↑/↓.
    let h = content.height as usize;
    let max_scroll = lines.len().saturating_sub(h);
    let scroll = selected_line
        .map(|sl| sl.saturating_sub(h / 2))
        .unwrap_or(0)
        .min(max_scroll);
    // Register the pinned repomind row's on-screen rect (scroll-adjusted) so a click opens the view.
    if let Some(bli) = brain_line {
        if bli >= scroll && bli < scroll + h {
            app.orch_click.set(Some(Rect {
                x: content.x,
                y: content.y + (bli - scroll) as u16,
                width: content.width,
                height: 1,
            }));
        }
    }
    // Click-to-select: register each *visible* row at its on-screen position (scroll-adjusted).
    for (li, lane_id, session) in &lane_rows {
        if *li >= scroll && *li < scroll + h {
            click_zone(
                app,
                Rect {
                    x: content.x,
                    y: content.y + (*li - scroll) as u16,
                    width: content.width,
                    height: 1,
                },
                *lane_id,
                *session,
                false,
            );
        }
    }
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), content);
    f.render_widget(footer(FLEET_KEYS, app, rows[1].width), rows[1]);
}

const ORCH_KEYS: &str = "i message repomind · ↵/→ attach (full terminal) · r r restart (saved settings) · ↑↓/PgUp/wheel scroll · click lane to jump  ·  ←/esc back · q quit";
const ORCH_INSERT_KEYS: &str =
    "type to repomind · ↵ send · ^O leave insert · PgUp/PgDn scroll · esc/^C → repomind";

/// The pinned row's one-word state: `off` (no session), `chatting` (pane changed in the last few
/// seconds), else `idle`.
fn orch_status_word(app: &App) -> &'static str {
    if !app.orch_running {
        "off"
    } else if app
        .orch_last_output
        .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3))
    {
        "chatting"
    } else {
        "idle"
    }
}

/// The brain glyph plus a single space, used as the pinned row / summary marker. The emoji is
/// double-width, so it leads each line on its own (lane rows below sit on separate grid lines and
/// are unaffected); any terminal width disagreement only touches this line's own trailing text.
const BRAIN: &str = "🧠 ";

/// The pinned "repomind" fleet row's label when repomind is asking the human something: a
/// permission/decision dialog reads as a question, an end-of-turn reads as waiting.
fn orch_attention_label(attention: &str) -> &'static str {
    match attention {
        "permission" | "decision" => "repomind · question for you",
        _ => "repomind · waiting for you", // "end_of_turn" and any future word
    }
}

/// The pinned "repomind" fleet row (rendered at the top of Fleet and the Split sidebar). Wears the
/// `needs_you` styling and wording whenever repomind has raised a dialog or ended its turn
/// (`app.orch_attention`), the same treatment a lane gets when its agent needs you.
fn orch_row_line(app: &App, selected: bool) -> Line<'static> {
    let label = match app.orch_attention.as_deref() {
        Some(attention) => orch_attention_label(attention).to_string(),
        None => format!("repomind · {}", orch_status_word(app)),
    };
    if selected {
        Line::from(Span::styled(
            format!("{BRAIN}{label}"),
            app.theme.selected(),
        ))
    } else if app.orch_attention.is_some() {
        Line::from(Span::styled(
            format!("{BRAIN}{label}"),
            app.theme.needs_you(),
        ))
    } else {
        let brain_style = if app.orch_running {
            app.theme.accented()
        } else {
            app.theme.muted()
        };
        Line::from(vec![
            Span::styled(BRAIN.to_string(), brain_style),
            Span::raw("repomind "),
            Span::styled(format!("· {}", orch_status_word(app)), app.theme.muted()),
        ])
    }
}

/// repomind's live pane windowed to `height` (respecting scroll), or `None` when it has no output
/// yet. Shared by the command-center's right column and the Split preview.
fn orch_pane_window(app: &App, height: usize) -> Option<Vec<Line<'static>>> {
    let pane = app.orch_output.as_ref()?;
    if pane.raw.trim().is_empty() {
        return None;
    }
    let (start, end) = output_window(pane.lines.len(), height.max(1), app.scroll);
    Some(pane.lines[start..end].to_vec())
}

/// The command-center: a curated fleet summary on the left, repomind's live pane on the right.
fn render_orchestrator(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(2), // header
        Constraint::Min(0),    // summary + pane
        Constraint::Length(1), // mode line
        Constraint::Length(1), // footer
    ])
    .split(area);
    f.render_widget(
        Paragraph::new(vec![
            header_line(area.width, "REPOMON · REPOMIND", &fmt_clock(), app),
            rule(area.width, true, app),
        ]),
        rows[0],
    );
    // A comfortably-readable summary column (fixed width) with repomind's pane taking the rest.
    let body = Layout::horizontal([
        Constraint::Length(30),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(rows[1]);
    // Left summary; its returned click map lets a click on a needs-you lane jump to it.
    let (summary, lane_hits) = orch_summary_lines(app, body[0].width);
    f.render_widget(Paragraph::new(summary), body[0]);
    for (line_idx, lane_id) in lane_hits {
        let y = body[0].y + line_idx as u16;
        if y < body[0].y + body[0].height {
            click_zone(
                app,
                Rect {
                    x: body[0].x,
                    y,
                    width: body[0].width,
                    height: 1,
                },
                lane_id,
                None,
                false,
            );
        }
    }
    let divider: Vec<Line> = (0..body[1].height)
        .map(|_| Line::from(Span::styled(theme::VLIGHT.to_string(), app.theme.muted())))
        .collect();
    f.render_widget(Paragraph::new(divider), body[1]);
    // Record the pane size and the rect for mouse focus/attach. The event loop's
    // sync_orchestrator_size reads focus_pane_dims each tick and calls orchestrator.resize so
    // the daemon reflows repomind's tmux window to fit (mirrors sync_pane_size for lane windows).
    app.focus_pane_dims
        .set(Some((body[2].width, body[2].height)));
    app.orch_pane_zone.set(Some(body[2]));
    let right = orch_pane_window(app, body[2].height as usize).unwrap_or_else(|| {
        let msg = if app.orch_running {
            "  repomind is starting… its live pane appears here"
        } else {
            "  repomind isn't running"
        };
        vec![
            Line::raw(""),
            Line::from(Span::styled(msg.to_string(), app.theme.dim())),
        ]
    });
    f.render_widget(Paragraph::new(right), body[2]);
    // Draw repomind's real cursor where you're typing (insert mode, live tail) — same mechanism as
    // the lane focus/insert pane.
    if app.orch_insert && app.scroll == 0 {
        if let Some(p) = app.orch_output.as_ref() {
            if let Some((cx, cy)) = p.cursor {
                let h = (body[2].height as usize).max(1);
                let start = p.lines.len().saturating_sub(h);
                let count = p.lines.len() - start;
                place_pane_cursor(f, body[2], start, count, (cx, cy));
            }
        }
    }

    let mode = if app.orch_insert {
        Line::from(Span::styled(
            " ● INSERT: typing to repomind · ↵ send · ^O to command ",
            app.theme.selected(),
        ))
    } else {
        Line::from(Span::styled(
            " ○ press i to message repomind · ↵/→ attach into its terminal ",
            app.theme.muted(),
        ))
    };
    f.render_widget(Paragraph::new(mode), rows[2]);
    let keys = if app.orch_insert {
        ORCH_INSERT_KEYS
    } else {
        ORCH_KEYS
    };
    f.render_widget(footer(keys, app, rows[3].width), rows[3]);
}

/// The left column of the command-center: fleet counts plus the (only) lanes blocked on the user.
/// Returns the lines and, for each rendered needs-you lane, its `(line index, lane id)` so the
/// caller can register a click zone that jumps to it.
fn orch_summary_lines(app: &App, width: u16) -> (Vec<Line<'static>>, Vec<(usize, LaneId)>) {
    let running = app
        .lanes
        .iter()
        .filter(|l| {
            l.agent_sessions
                .iter()
                .any(|s| !s.inferred && s.status == AgentStatus::Running)
        })
        .count();
    let needs: Vec<&Lane> = app
        .lanes
        .iter()
        .filter(|l| l.agent_sessions.iter().any(|s| s.status.needs_you()))
        .collect();
    let needs_style = if needs.is_empty() {
        app.theme.muted()
    } else {
        app.theme.needs_you()
    };
    let mut lines = vec![Line::from(Span::styled(
        format!("{BRAIN}REPOMIND · {}", orch_status_word(app)),
        app.theme.header_style(),
    ))];
    // repomind is asking the human something: the attention word plus a one-line "why", right
    // under the header so it's the first thing you see opening the command-center.
    if let Some(attention) = app.orch_attention.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("  {}", attention.replace('_', " ")),
            app.theme.needs_you(),
        )));
        if let Some(headline) = app.orch_headline.as_deref().filter(|h| !h.is_empty()) {
            let cap = (width as usize).saturating_sub(4).max(8);
            lines.push(Line::from(Span::styled(
                format!("  {}", trunc(headline, cap)),
                app.theme.muted(),
            )));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(format!("  {running} running"), app.theme.accented()),
        Span::styled(" · ".to_string(), app.theme.muted()),
        Span::styled(format!("{} need you", needs.len()), needs_style),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "NEEDS YOU".to_string(),
        app.theme.header_style(),
    )));
    let mut hits = Vec::new();
    if needs.is_empty() {
        lines.push(Line::from(Span::styled(
            "  all caught up".to_string(),
            app.theme.muted(),
        )));
    } else {
        // Trim names to the column so the line never clips mid-word.
        let cap = (width as usize).saturating_sub(5).max(8);
        for l in needs {
            hits.push((lines.len(), l.id));
            let name = format!("{}/{}", l.repo.name, lane_name(l));
            lines.push(Line::from(vec![
                Span::styled("  ⏸ ".to_string(), app.theme.needs_you()),
                Span::raw(trunc(&name, cap)),
            ]));
        }
    }
    (lines, hits)
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
    // Record the pane size so the event loop resizes the agent's tmux window to match (reflow to
    // the visible width — no right-edge clipping).
    app.focus_pane_dims
        .set(Some((body[2].width, body[2].height)));
    // With the pinned repomind row selected the right column previews its live pane; otherwise the
    // selected lane's live agent output, falling back to the lane's git detail. Shows the
    // scrollback window when scrolled (`scroll > 0`), else the live tail, both via `output_window`.
    let pinned = app.orchestrator_selected();
    let id = app.selected_lane().map(|l| l.id);
    let has_output = id
        .and_then(|i| app.output.get(&i))
        .map(|p| !p.raw.trim().is_empty())
        .unwrap_or(false);
    let right = if pinned {
        app.orch_pane_zone.set(Some(body[2]));
        orch_pane_window(app, body[2].height as usize).unwrap_or_else(|| {
            let msg = if app.orch_running {
                "  repomind is starting…"
            } else {
                "  repomind is off  ·  press ↵ to start"
            };
            vec![
                Line::raw(""),
                Line::from(Span::styled(msg.to_string(), app.theme.dim())),
            ]
        })
    } else if has_output {
        let h = (body[2].height as usize).max(1);
        let lines: &[Line<'static>] = if app.scroll > 0 {
            app.scroll_lines.as_deref().unwrap_or(&[])
        } else {
            id.and_then(|i| app.output.get(&i))
                .map_or(&[][..], |p| p.lines.as_slice())
        };
        let (start, end) = output_window(lines.len(), h, app.scroll);
        lines[start..end].to_vec()
    } else {
        detail_lines(app)
    };
    f.render_widget(Paragraph::new(right), body[2]);
    // Clicking the pane focuses the selected agent for typing (double-click → real terminal). The
    // pinned-row preview registers its own pane rect above instead (a click opens the command-center).
    if !pinned {
        if let Some(id) = id {
            click_zone(app, body[2], id, None, true);
        }
    }
    // Show the agent's text cursor where you're typing — INSERT mode at the live tail only.
    if app.focus_insert && app.scroll == 0 && has_output {
        if let Some(p) = id.and_then(|i| app.output.get(&i)) {
            if let Some((cx, cy)) = p.cursor {
                let h = (body[2].height as usize).max(1);
                let start = p.lines.len().saturating_sub(h);
                let count = p.lines.len() - start;
                place_pane_cursor(f, body[2], start, count, (cx, cy));
            }
        }
    }
    // The pinned repomind preview draws repomind's cursor while typing to it (same mechanism).
    if pinned && app.orch_insert && app.scroll == 0 {
        if let Some(p) = app.orch_output.as_ref() {
            if let Some((cx, cy)) = p.cursor {
                let h = (body[2].height as usize).max(1);
                let start = p.lines.len().saturating_sub(h);
                let count = p.lines.len() - start;
                place_pane_cursor(f, body[2], start, count, (cx, cy));
            }
        }
    }

    // INSERT here forwards keystrokes straight to the selected agent (or repomind, on the pinned
    // row) without leaving the Split view.
    let mode = if pinned && app.orch_insert {
        Line::from(Span::styled(
            " ● INSERT: keys go to repomind (esc · ⇧⇥ · ^C all sent) · ^O to command ",
            app.theme.selected(),
        ))
    } else if pinned {
        Line::from(Span::styled(
            " ○ i type to repomind · ↵/→ open the full command-center ",
            app.theme.muted(),
        ))
    } else if app.focus_insert {
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

    let keys = if pinned && app.orch_insert {
        SPLIT_ORCH_INSERT_KEYS
    } else if pinned {
        SPLIT_ORCH_KEYS
    } else if app.focus_insert {
        SPLIT_INSERT_KEYS
    } else {
        SPLIT_KEYS
    };
    f.render_widget(footer(keys, app, rows[3].width), rows[3]);
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
    // Record the pane size so the event loop fits the agent's tmux window to the full-screen view.
    app.focus_pane_dims.set(Some((rows[1].width, avail as u16)));
    body.extend(focus_output(app, lane.map(|l| l.id), avail, out_y0));
    f.render_widget(Paragraph::new(body), rows[1]);

    // Show the agent's text cursor where you're typing — INSERT mode at the live tail only.
    if app.focus_insert && app.scroll == 0 {
        if let Some((cx, cy)) = lane
            .and_then(|l| app.output.get(&l.id))
            .and_then(|p| p.cursor)
        {
            let (cur_y0, start, count) = app.focus_geom.get();
            let pane = Rect {
                x: rows[1].x,
                y: cur_y0,
                width: rows[1].width,
                height: count as u16,
            };
            place_pane_cursor(f, pane, start, count, (cx, cy));
        }
    }

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
    f.render_widget(footer(keys, app, rows[3].width), rows[3]);
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
        f.render_widget(footer(GRID_KEYS, app, rows[3].width), rows[3]);
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
            click_zone(app, cell, ids[idx], None, true);
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
    f.render_widget(footer(keys, app, rows[3].width), rows[3]);
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
        // Name what the wait is (the most urgent one when agents disagree): a real question,
        // a routine permission ask, or a finished turn — instead of a generic "needs you".
        let word = match max_waiting_attention(sessions) {
            Attention::Decision => "question",
            Attention::Permission => "permission",
            _ => "done",
        };
        (format!("⏸ {word}{count}{tag}"), app.theme.needs_you())
    } else if running > 0 {
        (format!("▶ running{count}{tag}"), app.theme.running())
    } else {
        (format!("idle{count}{tag}"), app.theme.idle())
    }
}

/// The most urgent attention among a lane's waiting sessions (inferred placeholders never
/// count) — what a "⏸" actually is: a question, a permission ask, or a finished turn.
fn max_waiting_attention(sessions: &[repomon_core::model::AgentSession]) -> Attention {
    use repomon_core::model::AgentStatus;
    sessions
        .iter()
        .filter(|s| !s.inferred && s.status == AgentStatus::Waiting)
        .map(agent_attention)
        .max_by_key(|a| a.priority())
        .unwrap_or(Attention::None)
}

/// The waiting glyph for one session: `?` question, `⏸` permission, `✓` finished turn.
fn waiting_glyph(sess: &repomon_core::model::AgentSession) -> &'static str {
    match agent_attention(sess) {
        Attention::Decision => theme::WAIT_QUESTION,
        Attention::Permission => theme::WAITING,
        _ => theme::WAIT_DONE,
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

/// The bottom-right corner: usage for the focused agent's account (Claude `/usage` or Codex
/// `/status`) — `5h NN% · wk NN% · <reset>`, drawn over the free right end of the last row of
/// *every* view. Falls back to the focused lane's rate-limit countdown when there's no scraped
/// usage for that account, and draws nothing when there's nothing to show — never fake numbers.
fn corner(f: &mut Frame, app: &App) {
    let Some((text, style)) = corner_text(app) else {
        return;
    };
    let area = f.area();
    if area.height == 0 || area.width == 0 || text.is_empty() {
        return;
    }
    let w = (text.chars().count() as u16).min(area.width);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w),
        y: area.y + area.height - 1,
        width: w,
        height: 1,
    };
    f.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), rect);
}

/// What the corner should show, if anything: scraped usage for the focused agent's account
/// (matched by key, so a Codex agent never shows Claude's numbers), else that lane's rate-limit
/// countdown, else nothing.
fn corner_text(app: &App) -> Option<(String, Style)> {
    if let Some(key) = app.focused_account_key() {
        if let Some(u) = app.usage.iter().find(|u| u.key == key) {
            if let Some(found) = format_usage(app, u) {
                return Some(found);
            }
        }
    }
    corner_fallback(app)
}

/// Format one account's usage as `[label · ]<win> NN% · <win> NN% · <reset>` from its limit
/// windows (the first two — usually 5h + weekly), plus the soonest reset. The account label shows
/// only when more than one account is in play. `None` when there are no windows.
fn format_usage(app: &App, u: &repomon_core::agent::AccountUsage) -> Option<(String, Style)> {
    let windows = &u.report.windows;
    if windows.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if app.usage.len() > 1 {
        parts.push(u.label.clone());
    }
    for w in windows.iter().take(2) {
        parts.push(format!("{} {}%", w.label, w.pct_used));
    }
    if let Some(t) = windows.iter().find_map(|w| w.reset_at) {
        parts.push(fmt_reset_short(t));
    }
    // Cyan warning once the tightest window is getting full; muted otherwise.
    let tight = windows.first().is_some_and(|w| w.pct_used >= 80);
    let style = if tight {
        app.theme.rate_limited()
    } else {
        app.theme.muted()
    };
    Some((parts.join(" · "), style))
}

/// A reset time for the corner: a clock time when it's within a day, else a short date (Codex's
/// monthly reset can be weeks out).
fn fmt_reset_short(t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&Local);
    if (t - Utc::now()).num_hours().abs() < 24 {
        local.format("%-I:%M %p").to_string()
    } else {
        local.format("%b %-d").to_string()
    }
}

/// The focused lane's rate-limit countdown, when usage numbers aren't available — so the corner
/// stays useful even if the probe is off or failed.
fn corner_fallback(app: &App) -> Option<(String, Style)> {
    use repomon_core::model::AgentStatus;
    let lane = app.selected_lane()?;
    let rl = lane
        .agent_sessions
        .iter()
        .find(|s| s.status == AgentStatus::RateLimited)?;
    Some((
        format!("⏳ {}", fmt_resume(rl.resume_at)),
        app.theme.rate_limited(),
    ))
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
pub fn parse_pane(raw: &str) -> Vec<Line<'static>> {
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

/// Place the real terminal cursor at the agent pane's cursor, when it falls inside the visible
/// window of the rendered pane. `area` is the pane's screen rect; `start` is the index of the first
/// visible captured line and `count` how many are shown; `(cx, cy)` is the agent cursor in
/// captured-pane coordinates (col, row). No-op when the cursor row is scrolled out of view or the
/// column runs past the pane width.
fn place_pane_cursor(f: &mut Frame, area: Rect, start: usize, count: usize, (cx, cy): (u16, u16)) {
    let cy = cy as usize;
    if cy < start || cy >= start + count {
        return;
    }
    let row = (cy - start) as u16;
    if row >= area.height || cx >= area.width {
        return;
    }
    f.set_cursor_position((area.x + cx, area.y + row));
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
    f.render_widget(footer(NEWLANE_KEYS, app, rows[1].width), rows[1]);
}

// ---- shared line builders ----------------------------------------------------

#[allow(clippy::type_complexity)]
fn fleet_lines(
    app: &App,
    content: Rect,
) -> (
    Vec<Line<'static>>,
    Option<usize>,
    Vec<(usize, LaneId, Option<usize>)>,
    Option<usize>,
) {
    let width = content.width;
    let now = Utc::now();
    let mut lines = Vec::new();
    // The selected row's line index (so the caller can scroll to keep it visible) and every row's
    // (line index, lane, agent session) so a click maps back through the scroll offset.
    let mut selected_line: Option<usize> = None;
    let mut lane_rows: Vec<(usize, LaneId, Option<usize>)> = Vec::new();
    // The pinned repomind row's line index, so the caller can register its (scroll-adjusted) click.
    let mut brain_line: Option<usize> = None;
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

    let rows = app.fleet_rows();
    let mut current: Option<RepoId> = None;
    for (ri, row) in rows.iter().enumerate() {
        let selected = ri == app.selected;
        // The pinned repomind row sits above the lanes; it targets no lane, so render + record it
        // before the lane lookup below.
        if row.orchestrator {
            let li = lines.len();
            brain_line = Some(li);
            if selected {
                selected_line = Some(li);
            }
            lines.push(orch_row_line(app, selected));
            lines.push(Line::raw(""));
            continue;
        }
        let Some(&lane) = visible.get(row.lane_idx) else {
            continue;
        };
        if row.session.is_none() && current != Some(lane.repo.id) {
            if current.is_some() {
                lines.push(Line::raw("")); // a gap between repo groups
            }
            current = Some(lane.repo.id);
            lines.push(repo_header(width, &lane.repo.name, app));
        }
        let mut line = match row.session {
            None => lane_row(lane, now, app, selected),
            Some(s) => match lane.agent_sessions.get(s) {
                Some(sess) => agent_subrow(sess, app, selected),
                None => continue, // index stale (lane list changed mid-frame) — skip defensively
            },
        };
        if !selected && row.session.is_none() && app.hover_lane == Some(lane.id) {
            line = line.style(app.theme.hover());
        }
        let li = lines.len();
        lane_rows.push((li, lane.id, row.session));
        if selected {
            selected_line = Some(li);
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
    (lines, selected_line, lane_rows, brain_line)
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
    let _ = now;
    let rows = app.fleet_rows();
    let mut current: Option<RepoId> = None;
    for (ri, row) in rows.iter().enumerate() {
        let selected = ri == app.selected;
        // The pinned repomind row sits above the lanes; record its click rect and continue.
        if row.orchestrator {
            let li = lines.len() as u16;
            if li < content.height {
                app.orch_click.set(Some(Rect {
                    x: content.x,
                    y: content.y + li,
                    width: content.width,
                    height: 1,
                }));
            }
            lines.push(orch_row_line(app, selected));
            lines.push(Line::raw(""));
            continue;
        }
        let Some(&lane) = visible.get(row.lane_idx) else {
            continue;
        };
        if row.session.is_none() && current != Some(lane.repo.id) {
            current = Some(lane.repo.id);
            lines.push(Line::from(Span::styled(
                lane.repo.name.clone(),
                app.theme.muted(),
            )));
        }
        let mut line = match row.session {
            None => {
                let glyph = status_glyph(lane);
                let counts = dirty_str(&lane.state.dirty);
                let rest = format!(" {:<10} {}", trunc(&lane_name(lane), 10), counts);
                let count = agent_count_badge(lane);
                if selected {
                    let badge = count
                        .as_deref()
                        .map(|c| format!(" {c}"))
                        .unwrap_or_default();
                    Line::from(Span::styled(
                        format!(" {glyph}{rest}{badge}"),
                        app.theme.selected(),
                    ))
                } else {
                    let glyph_style = if lane.state.dirty.is_clean() {
                        app.theme.muted()
                    } else {
                        app.theme.accented()
                    };
                    let mut spans = vec![
                        Span::raw(" "),
                        Span::styled(glyph.to_string(), glyph_style),
                        Span::raw(rest),
                    ];
                    if let Some(c) = &count {
                        spans.push(Span::styled(format!(" {c}"), app.theme.accented()));
                    }
                    Line::from(spans)
                }
            }
            Some(s) => match lane.agent_sessions.get(s) {
                Some(sess) => agent_subrow(sess, app, selected),
                None => continue, // index stale (lane list changed mid-frame) — skip defensively
            },
        };
        if !selected && row.session.is_none() && app.hover_lane == Some(lane.id) {
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
                row.session,
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

fn footer(keys: &str, app: &App, width: u16) -> Paragraph<'static> {
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
    // Reserve room for the bottom-right usage corner (drawn on top by `corner()`), so the hint bar
    // never slides under it — measured exactly as `corner()` measures, in chars.
    let reserve = corner_text(app).map_or(0, |(t, _)| t.chars().count() as u16 + 1);
    let budget = width.saturating_sub(reserve);
    Paragraph::new(Line::from(footer_spans(keys, app, budget)))
}

/// Render a footer hint string into styled spans: each item's leading key token(s) in the accent
/// (bold), labels and separators muted, and the source's `"  ·  "` group breaks promoted to a muted
/// `│` rail. Truncates at item boundaries to `budget` columns, ending in a muted `" …"`, so the
/// usage corner keeps clear space. Glyph widths are counted in chars (as `corner()` does).
fn footer_spans(keys: &str, app: &App, budget: u16) -> Vec<Span<'static>> {
    let muted = app.theme.muted();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used: u16 = 0;
    let mut truncated = false;
    // Groups are the literal "  ·  " (two-space middot two-space): real group breaks are always that
    // exact run, while every in-prose middot is single-spaced " · " inside parens, so this never
    // fractures the *_INSERT strings.
    'outer: for group in keys.split("  ·  ") {
        for (ii, item) in split_items_depth0(group).into_iter().enumerate() {
            // First item of a non-first group gets the "│" rail; later items get the "·" dot.
            let sep = if spans.is_empty() {
                ""
            } else if ii == 0 {
                " │ "
            } else {
                " · "
            };
            let (item_spans, item_w) = key_label_spans(item, app);
            if item_w == 0 {
                continue;
            }
            let sep_w = sep.chars().count() as u16;
            if used + sep_w + item_w > budget {
                truncated = true;
                break 'outer;
            }
            if !sep.is_empty() {
                spans.push(Span::styled(sep.to_string(), muted));
                used += sep_w;
            }
            spans.extend(item_spans);
            used += item_w;
        }
    }
    if truncated {
        spans.push(Span::styled(" …".to_string(), muted));
    }
    spans
}

/// Split a hint group into items on `" · "`, but only at paren depth 0 — so the middots inside a
/// parenthetical like `"(esc · ⇧⇥ · ^C sent)"` never split the prose. Items are trimmed.
fn split_items_depth0(group: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = group.char_indices().collect();
    let mut items: Vec<&str> = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let (bi, c) = chars[i];
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ => {}
        }
        // A " · " boundary (space, middot, space) only counts at the top level.
        if depth == 0
            && c == ' '
            && chars.get(i + 1).is_some_and(|&(_, c1)| c1 == '·')
            && chars.get(i + 2).is_some_and(|&(_, c2)| c2 == ' ')
        {
            items.push(group[start..bi].trim());
            let (last_bi, last_c) = chars[i + 2];
            start = last_bi + last_c.len_utf8();
            i += 3;
            continue;
        }
        i += 1;
    }
    items.push(group[start..].trim());
    items.into_iter().filter(|s| !s.is_empty()).collect()
}

/// Style one hint item as `<key> <label>`: an accent (bold) key span plus a muted label span.
/// Returns the spans and their width in chars. A bare key (e.g. `"q"`) yields just the accent span.
fn key_label_spans(item: &str, app: &App) -> (Vec<Span<'static>>, u16) {
    let (key, label) = split_key_label(item);
    if key.is_empty() {
        return (Vec::new(), 0);
    }
    let mut w = key.chars().count() as u16;
    let mut spans = vec![Span::styled(key, app.theme.footer_key())];
    if let Some(label) = label {
        let label = format!(" {label}");
        w += label.chars().count() as u16;
        spans.push(Span::styled(label, app.theme.muted()));
    }
    (spans, w)
}

/// Split a hint item into its key and optional label. The first whitespace token is always the key,
/// and if it is purely symbolic, following symbolic tokens join it (so `"↑↓ ↵ open"` → key `"↑↓ ↵"`,
/// label `"open"`). A bare key (e.g. `"q"`) returns `(key, None)`.
fn split_key_label(item: &str) -> (String, Option<String>) {
    let mut toks = item.split_whitespace();
    let Some(first) = toks.next() else {
        return (String::new(), None);
    };
    let mut key = first.to_string();
    let chain = is_glyph_token(first);
    let mut rest: Vec<&str> = Vec::new();
    for t in toks {
        if rest.is_empty() && chain && is_glyph_token(t) {
            key.push(' ');
            key.push_str(t);
        } else {
            rest.push(t);
        }
    }
    let label = (!rest.is_empty()).then(|| rest.join(" "));
    (key, label)
}

/// Whether a token is made entirely of key glyphs (arrows, enter, tab, shift, modifiers) — used to
/// chain a second symbolic key onto the first, e.g. the `"↑↓ ↵"` in `"↑↓ ↵ open"`.
fn is_glyph_token(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| "↑↓←→↵⇥⇧^/+*,".contains(c))
}

fn header_line(width: u16, left: &str, right: &str, app: &App) -> Line<'static> {
    // The view title is the accent; the clock on the right is muted. Unseen notifications get
    // a ⚑ badge beside the clock, visible from every view (`5` opens the feed and clears it).
    let unread = app.unread_notifs();
    let badge = if unread > 0 {
        format!("⚑ {unread} · ")
    } else {
        String::new()
    };
    let used = left.chars().count() + badge.chars().count() + right.chars().count();
    let pad = (width as usize).saturating_sub(used);
    Line::from(vec![
        Span::styled(left.to_string(), app.theme.header_style()),
        Span::raw(" ".repeat(pad)),
        Span::styled(badge, app.theme.needs_you()),
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

/// `Some("×N")` when several agents share one lane — the compact multi-agent marker shown in the
/// Fleet list and the Split sidebar (mirrors the Grid badge's `×N`). `None` for 0 or 1 agent.
fn agent_count_badge(lane: &Lane) -> Option<String> {
    let n = lane.agent_sessions.len();
    (n > 1).then(|| format!("×{n}"))
}

/// A short 1-4 word summary of an agent session for the expanded sidebar: the user's `custom_label`
/// when set, else the first few words of the session's title (its opening prompt), else its last
/// message, falling back to the agent name.
fn agent_summary(sess: &repomon_core::model::AgentSession) -> String {
    if let Some(l) = &sess.custom_label {
        if !l.is_empty() {
            return l.clone();
        }
    }
    let src = sess
        .title
        .as_deref()
        .or(sess.last_message.as_deref())
        .unwrap_or("");
    let words: Vec<&str> = src.split_whitespace().take(4).collect();
    if words.is_empty() {
        sess.agent.short().to_string()
    } else {
        words.join(" ")
    }
}

/// One agent sub-row under an expanded lane: `↳ summary   <status glyph>`. While the selected row
/// is being renamed, shows the live edit buffer with a cursor instead of the summary.
fn agent_subrow(
    sess: &repomon_core::model::AgentSession,
    app: &App,
    selected: bool,
) -> Line<'static> {
    use repomon_core::model::AgentStatus;
    let (glyph, gstyle) = match sess.status {
        AgentStatus::RateLimited => (theme::RATE_LIMITED, app.theme.rate_limited()),
        AgentStatus::Waiting => (waiting_glyph(sess), app.theme.needs_you()),
        AgentStatus::Running => (theme::AGENT_ACTIVE, app.theme.running()),
        _ if sess.inferred => (theme::INFERRED_ACTIVE, app.theme.dim()),
        _ => ("·", app.theme.dim()),
    };
    let summary = if selected && app.renaming {
        format!("{}_", app.rename_buf) // live rename buffer + cursor
    } else {
        trunc(&agent_summary(sess), 30)
    };
    if selected {
        return Line::from(Span::styled(
            format!("     ↳ {summary}  {glyph}"),
            app.theme.selected(),
        ));
    }
    Line::from(vec![
        Span::raw("     ↳ "),
        Span::styled(format!("{summary:<30}"), app.theme.muted()),
        Span::raw(" "),
        Span::styled(glyph.to_string(), gstyle),
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
        // ? question · ⏸ permission · ✓ finished turn — all amber "blocked on you".
        let glyph = match max_waiting_attention(&lane.agent_sessions) {
            Attention::Decision => theme::WAIT_QUESTION,
            Attention::Permission => theme::WAITING,
            _ => theme::WAIT_DONE,
        };
        (glyph, app.theme.needs_you())
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
    let agent_name = lane
        .agent_sessions
        .first()
        .map(|s| trunc(s.agent.short(), 7))
        .unwrap_or_default();
    // "claude" or "claude ×2"; the count gets its own accent span so a multi-agent lane stands
    // out. The cell is padded to a fixed width so the time column stays aligned either way.
    let count = agent_count_badge(lane);
    const AGENT_W: usize = 10;
    let agent_cell = match &count {
        Some(c) => format!("{agent_name} {c}"),
        None => agent_name.clone(),
    };
    let agent_pad = " ".repeat(AGENT_W.saturating_sub(agent_cell.chars().count()));
    let left = format!(
        "{:<12} {:<36} {:<11} {:<7} ",
        trunc(&lane_name(lane), 12),
        trunc(&lane_branch(lane), 36),
        dirty_str(&lane.state.dirty),
        ahead_behind_str(lane.state.ahead, lane.state.behind),
    );
    let time = rel_time(lane.last_activity_at, now);
    if selected {
        // A clean reverse-video bar for the selection (per-cell colors would muddy it).
        let text = format!("  {glyph} {active} {left}{agent_cell}{agent_pad} {time}");
        return Line::from(Span::styled(text, app.theme.selected()));
    }
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(glyph.to_string(), glyph_style),
        Span::raw(" "),
        Span::styled(active.to_string(), active_style),
        Span::raw(" "),
        Span::raw(left),
        Span::raw(agent_name),
    ];
    if let Some(c) = &count {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(c.clone(), app.theme.accented()));
    }
    spans.push(Span::raw(format!("{agent_pad} ")));
    spans.push(Span::styled(time, app.theme.muted()));
    Line::from(spans)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_split_only_at_top_level() {
        // A plain group: every " · " is a boundary.
        assert_eq!(
            split_items_depth0("↑↓ lane · tab session"),
            vec!["↑↓ lane", "tab session"],
        );
        // The middots inside the parenthetical must NOT split the prose item.
        assert_eq!(
            split_items_depth0("keys → agent (esc · ⇧⇥ · ^C sent) · PgUp/PgDn scroll"),
            vec!["keys → agent (esc · ⇧⇥ · ^C sent)", "PgUp/PgDn scroll"],
        );
    }

    #[test]
    fn insert_strings_keep_two_groups_with_prose_intact() {
        // SPLIT_INSERT_KEYS / FOCUS_INSERT_KEYS shape: group split on "  ·  " then paren-aware items.
        let groups: Vec<Vec<&str>> = SPLIT_INSERT_KEYS
            .split("  ·  ")
            .map(split_items_depth0)
            .collect();
        assert_eq!(groups.len(), 2);
        // The first item is the whole prose sentence, parenthetical and all.
        assert_eq!(groups[0][0], "keys → agent (esc · ⇧⇥ · ^C sent)");

        let focus: Vec<Vec<&str>> = FOCUS_INSERT_KEYS
            .split("  ·  ")
            .map(split_items_depth0)
            .collect();
        assert_eq!(focus.len(), 2);
        assert_eq!(focus[0][0], "keys → agent (esc · ⇧⇥ · ^C sent)");
    }

    #[test]
    fn key_label_splits_and_chains_glyphs() {
        // Single symbolic key + label.
        assert_eq!(
            split_key_label("i quick-type"),
            ("i".into(), Some("quick-type".into())),
        );
        // Two-key glyph run: both arrows chain into the key.
        assert_eq!(
            split_key_label("↑↓ ↵ open"),
            ("↑↓ ↵".into(), Some("open".into())),
        );
        // Bare key, no label.
        assert_eq!(split_key_label("q"), ("q".into(), None));
        // Multi-word labels are preserved (paren prose rides in the label).
        assert_eq!(
            split_key_label("keys → agent (esc · ⇧⇥ · ^C sent)"),
            ("keys".into(), Some("→ agent (esc · ⇧⇥ · ^C sent)".into()),),
        );
        // A word-key does NOT chain a following arrow into the key.
        assert_eq!(
            split_key_label("wheel/PgUp scroll"),
            ("wheel/PgUp".into(), Some("scroll".into())),
        );
    }
}
