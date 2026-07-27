//! Automations overlay panel (list + create/edit).

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

pub use state::AutomationsAction;

use input::{handle_key, handle_mouse};
use render::{draw_screen, ClickTargets};
use state::{AutomationsState, Mode, PanelHover};

pub fn run_automations(config: &Config) -> Result<()> {
    let mut state = AutomationsState::load(config)?;
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
    state: &mut AutomationsState,
    hover: &mut PanelHover,
    targets: &mut ClickTargets,
) -> Result<AutomationsAction> {
    let mut last_reload = std::time::Instant::now();
    loop {
        terminal.draw(|frame| {
            *targets = draw_screen(frame, state, hover);
        })?;
        if !event::poll(Duration::from_millis(80))? {
            if state.mode == Mode::List && last_reload.elapsed() > Duration::from_secs(2) {
                let _ = state.reload(config);
                last_reload = std::time::Instant::now();
            }
            continue;
        }
        match event::read()? {
            Event::Key(key)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
            {
                match handle_key(state, config, key)? {
                    AutomationsAction::Close => return Ok(AutomationsAction::Close),
                    AutomationsAction::Unchanged => {}
                }
            }
            Event::Mouse(mouse) => match handle_mouse(state, config, mouse, targets, hover)? {
                AutomationsAction::Close => return Ok(AutomationsAction::Close),
                AutomationsAction::Unchanged => {}
            },
            Event::Resize(_, _) => {}
            Event::Paste(text) => state.apply_paste(&text),
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
        crossterm::event::EnableBracketedPaste,
    )?;
    Ok(Terminal::new(CrosstermBackend::new(io::stdout()))?)
}

fn teardown_terminal() -> Result<()> {
    crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}
