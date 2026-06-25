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