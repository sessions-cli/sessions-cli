pub mod app;
pub mod art_canvas;
pub mod art_encode;
pub mod client;
pub mod directory_discovery;
pub mod editor;
pub mod group_order;
pub mod host_terminal;
pub mod keys;
pub mod mouse_cursor;
pub mod new_session;
pub mod notepad;
pub mod overlay;
pub mod panel_popup;
pub mod path_picker;
pub mod settings;
pub mod sidebar_ui;
pub mod ui;

use crate::color_env;
use crate::config::Config;
use anyhow::Result;

pub fn run_bar(socket_path: Option<std::path::PathBuf>) -> Result<()> {
    color_env::force_process_color_env();
    let mut config = Config::default();
    if let Some(p) = socket_path {
        config.socket_path = p;
    }
    let mut app = app::App::new(&config)?;
    app.run()
}

pub fn run_settings() -> Result<()> {
    color_env::force_process_color_env();
    overlay::run_settings(&Config::default())
}

pub fn run_new_session() -> Result<()> {
    color_env::force_process_color_env();
    overlay::run_new_session(&Config::default())
}

pub fn run_automations() -> Result<()> {
    color_env::force_process_color_env();
    overlay::run_automations(&Config::default())
}

pub fn run_skills() -> Result<()> {
    color_env::force_process_color_env();
    overlay::run_skills(&Config::default())
}

pub fn run_mcps() -> Result<()> {
    color_env::force_process_color_env();
    overlay::run_mcps(&Config::default())
}
