//! `repomon` — the terminal UI client. Thin wrapper around [`repomon_tui::run_cli`].

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    repomon_tui::run_cli().await
}
