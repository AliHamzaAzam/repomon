//! View modes and arrow-first key mapping.
//!
//! Arrow keys drive navigation at every level; `hjkl` are aliases. `↵`/`→` zoom in,
//! `esc`/`←` zoom out, `space` toggles the babysit grid (Phase 2).

use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// The current zoom level / modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Fleet,
    Split,
    /// One agent, full-screen: live output + input + controls.
    Focus,
    /// Babysit grid of live tiles.
    Grid,
    NewLane,
    /// Per-repo commit-density timeline (Phase 3).
    Timeline,
    /// Detected work sessions (Phase 3).
    Sessions,
    /// Global commit search (Phase 3).
    Search,
    /// Interactive repo browser (add repos by exploring the filesystem).
    AddRepo,
    /// Manage agent launch commands (add/edit/delete customs, set the default).
    Agents,
}

/// A user intent derived from a key press in navigation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    MoveUp,
    MoveDown,
    ZoomIn,
    ZoomOut,
    Quit,
    NewLane,
    DeleteLane,
    StartFilter,
    Refresh,
    CdToLane,
    ToggleBabysit,
    JumpNeedsYou,
    StopAgent,
    Pin,
    Merge,
    SpawnAgent,
    AdoptAgent,
    OpenTerminal,
    AttachTerminal,
    ToggleMouse,
    Goto(View),
}

/// Map a key to a navigation action (used when not in a text-input mode).
pub fn nav(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveUp),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveDown),
        KeyCode::Right | KeyCode::Enter | KeyCode::Char('l') => Some(Action::ZoomIn),
        KeyCode::Left | KeyCode::Esc | KeyCode::Char('h') => Some(Action::ZoomOut),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('n') => Some(Action::NewLane),
        KeyCode::Char('d') | KeyCode::Char('x') => Some(Action::DeleteLane),
        KeyCode::Char('/') => Some(Action::StartFilter),
        KeyCode::Char('r') => Some(Action::Refresh),
        KeyCode::Char('c') => Some(Action::CdToLane),
        KeyCode::Char('g') => Some(Action::JumpNeedsYou),
        KeyCode::Char('a') => Some(Action::Goto(View::AddRepo)),
        KeyCode::Char('A') => Some(Action::Goto(View::Agents)),
        KeyCode::Char('s') => Some(Action::StopAgent),
        KeyCode::Char('p') => Some(Action::Pin),
        KeyCode::Char('m') => Some(Action::Merge),
        KeyCode::Char('e') => Some(Action::SpawnAgent),
        KeyCode::Char('o') => Some(Action::AdoptAgent),
        KeyCode::Char('t') => Some(Action::OpenTerminal),
        KeyCode::Char('T') => Some(Action::AttachTerminal),
        KeyCode::Char('y') => Some(Action::ToggleMouse),
        KeyCode::Char(' ') => Some(Action::ToggleBabysit),
        KeyCode::Char('1') => Some(Action::Goto(View::Fleet)),
        KeyCode::Char('2') => Some(Action::Goto(View::Timeline)),
        KeyCode::Char('3') => Some(Action::Goto(View::Sessions)),
        KeyCode::Char('4') => Some(Action::Goto(View::Search)),
        _ => None,
    }
}
