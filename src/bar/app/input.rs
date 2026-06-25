use super::App;
use crate::bar::keys::{has_command_modifier, has_paste_modifier};
use crate::bar::ui::ToolbarAction;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

impl App {
    pub(crate) fn handle_event(
        &mut self,
        event: &Event,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        match event {
            Event::FocusLost => {
                self.clear_pointer_hover_states();
            }
            Event::FocusGained => {}
            Event::Paste(text) => {
                return self.handle_paste_event(text, terminal);
            }
            Event::Key(key) => {
                if has_paste_modifier(key.modifiers)
                    && matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'))
                    && key.kind != KeyEventKind::Release
                {
                    return self.handle_paste_key(terminal);
                }
                if self.delete_note_confirm.is_some() {
                    return self.handle_delete_note_confirm_key(*key, terminal);
                }
                if self.rename.is_some() {
                    return self.handle_rename_key(*key, terminal);
                }
                if self.notepad_focused {
                    if !self.handle_notepad_d_for_close_mode(*key, terminal)? {
                        return Ok(());
                    }
                } else if self.handle_close_mode_d_key(*key, terminal)? {
                    return Ok(());
                }
                if self.notepad_focused
                    && key.kind != KeyEventKind::Release
                    && has_command_modifier(key.modifiers)
                    && matches!(key.code, KeyCode::Char(','))
                {
                    self.clear_close_mode();
                    self.run_toolbar_action(ToolbarAction::Settings);
                    return Ok(());
                }
                if self.notepad_focused {
                    return self.handle_notepad_key(*key, terminal);
                }
                if self.context_menu.is_some() && key.code == KeyCode::Esc {
                    self.context_menu = None;
                    self.force_redraw();
                    return Ok(());
                }
                if key.kind == KeyEventKind::Release {
                    return Ok(());
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('c') | KeyCode::Char('q') => {
                            self.detach_client();
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                if has_command_modifier(key.modifiers) {
                    match key.code {
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            self.clear_close_mode();
                            self.run_toolbar_action(ToolbarAction::NewSession);
                            return Ok(());
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') => {
                            self.clear_close_mode();
                            self.create_session();
                            return Ok(());
                        }
                        KeyCode::Char('g') | KeyCode::Char('G') => {
                            self.clear_close_mode();
                            self.create_agent_session("grok");
                            return Ok(());
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            self.clear_close_mode();
                            self.create_agent_session("claude");
                            return Ok(());
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            self.clear_close_mode();
                            self.create_agent_session("codex");
                            return Ok(());
                        }
                        KeyCode::Char('o') | KeyCode::Char('O') => {
                            self.clear_close_mode();
                            self.create_agent_session("opencode");
                            return Ok(());
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            self.clear_close_mode();
                            self.run_toolbar_action(ToolbarAction::Search);
                            return Ok(());
                        }
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            self.clear_close_mode();
                            self.run_toolbar_action(ToolbarAction::Automations);
                            return Ok(());
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            self.clear_close_mode();
                            self.run_toolbar_action(ToolbarAction::Mcps);
                            return Ok(());
                        }
                        KeyCode::Char('k') | KeyCode::Char('K') => {
                            self.clear_close_mode();
                            self.run_toolbar_action(ToolbarAction::Skills);
                            return Ok(());
                        }
                        KeyCode::Char(',') => {
                            self.clear_close_mode();
                            self.run_toolbar_action(ToolbarAction::Settings);
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                match key.code {
                    KeyCode::Esc if self.close_modifier_held => {
                        self.disengage_close_mode();
                        self.force_redraw();
                    }
                    KeyCode::Esc if !self.close_modifier_held && self.workspace_panel_open() => {
                        self.restore_workspace_if_panel_open();
                    }
                    KeyCode::Esc if !self.close_modifier_held => {}
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        self.clear_close_mode();
                        if !self.try_start_rename_for_hover() {
                            let _ = self.client.refresh();
                            self.rows_version = self.rows_version.wrapping_add(1);
                        }
                    }
                    KeyCode::Char('x') => {
                        self.clear_close_mode();
                        self.confirm_close_session();
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.clear_close_mode();
                        self.move_selection(-1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.clear_close_mode();
                        self.move_selection(1);
                    }
                    KeyCode::Enter if self.close_modifier_held => {
                        if let Some(row) = self.close_target_row {
                            if self.row_is_close_target_note(row) {
                                self.request_delete_note_confirm_at_row(row);
                            } else if self.session_at(row).is_some() {
                                self.close_row(row);
                            } else {
                                self.close_selected_session();
                            }
                        } else {
                            self.close_selected_session();
                        }
                    }
                    KeyCode::Enter => {
                        self.clear_close_mode();
                        self.activate_selected();
                    }
                    KeyCode::Char(c @ '0'..='9') => {
                        self.clear_close_mode();
                        self.push_digit(c);
                    }
                    KeyCode::Char('.') => {
                        self.clear_close_mode();
                        self.toggle_notepad_expanded(true);
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
                let size = terminal.size()?;
                let metrics = self.layout_metrics(size);
                if matches!(mouse.kind, MouseEventKind::Down(_))
                    || matches!(mouse.kind, MouseEventKind::Up(_))
                {
                    let _ = crate::daemon::tmux::select_own_pane();
                }
                self.note_pointer_activity(mouse, size.width);
                self.handle_mouse(mouse, &metrics);
                if matches!(
                    mouse.kind,
                    MouseEventKind::Moved | MouseEventKind::Drag(_) | MouseEventKind::Up(_)
                ) {
                    self.redraw_if_needed(terminal)?;
                }
            }
            Event::Resize(width, height) => {
                self.handle_terminal_resize(*width, *height);
            }
        }
        Ok(())
    }
}