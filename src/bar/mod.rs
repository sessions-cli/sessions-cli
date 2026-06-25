pub mod app;
pub mod editor;
pub mod art_canvas;
pub mod art_encode;
pub mod client;
pub mod group_order;
pub mod sidebar_ui;
pub mod keys;
pub mod mouse_cursor;
pub mod new_session;
pub mod notepad;
pub mod overlay;
pub mod panel_popup;
pub mod directory_discovery;
pub mod settings;
pub mod ui;

use crate::config::Config;
use anyhow::Result;

pub fn run_bar(socket_path: Option<std::path::PathBuf>) -> Result<()> {
    let mut config = Config::default();
    if let Some(p) = socket_path {
        config.socket_path = p;
    }
    let mut app = app::App::new(&config)?;
    app.run()
}

pub fn run_settings() -> Result<()> {
    overlay::run_settings(&Config::default())
}

pub fn run_new_session() -> Result<()> {
    overlay::run_new_session(&Config::default())
}