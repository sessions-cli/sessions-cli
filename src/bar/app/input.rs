use super::{parse_digit_ordinal, App};
use crate::bar::keys::{has_command_modifier, has_paste_modifier};
use crate::bar::ui::{self, ToolbarAction};
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
            Event::FocusLost => {}
            Event::FocusGained => {
                self.refresh_sidebar_mouse_capture();
                self.pointer_hover_refresh_pending = true;
            }
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
                    return self.handle_delete_note_confirm_key(*key);
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
                {
                    if matches!(key.code, KeyCode::Char(',')) {
                        self.clear_close_mode();
                        self.run_toolbar_action(ToolbarAction::Settings);
                        return Ok(());
                    }
                    // ⌘1–⌘0 jump to sessions even while a note is focused.
                    if let KeyCode::Char(c @ '0'..='9') = key.code {
                        self.clear_close_mode();
                        self.clear_digit_buffer();
                        if let Some(ordinal) = parse_digit_ordinal(&c.to_string()) {
                            self.jump_to_ordinal(ordinal);
                        }
                        return Ok(());
                    }
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
                // Collapsed rail: most keys expand first so navigation works immediately.
                // Width keys (`[`/`]`/`{`/`}`) go through resize_sidebar_by so shrink
                // on a rail stays a no-op and grow expands cleanly.
                if self.is_sidebar_rail_collapsed()
                    && !matches!(
                        key.code,
                        KeyCode::Char('b')
                            | KeyCode::Char('B')
                            | KeyCode::Esc
                            | KeyCode::Char('c')
                            | KeyCode::Char('q')
                            | KeyCode::Char('[')
                            | KeyCode::Char(']')
                            | KeyCode::Char('{')
                            | KeyCode::Char('}')
                    )
                {
                    self.expand_sidebar_from_rail();
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
                        // ⌘1–⌘9 / ⌘0 → jump to sidebar ordinal (same as plain digits /
                        // `sessions focus N`). 0 maps to 10.
                        KeyCode::Char(c @ '0'..='9') => {
                            self.clear_close_mode();
                            self.clear_digit_buffer();
                            if let Some(ordinal) = parse_digit_ordinal(&c.to_string()) {
                                self.jump_to_ordinal(ordinal);
                            }
                            return Ok(());
                        }
                        // ⌘[ / ⌘] (or Meta from host) — same as plain [ / ] width step.
                        KeyCode::Char('[') | KeyCode::Char('{') => {
                            let large = matches!(key.code, KeyCode::Char('{'))
                                || key.modifiers.contains(KeyModifiers::SHIFT);
                            let step = if large {
                                ui::KEYBOARD_RESIZE_STEP_LARGE
                            } else {
                                ui::KEYBOARD_RESIZE_STEP
                            };
                            self.resize_sidebar_by(-(step as i16));
                            return Ok(());
                        }
                        KeyCode::Char(']') | KeyCode::Char('}') => {
                            let large = matches!(key.code, KeyCode::Char('}'))
                                || key.modifiers.contains(KeyModifiers::SHIFT);
                            let step = if large {
                                ui::KEYBOARD_RESIZE_STEP_LARGE
                            } else {
                                ui::KEYBOARD_RESIZE_STEP
                            };
                            self.resize_sidebar_by(step as i16);
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
                    KeyCode::Esc if !self.close_modifier_held && self.sidebar_force_expanded => {
                        self.collapse_sidebar_rail_if_narrow();
                    }
                    KeyCode::Esc if !self.close_modifier_held => {}
                    KeyCode::Char('b') | KeyCode::Char('B') => {
                        self.clear_close_mode();
                        self.toggle_sidebar_rail();
                    }
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
                    // Sidebar width: [ narrower, ] wider (step 4). Shift/{/} use step 10.
                    // Preferred over IDE edge-drag (Cursor/xterm.js grip is unreliable).
                    KeyCode::Char('[') => {
                        let step = if key.modifiers.contains(KeyModifiers::SHIFT) {
                            ui::KEYBOARD_RESIZE_STEP_LARGE
                        } else {
                            ui::KEYBOARD_RESIZE_STEP
                        };
                        self.resize_sidebar_by(-(step as i16));
                    }
                    KeyCode::Char(']') => {
                        let step = if key.modifiers.contains(KeyModifiers::SHIFT) {
                            ui::KEYBOARD_RESIZE_STEP_LARGE
                        } else {
                            ui::KEYBOARD_RESIZE_STEP
                        };
                        self.resize_sidebar_by(step as i16);
                    }
                    KeyCode::Char('{') => {
                        self.resize_sidebar_by(-(ui::KEYBOARD_RESIZE_STEP_LARGE as i16));
                    }
                    KeyCode::Char('}') => {
                        self.resize_sidebar_by(ui::KEYBOARD_RESIZE_STEP_LARGE as i16);
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
                let size = terminal.size()?;
                let metrics = self.layout_metrics(size);
                self.note_pointer_activity(mouse, size.width);
                let was_edge_resizing = self.edge_resize_active;
                self.handle_mouse(mouse, &metrics);
                // Live edge-drag: skip full paints on every Drag sample — tmux
                // width change + Resize events drive layout. Paint when the drag
                // ends (or for normal hover/select motion).
                let edge_drag_sample = was_edge_resizing
                    && matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left));
                if !edge_drag_sample
                    && matches!(
                        mouse.kind,
                        MouseEventKind::Moved | MouseEventKind::Drag(_) | MouseEventKind::Up(_)
                    )
                {
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
