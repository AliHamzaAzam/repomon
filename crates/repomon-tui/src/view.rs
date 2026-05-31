//! Rendering for the four views. Brutalist mono: light/heavy rules, single-char glyphs,
//! two-space indents, reverse-video selection, no color.

use chrono::{DateTime, Local, Utc};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use repomon_core::model::{Commit, DirtyState, Lane, LaneId, RepoId};

use crate::app::{AgField, App};
use crate::keybinds::View;
use crate::theme;

const FLEET_KEYS: &str =
    "↑↓ ↵ open  ·  n new · e spawn · t term  ·  a add-repo · A agents · d del  ·  / filter · g needs-you · C auto-cont  ·  2 timeline · 3 sessions · 4 search  ·  spc grid · q";
const SPLIT_KEYS: &str =
    "↑↓ lane · tab session  ·  ↵ open (real terminal) · → focus · i quick-type  ·  e spawn · o adopt · C auto-cont  ·  ←/esc back";
const SPLIT_INSERT_KEYS: &str = "keys → agent (esc · ⇧⇥ · ^C sent)  ·  ^O command-mode";
const FOCUS_CMD_KEYS: &str =
    "↵/→ open (real terminal) · i quick-type · PgUp scroll  ·  e spawn · o adopt · t term · s stop  ·  ←/esc back";
const FOCUS_INSERT_KEYS: &str = "keys → agent (esc · ⇧⇥ · ^C sent)  ·  ^O command-mode";
const GRID_KEYS: &str =
    "←→ move · ↵ open (real terminal)  ·  e spawn · s stop · p pin  ·  spc/esc fleet · q quit";
const NEWLANE_KEYS: &str =
    "↑↓ repo · tab agent · ^a manage  ·  type branch · ↵ create + spawn  ·  esc cancel";

/// Render the current view.
pub fn render(f: &mut Frame, app: &App) {
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
            rule(area.width, true),
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

const ADDREPO_KEYS: &str =
    "↑↓ select · ↵/→ enter · ←/h up  ·  a add repo · d discover here  ·  esc back";

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
            rule(area.width, true),
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
        rule(area.width, true),
        Line::raw(""),
    ];
    match &app.timeline {
        Some(t) if !t.rows.is_empty() => {
            let zoom = match app.timeline_zoom {
                crate::app::Zoom::Day => "day",
                crate::app::Zoom::Week => "week",
                crate::app::Zoom::Month => "month",
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{} · {} buckets   [d]ay [w]eek [m]onth",
                    zoom,
                    t.rows.first().map(|r| r.density.len()).unwrap_or(0)
                ),
                app.theme.dim(),
            )));
            lines.push(Line::raw(""));
            let label_w = t
                .rows
                .iter()
                .map(|r| r.repo_name.len())
                .max()
                .unwrap_or(8)
                .min(20);
            for row in &t.rows {
                let bars: String = row.density.iter().map(|&l| analytics_char(l)).collect();
                lines.push(Line::raw(format!(
                    "  {:<label_w$}  {}",
                    trunc(&row.repo_name, label_w),
                    bars
                )));
            }
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled("CORRELATIONS", app.theme.bold())));
            lines.push(rule(area.width, false));
            lines.push(Line::raw(""));
            if t.correlations.is_empty() {
                lines.push(Line::raw("  (none above threshold)".to_string()));
            }
            for c in t.correlations.iter().take(8) {
                lines.push(Line::raw(format!(
                    "  {} ↔ {}     {} windows     {:.2} overlap",
                    c.a, c.b, c.windows, c.overlap
                )));
            }
        }
        _ => lines.push(Line::raw(
            "  no commit history yet (the indexer runs in the background)".to_string(),
        )),
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(footer(DASH_KEYS, app), rows[1]);
}

