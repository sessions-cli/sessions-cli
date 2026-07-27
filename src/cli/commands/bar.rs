use crate::bar;
use anyhow::Result;
use std::path::PathBuf;

pub fn run(socket: Option<PathBuf>) -> Result<()> {
    bar::run_bar(socket)
}

pub fn run_settings() -> Result<()> {
    bar::run_settings()
}

pub fn run_new_session() -> Result<()> {
    bar::run_new_session()
}

pub fn run_automations() -> Result<()> {
    bar::run_automations()
}

pub fn run_skills() -> Result<()> {
    bar::run_skills()
}

pub fn run_mcps() -> Result<()> {
    bar::run_mcps()
}
