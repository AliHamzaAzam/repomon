//! Wire shapes for `agent.command_catalog`: the native model picker and slash-command palette
//! read these instead of driving an agent's interactive picker blind. Discovery lives in
//! `repomon-daemon`'s `command_catalog` module, which is the only writer of these values; never
//! invent an entry here or there - an empty list is correct, a fabricated one is not.
use serde::{Deserialize, Serialize};

/// Where a catalog command came from, so the palette can group and namespace it the way the
/// operator's reference does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CatalogSource {
    /// Shipped with the agent itself; not discoverable on disk, curated by hand.
    Builtin,
    /// A user-defined command file.
    User,
    /// A command a plugin provides.
    Plugin,
    /// Discovered but not attributable to one of the above.
    Unknown,
}

/// One slash command the palette can show, without its leading slash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct CatalogCommand {
    pub name: String,
    /// May be empty; never invented when the source does not state one.
    pub description: String,
    pub source: CatalogSource,
    /// True when this command can be sent as one fully specified line (its argument included)
    /// with no interactive state to steer. False keeps the existing terminal route.
    pub one_shot: bool,
}

/// One selectable model, as the model panel shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct CatalogModel {
    pub id: String,
    pub label: String,
    pub current: bool,
}

/// The full answer to `agent.command_catalog`. `models` is empty when the kind's model set is
/// not knowable; `model_command` is `None` when this kind has no way to set a model at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct CommandCatalog {
    pub commands: Vec<CatalogCommand>,
    pub models: Vec<CatalogModel>,
    pub model_command: Option<String>,
}

impl CommandCatalog {
    pub fn empty() -> Self {
        CommandCatalog {
            commands: Vec::new(),
            models: Vec::new(),
            model_command: None,
        }
    }
}