fn render_sessions(f: &mut Frame, app: &App) {
    use repomon_core::model::SessionKind;
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let total: i64 = app.sessions.iter().map(|s| s.duration_minutes()).sum();
    let mut lines = vec![
        header_line(area.width, "REPOMON · SESSIONS", &fmt_clock(), app),
        rule(area.width, true),
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

fn render_search(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let mut lines = vec![
        header_line(area.width, "REPOMON · SEARCH", &fmt_clock(), app),
        rule(area.width, true),
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
    f.render_widget(Paragraph::new(fleet_lines(app, area.width)), rows[0]);
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
            rule(area.width, true),
        ]),
        rows[0],
    );
    let body = Layout::horizontal([Constraint::Length(26), Constraint::Min(0)]).split(rows[1]);
    f.render_widget(Paragraph::new(sidebar_lines(app)), body[0]);
    // Live agent output if there is any, otherwise the lane's git detail.
    let id = app.selected_lane().map(|l| l.id);
    let has_output = id
        .and_then(|i| app.output.get(&i))
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let right = if has_output {
        output_tail(app, id, body[1].height as usize)
    } else {
        detail_lines(app)
    };
    f.render_widget(Paragraph::new(right), body[1]);

    // INSERT here forwards keystrokes straight to the selected agent (no need to zoom).
    let mode = if app.focus_insert {
        Line::from(Span::styled(
            " ● INSERT — keys go to the agent (esc · ⇧⇥ · ^C all sent) · ^O to command ",
            app.theme.selected(),
        ))
    } else {
        Line::from(Span::styled(
            " ○ i type to the selected agent · ↵/→ full-screen focus ",
            app.theme.dim(),
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
            rule(area.width, true),
        ]),
        rows[0],
    );

    let mut body: Vec<Line> = Vec::new();
    if let Some(l) = lane {
        body.push(Line::from(Span::styled(
            focus_status_line(l),
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
            app.theme.dim(),
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
            rule(area.width, true),
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

    // Two columns; rows as needed for the visible tiles.
    let cols = if area.width >= 80 { 2 } else { 1 };
    let tile_rows = n.div_ceil(cols);
    let row_constraints: Vec<Constraint> = (0..tile_rows)
        .map(|_| Constraint::Ratio(1, tile_rows as u32))
        .collect();
    let grid_rows = Layout::vertical(row_constraints).split(rows[1]);

    for (r, row_area) in grid_rows.iter().enumerate() {
        let col_constraints: Vec<Constraint> = (0..cols)
            .map(|_| Constraint::Ratio(1, cols as u32))
            .collect();
        let cells = Layout::horizontal(col_constraints).split(*row_area);
        for (c, cell) in cells.iter().enumerate() {
            let idx = r * cols + c;
            if idx >= n {
                continue;
            }
            let lane = app.lanes.iter().find(|l| l.id == ids[idx]);
            f.render_widget(
                Paragraph::new(tile_lines(
                    app,
                    lane,
                    ids[idx],
                    cell.height as usize,
                    idx == active,
                )),
                *cell,
            );
        }
    }

    // Instagram-style position indicator at the bottom (above the footer): a dot per tile,
    // filled for the active one, plus its name — so what's selected is clear at a glance.
    let dots: String = (0..n)
        .map(|i| if i == active { "●" } else { "○" })
        .collect::<Vec<_>>()
        .join(" ");
    let label = app
        .lanes
        .iter()
        .find(|l| l.id == ids[active])
        .map(|l| format!("{}/{}", l.repo.name, lane_name(l)))
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {dots}    {}/{n}  {label}", active + 1),
            app.theme.bold(),
        ))),
        rows[2],
    );
    f.render_widget(footer(GRID_KEYS, app), rows[3]);
}

fn tile_lines(
    app: &App,
    lane: Option<&Lane>,
    id: LaneId,
    height: usize,
    active: bool,
) -> Vec<Line<'static>> {
    let marker = if active { "▎" } else { " " };
    let head = match lane {
        Some(l) => format!(
            "{marker}{}/{}  {}",
            l.repo.name,
            lane_name(l),
            agent_badge(l)
        ),
        None => format!("{marker}lane {id}"),
    };
    let style = if active {
        app.theme.bold()
    } else {
        app.theme.dim()
    };
    let mut lines = vec![Line::from(Span::styled(head, style))];
    lines.extend(output_tail(app, Some(id), height.saturating_sub(1)));
    lines
}

