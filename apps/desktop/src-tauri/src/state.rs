use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use repomon_core::client::DaemonClient;
use tokio::sync::OnceCell;
use tokio::sync::oneshot;

use crate::connection::ConnectionSnapshot;

pub struct AppState {
    pub client: OnceCell<DaemonClient>,
    pub connection: RwLock<ConnectionSnapshot>,
    pub terminal_watches: Arc<Mutex<HashMap<String, oneshot::Sender<oneshot::Sender<()>>>>>,
    endpoint: String,
}

impl AppState {
    pub fn new(endpoint: PathBuf) -> Self {
        let endpoint = endpoint.to_string_lossy().into_owned();
        Self {
            client: OnceCell::new(),
            connection: RwLock::new(ConnectionSnapshot::starting(&endpoint)),
            terminal_watches: Arc::new(Mutex::new(HashMap::new())),
            endpoint,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
