use crate::notify;
use anyhow::Result;

pub fn run(event: &str, payload: Option<&str>, stdin: bool) -> Result<()> {
    let use_stdin = stdin || notify::hook_reads_stdin(event, payload);
    notify::run_notify(event, payload, use_stdin)
}