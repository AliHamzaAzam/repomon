use std::path::PathBuf;
use std::sync::RwLock;

use repomon_core::client::DaemonClient;
use tokio::sync::OnceCell;

use crate::connection::ConnectionSnapshot;

pub struct AppState {
    pub client: OnceCell<DaemonClient>,
    pub connection: RwLock<ConnectionSnapshot>,
    endpoint: String,
}

impl AppState {
    pub fn new(endpoint: PathBuf) -> Self {
        let endpoint = endpoint.to_string_lossy().into_owned();
        Self {
            client: OnceCell::new(),
            connection: RwLock::new(ConnectionSnapshot::starting(&endpoint)),
            endpoint,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
