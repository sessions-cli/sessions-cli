//! Skills management portal — inventory, drift, and sync across agents.

mod input;
mod render;
mod state;

use crate::bar::mouse_cursor;
use crate::config::Config;
use crate::daemon::tmux;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Duration;

pub use state::SkillsAction;

use input::{handle_key, handle_mouse};
use render::{draw_screen, ClickTargets};
use state::{PanelHover, SkillsState};

pub fn run_skills(config: &Config) -> Result<()> {
    let mut state = SkillsState::load(config)?;
    let mut hover = PanelHover::default();
    let mut targets = ClickTargets::default();
    let _ = tmux::write_host_terminal_backdrop();
    tmux::enable_pane_graphics_passthrough(None);
    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, config, &mut state, &mut hover, &mut targets);
    let _ = mouse_cursor::reset_mouse_cursor();
    teardown_terminal()?;
    result.map(|_| ())
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &Config,
    state: &mut SkillsState,
    hover: &mut PanelHover,
    targets: &mut ClickTargets,
) -> Result<SkillsAction> {
    loop {
        state.drain_setup(config);
        terminal.draw(|frame| {
            *targets = draw_screen(frame, state, hover);
        })?;
        if !event::poll(Duration::from_millis(80))? {
            continue;
        }
        match event::read()? {
            Event::Key(key)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
            {
                match handle_key(state, config, key)? {
                    SkillsAction::Close => return Ok(SkillsAction::Close),
                    SkillsAction::Unchanged => {}
                }
            }
            Event::Mouse(mouse) => match handle_mouse(state, config, mouse, targets, hover)? {
                SkillsAction::Close => return Ok(SkillsAction::Close),
                SkillsAction::Unchanged => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
    )?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn teardown_terminal() -> Result<()> {
    crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen,
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}