fn agent_badge(lane: &Lane) -> String {
    use repomon_core::model::AgentStatus;
    let sessions = &lane.agent_sessions;
    if sessions.is_empty() {
        return String::new();
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
        format!("⏳ rate-limited · {}{count}{tag}", fmt_resume(rl.resume_at))
    } else if waiting > 0 {
        format!("⏸ needs you{count}{tag}")
    } else if running > 0 {
        format!("▶ running{count}{tag}")
    } else {
        format!("idle{count}{tag}")
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

/// The last `height` lines of a lane's captured output, ANSI-colored, or a placeholder.
fn output_tail(app: &App, lane_id: Option<LaneId>, height: usize) -> Vec<Line<'static>> {
    use ansi_to_tui::IntoText;
    let content = lane_id.and_then(|id| app.output.get(&id));
    match content {
        Some(text) if !text.trim().is_empty() => {
            // Parse the captured pane (with `-e` escapes) into styled lines.
            let mut lines = text
                .into_text()
                .map(|t| t.lines)
                .unwrap_or_else(|_| text.lines().map(|l| Line::raw(l.to_string())).collect());
            // Drop trailing blank rows tmux pads the pane with.
            while lines.last().map(line_is_blank).unwrap_or(false) {
                lines.pop();
            }
            let start = lines.len().saturating_sub(height.max(1));
            lines.split_off(start)
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
    use ansi_to_tui::IntoText;
    let raw: String = if app.scroll > 0 {
        app.scroll_buf.clone().unwrap_or_default()
    } else {
        lane_id
            .and_then(|id| app.output.get(&id))
            .cloned()
            .unwrap_or_default()
    };
    if raw.trim().is_empty() {
        app.focus_geom.set((out_y0, 0, 0));
        return vec![Line::from(Span::styled(
            "(no live output — press e to start claude here)".to_string(),
            app.theme.dim(),
        ))];
    }
    let mut lines: Vec<Line<'static>> = raw
        .into_text()
        .map(|t| t.lines)
        .unwrap_or_else(|_| raw.lines().map(|l| Line::raw(l.to_string())).collect());
    while lines.last().map(line_is_blank).unwrap_or(false) {
        lines.pop();
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

fn focus_status_line(lane: &Lane) -> String {
    let s = &lane.state;
    let branch = lane_branch(lane);
    let ab = ahead_behind_str(s.ahead, s.behind);
    match lane.agent_sessions.first() {
        Some(sess) => format!(
            "{} · {} {} · {} · {} calls{}",
            sess.agent.short(),
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
        rule(area.width, true),
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
    lines.push(rule(area.width, false));
    lines.push(Line::raw(""));
    if !app.status.is_empty() {
        lines.push(Line::raw(format!("  {}", app.status)));
    }
    f.render_widget(Paragraph::new(lines), rows[0]);
    f.render_widget(footer(NEWLANE_KEYS, app), rows[1]);
}

// ---- shared line builders ----------------------------------------------------

fn fleet_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let now = Utc::now();
    let mut lines = Vec::new();
    lines.push(header_line(width, "REPOMON", &fmt_clock(), app));
    lines.push(rule(width, true));
    lines.push(Line::raw(""));

    let visible = app.visible_lanes();
    let repos = distinct_repos(&visible);
    let needs = visible
        .iter()
        .filter(|l| l.agent_sessions.iter().any(|s| s.status.needs_you()))
        .count();
    let left = format!(
        "FLEET  {} lanes · {repos} repos · {needs} need you",
        visible.len()
    );
    let right = format!("today · {} commits", app.commits.len());
    lines.push(padded_line(width, &left, &right, app.theme.bold()));
    lines.push(rule(width, false));
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
            current = Some(lane.repo.id);
            lines.push(repo_header(width, &lane.repo.name));
        }
        let mut line = lane_row(lane, now);
        if i == app.selected {
            line = line.style(app.theme.selected());
        }
        lines.push(line);
    }

    lines.push(Line::raw(""));
    lines.push(padded_line(width, "TODAY", &tz_offset(), app.theme.bold()));
    lines.push(rule(width, false));
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

fn sidebar_lines(app: &App) -> Vec<Line<'static>> {
    let now = Utc::now();
    let visible = app.visible_lanes();
    let mut lines = vec![
        Line::from(Span::styled(
            format!("FLEET {}", visible.len()),
            app.theme.bold(),
        )),
        Line::raw(""),
    ];
    let mut current: Option<RepoId> = None;
    for (i, lane) in visible.iter().enumerate() {
        if current != Some(lane.repo.id) {
            current = Some(lane.repo.id);
            lines.push(Line::from(Span::styled(
                lane.repo.name.clone(),
                app.theme.dim(),
            )));
        }
        let glyph = status_glyph(lane);
        let counts = dirty_str(&lane.state.dirty);
        let text = format!(" {glyph} {:<10} {}", trunc(&lane_name(lane), 10), counts);
        let mut line = Line::raw(text);
        let _ = now;
        if i == app.selected {
            line = line.style(app.theme.selected());
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
            app.theme.bold(),
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
            app.theme.bold(),
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
            lines.push(Line::raw(format!(
                "  {cursor} {glyph} {kind}  {:<40}  {}",
                trunc(&label, 40),
                trailer
            )));
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
    lines.push(Line::from(Span::styled("RECENT COMMITS", app.theme.bold())));
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
    Paragraph::new(Line::from(Span::styled(keys.to_string(), app.theme.dim())))
}

fn header_line(width: u16, left: &str, right: &str, app: &App) -> Line<'static> {
    padded_line(width, left, right, app.theme.header_style())
}

fn padded_line(width: u16, left: &str, right: &str, left_style: Style) -> Line<'static> {
    let used = left.chars().count() + right.chars().count();
    let pad = (width as usize).saturating_sub(used);
    Line::from(vec![
        Span::styled(left.to_string(), left_style),
        Span::raw(" ".repeat(pad)),
        Span::raw(right.to_string()),
    ])
}

fn rule(width: u16, heavy: bool) -> Line<'static> {
    let c = if heavy { theme::HEAVY } else { theme::LIGHT };
    Line::raw(c.to_string().repeat(width as usize))
}

fn repo_header(width: u16, name: &str) -> Line<'static> {
    let prefix = format!("  {name} ");
    let dashes = (width as usize).saturating_sub(prefix.chars().count());
    Line::raw(format!(
        "{prefix}{}",
        theme::LIGHT.to_string().repeat(dashes)
    ))
}

fn lane_row(lane: &Lane, now: DateTime<Utc>) -> Line<'static> {
    use repomon_core::model::AgentStatus;
    let glyph = status_glyph(lane);
    let any = |st: AgentStatus| lane.agent_sessions.iter().any(|s| s.status == st);
    let active = if any(AgentStatus::RateLimited) {
        theme::RATE_LIMITED // ⏳ paused on a usage limit, auto-continuing
    } else if any(AgentStatus::Waiting) {
        theme::WAITING // ⏸ needs you
    } else if any(AgentStatus::Running) {
        theme::AGENT_ACTIVE // ▶ working
    } else {
        " "
    };
    let agent = lane
        .agent_sessions
        .first()
        .map(|s| s.agent.short().to_string())
        .unwrap_or_default();
    let text = format!(
        "  {glyph} {active} {:<12} {:<26} {:<11} {:<7} {:<7} {}",
        trunc(&lane_name(lane), 12),
        lane_branch(lane),
        dirty_str(&lane.state.dirty),
        ahead_behind_str(lane.state.ahead, lane.state.behind),
        agent,
        rel_time(lane.last_activity_at, now),
    );
    Line::raw(text)
}

fn commit_line(c: &Commit, app: &App) -> Line<'static> {
    let time = c.time.with_timezone(&Local).format("%H:%M").to_string();
    let repo = app
        .lanes
        .iter()
        .find(|l| l.repo.id == c.repo_id)
        .map(|l| l.repo.name.clone())
        .unwrap_or_else(|| format!("repo{}", c.repo_id));
    Line::raw(format!("  {time}  {:<18} {}", trunc(&repo, 18), c.summary))
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
