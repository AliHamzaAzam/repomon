//! Rendering for the four views. Brutalist mono: light/heavy rules, single-char glyphs,
//! two-space indents, reverse-video selection, no color.

use chrono::{DateTime, Local, Utc};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use repomon_core::model::{Commit, DirtyState, Lane, LaneId, RepoId};

use crate::app::App;
use crate::keybinds::View;
use crate::theme;

const FLEET_KEYS: &str =
    "↑↓ ↵ open  n new  d del  / filter  g needs-you  c cd  2 timeline  3 sessions  4 search  q quit";
const SPLIT_KEYS: &str = "↑↓ switch  ↵/→ focus  e spawn  a attach  spc grid  ←/esc back  q quit";
const FOCUS_CMD_KEYS: &str = "i/↵ type  e spawn  s stop  a attach  m merge  c cd  ←/esc back";
const FOCUS_INSERT_KEYS: &str = "type to the agent   ↵ send   esc command-mode";
const GRID_KEYS: &str = "↑↓←→ move  ↵ focus  e spawn  s stop  p pin  spc/f fleet  q quit";
const NEWLANE_KEYS: &str = "↑↓ repo  tab agent  type branch  ↵ create + spawn  esc cancel";

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
    }
}

const DASH_KEYS: &str = "1 fleet  2 timeline  3 sessions  4 search  ←/esc fleet  q quit";

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
            "e export-md  1 fleet  2 timeline  3 sessions  4 search  q quit",
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
    f.render_widget(footer("type query  ←/esc fleet", app), rows[1]);
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
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
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
    f.render_widget(footer(SPLIT_KEYS, app), rows[2]);
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
    let avail = (rows[1].height as usize).saturating_sub(1);
    body.extend(output_tail(app, lane.map(|l| l.id), avail));
    f.render_widget(Paragraph::new(body), rows[1]);

    let input = if app.focus_insert {
        format!("› {}_", app.input)
    } else {
        "› press i to type to the agent".to_string()
    };
    f.render_widget(Paragraph::new(Line::raw(input)), rows[2]);

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
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    let ids = app.grid_lane_ids();
    let header = format!(
        "REPOMON · BABYSIT {} of {}",
        ids.len(),
        app.visible_lanes().len()
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
            Paragraph::new(Line::raw(
                "  no lanes to babysit — pin some with p, or add a repo",
            )),
            rows[1],
        );
        f.render_widget(footer(GRID_KEYS, app), rows[2]);
        return;
    }

    // Two columns; rows as needed for the visible tiles.
    let cols = if area.width >= 80 { 2 } else { 1 };
    let tile_rows = ids.len().div_ceil(cols);
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
            if idx >= ids.len() {
                continue;
            }
            let lane = app.lanes.iter().find(|l| l.id == ids[idx]);
            let active = idx == app.grid_active;
            f.render_widget(
                Paragraph::new(tile_lines(
                    app,
                    lane,
                    ids[idx],
                    cell.height as usize,
                    active,
                )),
                *cell,
            );
        }
    }
    f.render_widget(footer(GRID_KEYS, app), rows[2]);
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
    match lane.agent_sessions.first() {
        Some(s) if s.status == AgentStatus::Waiting => {
            format!("⏸ needs you  {}↻", s.tool_call_count)
        }
        Some(s) if s.status == AgentStatus::Running => format!("▶ {}↻", s.tool_call_count),
        Some(_) => "idle".to_string(),
        None => String::new(),
    }
}

/// The last `height` lines of a lane's captured output, or a placeholder.
fn output_tail(app: &App, lane_id: Option<LaneId>, height: usize) -> Vec<Line<'static>> {
    let content = lane_id.and_then(|id| app.output.get(&id));
    match content {
        Some(text) if !text.trim().is_empty() => {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(height.max(1));
            lines[start..]
                .iter()
                .map(|l| Line::raw(l.to_string()))
                .collect()
        }
        _ => vec![Line::from(Span::styled(
            "(no live output — press e to start claude here)".to_string(),
            app.theme.dim(),
        ))],
    }
}

fn focus_status_line(lane: &Lane) -> String {
    let s = &lane.state;
    let branch = lane_branch(lane);
    let ab = ahead_behind_str(s.ahead, s.behind);
    match lane.agent_sessions.first() {
        Some(sess) => format!(
            "{} · {} {} · {} · {} calls",
            sess.agent.short(),
            branch,
            ab,
            sess.status.as_str(),
            sess.tool_call_count
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
    let agent = crate::app::AGENT_KINDS[app.nl_agent_idx % crate::app::AGENT_KINDS.len()];
    lines.push(Line::raw(format!("  repo      {repo_name}")));
    lines.push(Line::raw(format!("  agent     {agent}   [tab to change]")));
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
            "  no lanes — press n to create one, or add a repo".to_string(),
        ));
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
    lines.push(Line::raw(
        "  agent    none running  (spawn arrives in Phase 2)".to_string(),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("RECENT COMMITS", app.theme.bold())));
    lines.push(Line::raw(""));
    let mut shown = 0;
    for c in app
        .commits
        .iter()
        .filter(|c| c.repo_id == lane.repo.id)
        .take(8)
    {
        lines.push(commit_line(c, app));
        shown += 1;
    }
    if shown == 0 {
        lines.push(Line::raw("  (no commits today)".to_string()));
    }
    lines
}

// ---- atoms -------------------------------------------------------------------

fn footer(keys: &str, app: &App) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(keys.to_string(), app.theme.dim())))
}

fn header_line(width: u16, left: &str, right: &str, app: &App) -> Line<'static> {
    padded_line(width, left, right, app.theme.bold())
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
    let active = if lane
        .agent_sessions
        .iter()
        .any(|s| s.status == AgentStatus::Waiting)
    {
        theme::WAITING // ⏸ needs you
    } else if lane
        .agent_sessions
        .iter()
        .any(|s| s.status == AgentStatus::Running)
    {
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
