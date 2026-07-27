use super::{App, SIDEBAR_POINTER_CURSOR_HOLD};
use crate::bar::ui::{self, ToolbarAction};
use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::{Duration, Instant};

impl App {
    pub(crate) fn sidebar_trail_base(&self) -> usize {
        ui::sidebar_trail_base_row(self.rows.len(), self.sessions_expanded)
    }
    pub(crate) fn layout_metrics(&self, size: ratatui::layout::Size) -> ui::LayoutMetrics {
        ui::layout_metrics_with_notepad(
            size,
            &self.rows,
            self.sessions_expanded,
            &self.notes,
            self.notepad_expanded,
            self.update_banner.is_some(),
        )
    }
    pub(crate) fn needs_run_spinner_animation(&self) -> bool {
        if self.is_boot_loading() {
            return true;
        }
        // Need a known frame size to know the viewport. Before first paint,
        // skip continuous spinner animation for off-screen workers.
        let Some((w, h)) = self.render_cache.size else {
            return false;
        };
        // Collapsed sessions section hides session rows from the list; the
        // narrow rail still paints status chips aligned to list rows.
        let rail = self.is_sidebar_rail_collapsed() || ui::is_collapsed_sidebar_width(w);
        if !self.sessions_expanded && !rail {
            return false;
        }
        let metrics = self.layout_metrics(ratatui::layout::Size::new(w, h));
        ui::needs_run_spinner_animation_in_viewport(&self.rows, self.scroll, metrics.list_height)
    }
    pub(crate) fn needs_continuous_animation(&self) -> bool {
        self.needs_run_spinner_animation()
            || self.needs_coming_soon_animation()
            || (self.notepad_focused && self.notepad_expanded)
    }

    /// Unfocused, no open panels, no animation, no recent pointer — probe slowly.
    /// Daemon snapshots still surface new/closed windows on the normal poll cadence.
    pub(crate) fn is_deep_idle(&self) -> bool {
        !self.sidebar_pane_focused
            && !self.edge_resize_active
            && !self.workspace_settings_open
            && !self.workspace_new_session_open
            && !self.workspace_automations_open
            && !self.workspace_mcps_open
            && !self.workspace_skills_open
            && self.workspace_panel_open_grace_until.is_none()
            && !self.needs_continuous_animation()
            && self.last_mouse_activity.elapsed() > super::SIDEBAR_POINTER_CURSOR_HOLD
    }

    /// Interactive probes stay fast; deep idle backs off to [`super::IDLE_PROBE_INTERVAL`].
    pub(crate) fn effective_probe_interval(&self, interactive: Duration) -> Duration {
        if self.is_deep_idle() {
            super::IDLE_PROBE_INTERVAL.max(interactive)
        } else {
            interactive
        }
    }

    pub(crate) fn workspace_pane_has_focus(&self) -> bool {
        self.last_workspace_pane_focused
    }
    pub(crate) fn pointer_in_sidebar_column(&self) -> bool {
        self.last_mouse.is_some_and(|mouse| {
            let width = self.user_pane_width.unwrap_or(u16::MAX);
            mouse.column < width
        })
    }
    /// Fast poll while the sidebar pane is focused and the pointer is over it.
    ///
    /// Hover is only tracked when the sidebar owns focus. We deliberately do
    /// **not** steal tmux focus from the workspace to sample MouseMove — that
    /// focus flash caused cross-pane rendering artifacts in the nested client.
    pub(crate) fn needs_sidebar_hover_poll(&self) -> bool {
        self.sidebar_pane_focused
            && !self.last_workspace_pane_focused
            && self.pointer_in_sidebar_column()
            && self.last_mouse_activity.elapsed() < SIDEBAR_POINTER_CURSOR_HOLD
    }
    pub(crate) fn needs_pointer_hover_poll(&self) -> bool {
        self.needs_sidebar_hover_poll()
    }
    pub(crate) fn animation_interval_ms(&self) -> u64 {
        if self.needs_run_spinner_animation() {
            ui::RUN_SPINNER_INTERVAL_MS
        } else {
            ui::COMING_SOON_INTERVAL_MS
        }
    }
    pub(crate) fn advance_anim_frame(&mut self) {
        if !self.needs_continuous_animation() {
            return;
        }
        if self.last_anim_tick.elapsed() < Duration::from_millis(self.animation_interval_ms()) {
            return;
        }
        self.anim_frame = self.anim_frame.wrapping_add(1);
        self.last_anim_tick = Instant::now();
    }
    pub(crate) fn needs_coming_soon_animation(&self) -> bool {
        !self.coming_soon_anims.is_empty()
    }
    pub(crate) fn active_coming_soon_frames(&self) -> Vec<(ToolbarAction, usize)> {
        if self.coming_soon_anims.is_empty() {
            return Vec::new();
        }
        let mut frames: Vec<_> = self
            .coming_soon_anims
            .iter()
            .filter_map(|(action, started)| {
                let frame =
                    (started.elapsed().as_millis() as u64 / ui::COMING_SOON_INTERVAL_MS) as usize;
                if frame < ui::COMING_SOON_CYCLE_FRAMES {
                    Some((*action, frame))
                } else {
                    None
                }
            })
            .collect();
        frames.sort_by_key(|(action, _)| *action);
        frames
    }
    pub(crate) fn expire_coming_soon_anims_if_due(&mut self) {
        self.coming_soon_anims
            .retain(|_, started| started.elapsed().as_millis() < ui::COMING_SOON_CYCLE_MS as u128);
    }
    pub(crate) fn preferred_sidebar_width(&self) -> u16 {
        self.preferred_pane_width.max(ui::MIN_PANE_WIDTH)
    }

    pub(crate) fn effective_sidebar_width(&self) -> u16 {
        // Prefer the cached client width so expand-from-rail does not block on
        // an extra tmux round-trip before the resize.
        let client = self.last_client_width.unwrap_or_else(|| {
            crate::daemon::tmux::current_pane_client_width().unwrap_or(
                self.preferred_sidebar_width()
                    .saturating_add(ui::WORKSPACE_MIN_WIDTH),
            )
        });
        ui::responsive_sidebar_width(
            self.preferred_sidebar_width(),
            client,
            self.sidebar_wants_rail(),
            self.sidebar_force_expanded,
        )
    }

    /// Auto or user collapse wants the rail (unless force-expanded peek is active).
    pub(crate) fn sidebar_wants_rail(&self) -> bool {
        self.sidebar_auto_collapsed || self.sidebar_user_collapsed
    }

    /// True when the bar is showing the narrow rail (not a temporary peek expand).
    pub(crate) fn is_sidebar_rail_collapsed(&self) -> bool {
        self.sidebar_wants_rail() && !self.sidebar_force_expanded
    }

    pub(crate) fn edge_resize_enabled(&self) -> bool {
        self.host.uses_edge_resize()
    }

    pub(crate) fn is_edge_resize_hit(&self, column: u16, metrics: &ui::LayoutMetrics) -> bool {
        if !self.edge_resize_enabled() || self.is_sidebar_rail_collapsed() {
            return false;
        }
        crate::bar::host_terminal::is_edge_resize_column(column, metrics.frame_width)
    }

    pub(crate) fn begin_edge_resize(&mut self, press_column: u16) {
        self.edge_resize_active = true;
        self.clear_close_mode();
        self.unfocus_notepad();
        self.clear_list_text_selection();
        self.clear_pointer_hover_states();
        // Cache client width once — live drag must not re-query tmux every pixel.
        if let Some(w) = crate::daemon::tmux::current_pane_client_width() {
            self.last_client_width = Some(w);
        }
        let current = self
            .user_pane_width
            .or(self.last_applied_sidebar_width)
            .or_else(crate::daemon::tmux::current_pane_width)
            .unwrap_or(ui::DEFAULT_PANE_WIDTH);
        // Keep the grabbed cell under the pointer (not a fixed +grip jump).
        self.edge_resize_grab_offset = current.saturating_sub(press_column);
        self.sidebar_expand_grace_until = Some(Instant::now() + super::SIDEBAR_EXPAND_GRACE);
    }

    pub(crate) fn finish_edge_resize(&mut self) {
        if !self.edge_resize_active {
            return;
        }
        self.edge_resize_active = false;
        self.edge_resize_grab_offset = 0;
        self.mark_sidebar_expand_grace();
        if let Some(cur) = crate::daemon::tmux::current_pane_width() {
            if cur > ui::DRAG_COLLAPSE_AT_OR_BELOW {
                self.preferred_pane_width = cur.max(ui::MIN_PANE_WIDTH);
                self.sidebar_user_collapsed = false;
                self.last_applied_sidebar_width = Some(cur);
                self.user_pane_width = Some(cur);
            }
        }
        // One full paint after the drag ends (not per-pixel during drag).
        self.force_redraw();
    }

    pub(crate) fn update_edge_resize(&mut self, mouse: &crossterm::event::MouseEvent) {
        if !self.edge_resize_active {
            return;
        }
        // desired = pointer + grab offset so the edge tracks the cursor 1:1.
        let desired = mouse
            .column
            .saturating_add(self.edge_resize_grab_offset)
            .max(1);
        if desired <= ui::DRAG_COLLAPSE_AT_OR_BELOW {
            self.ensure_usable_preferred_width();
            self.sidebar_user_collapsed = true;
            self.sidebar_force_expanded = false;
            self.edge_resize_active = false;
            self.edge_resize_grab_offset = 0;
            self.apply_sidebar_width(ui::COLLAPSED_PANE_WIDTH, ui::WORKSPACE_MIN_WIDTH);
            return;
        }
        self.sidebar_user_collapsed = false;
        self.sidebar_force_expanded = false;
        let target = desired.max(ui::MIN_PANE_WIDTH);
        if self.last_applied_sidebar_width == Some(target) {
            return;
        }
        // Cap tmux fork rate — xterm.js can emit dense Drag samples; each
        // `resize-pane` is a process spawn and was the main lag source.
        const EDGE_RESIZE_MIN_INTERVAL: Duration = Duration::from_millis(12);
        if self.last_edge_resize_apply.elapsed() < EDGE_RESIZE_MIN_INTERVAL {
            // Still remember the intended width so finish / next tick can catch up.
            self.preferred_pane_width = target;
            self.user_pane_width = Some(target);
            return;
        }
        self.preferred_pane_width = target;
        self.apply_sidebar_width_live(target);
    }

    /// Live edge-drag resize: one fast `resize-pane -x`, no force_redraw.
    /// Layout picks up the new terminal size from the Resize event.
    fn apply_sidebar_width_live(&mut self, target: u16) {
        let client = self
            .last_client_width
            .unwrap_or_else(|| crate::daemon::tmux::current_pane_client_width().unwrap_or(120));
        let _ = crate::daemon::tmux::resize_current_pane_width_fast(target, client);
        self.last_edge_resize_apply = Instant::now();
        self.last_applied_sidebar_width = Some(target);
        self.user_pane_width = Some(target);
        // Bookkeeping only — do **not** poke size to target-1 (that forced a full
        // paint every pixel and felt like the bar was thrashing under the pointer).
        if let Some((_, h)) = self.render_cache.size {
            self.render_cache.size = Some((target, h));
        }
    }

    fn apply_sidebar_width(&mut self, target: u16, workspace_min: u16) {
        // One fast resize: cache client width, clamp, single `resize-pane`.
        // The previous path spawned 3–5 tmux processes (before/after probes +
        // client_width + optional second format) and made collapse/expand feel
        // sticky — especially while spinner animation kept re-entering sync.
        //
        // Do not short-circuit on last_applied here: host-resize re-pin must
        // still call resize-pane when the pane has drifted. Idle rail re-pin is
        // skipped earlier via last_applied before this helper is called.
        let client = match self.last_client_width {
            Some(w) => w,
            None => {
                let w = crate::daemon::tmux::current_pane_client_width().unwrap_or(120);
                self.last_client_width = Some(w);
                w
            }
        };
        let width = crate::daemon::tmux::clamp_sidebar_width_with_workspace_min(
            target,
            client,
            workspace_min,
        );
        // Single `resize-pane` with the workspace_min-aware clamp already applied.
        // Avoid `resize_current_pane_width_fast` here — it re-clamps with the
        // default workspace floor and would undo peek expand's lower min.
        if let Ok(pane) = std::env::var("TMUX_PANE") {
            if !pane.is_empty() {
                let _ = crate::daemon::tmux::resize_pane_width_at_fast(&pane, width);
            }
        }
        let width_changed = self.last_applied_sidebar_width != Some(target);
        // Bookkeeping uses the **desired** width so host-resize drift never locks
        // in a temporary tmux intermediate as last_applied / preferred.
        self.last_applied_sidebar_width = Some(target);
        self.user_pane_width = Some(target);
        if width_changed {
            // Width-only: redraw content but do **not** clear scroll / layout size —
            // force_layout_redraw made the list jump vertically on every re-pin.
            // Do **not** optimistically set render_cache.size to the new width:
            // tmux pane width can lead the TTY size, and poisoning the cache made
            // layout_changed false while the last paint was still rail-mode
            // (empty expanded pane until click).
            self.force_redraw();
        }
    }

    fn mark_sidebar_expand_grace(&mut self) {
        self.sidebar_expand_grace_until = Some(Instant::now() + super::SIDEBAR_EXPAND_GRACE);
    }

    fn in_sidebar_expand_grace(&self) -> bool {
        self.sidebar_expand_grace_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Freeze list height while the pane width is changing so `list_top_y` /
    /// scroll (and rail status Y) do not jump when the TTY reports a 1-line
    /// height glitch during collapse/expand.
    fn freeze_layout_height_for_width_change(&mut self) {
        if self.sidebar_layout_height_freeze.is_none() {
            self.sidebar_layout_height_freeze = self.render_cache.size.map(|(_, h)| h);
        }
        self.sidebar_host_resize_until = Some(Instant::now() + super::SIDEBAR_HOST_RESIZE_GRACE);
    }

    fn mark_sidebar_host_resize_grace(&mut self) {
        // Capture height once when the host resize begins so list_top_y / list_height
        // stay stable if the emulator briefly reports a different pane height.
        self.freeze_layout_height_for_width_change();
    }

    pub(crate) fn in_sidebar_host_resize_grace(&self) -> bool {
        self.sidebar_host_resize_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Size used for vertical layout. During host-width resize, freeze height so
    /// session rows don't jump when the terminal reports a 1-line height glitch.
    fn layout_size_for_metrics(&mut self, size: ratatui::layout::Size) -> ratatui::layout::Size {
        if self.in_sidebar_host_resize_grace() {
            if let Some(frozen_h) = self.sidebar_layout_height_freeze {
                return ratatui::layout::Size {
                    width: size.width,
                    height: frozen_h,
                };
            }
            // First observation this grace window — lock whatever we have now.
            self.sidebar_layout_height_freeze = Some(size.height);
            return size;
        }
        // Grace over: accept live height again.
        self.sidebar_layout_height_freeze = None;
        size
    }

    fn ensure_usable_preferred_width(&mut self) {
        if self.preferred_pane_width <= ui::DRAG_COLLAPSE_AT_OR_BELOW
            || ui::is_collapsed_sidebar_width(self.preferred_pane_width)
        {
            self.preferred_pane_width = ui::DEFAULT_PANE_WIDTH;
        }
    }

    /// Leave rail mode and open to a usable list width (preferred / default).
    fn expand_sidebar_to_preferred(&mut self) {
        self.sidebar_user_collapsed = false;
        self.sidebar_force_expanded = self.sidebar_auto_collapsed;
        self.ensure_usable_preferred_width();
        let target = self.effective_sidebar_width();
        // Never "expand" to a rail-sized width — that leaves the UI stuck.
        let target = target
            .max(ui::MIN_PANE_WIDTH)
            .max(ui::MIN_EXPANDED_PANE_WIDTH.min(ui::DEFAULT_PANE_WIDTH));
        let workspace_min = ui::workspace_min_for_sidebar_target(self.sidebar_force_expanded);
        // Keep list_top_y stable while width opens so rail→list rows do not jump.
        self.freeze_layout_height_for_width_change();
        self.apply_sidebar_width(target, workspace_min);
        // apply_sidebar_width only force_redraws when the pane width changes. On
        // rapid collapse→expand the pane may still be full-width (collapse lag),
        // so width is a no-op — but we still switched rail→list paint mode and
        // must repaint or the bar stays blank until the next hover/click.
        self.force_redraw();
        self.mark_sidebar_expand_grace();
    }

    /// Recompute auto-collapse from client width and apply the target pane size.
    ///
    /// Host-terminal resize (left/right edge of the window): keep
    /// `preferred_pane_width` fixed and re-pin the pane every tick until the
    /// client is wide enough or we snap to the rail. **Never** adopt intermediate
    /// tmux sizes as preferred — that made the sidebar follow window compression.
    ///
    /// Divider drag (client width stable, outside host-resize grace): adopt the
    /// pane width as the new preferred.
    pub(crate) fn sync_responsive_sidebar_width(&mut self, force: bool) {
        if self.edge_resize_active {
            return;
        }
        let in_host_resize = self.in_sidebar_host_resize_grace();
        // Rate-limit idle polls only. Force (Resize events / toggles) and active
        // host-resize grace must re-pin every call — otherwise tmux shrinks the
        // sidebar between ticks and content jumps for up to 250ms. Deep idle
        // backs off further; host-driven resizes still re-pin every call.
        if !force
            && !in_host_resize
            && self.last_responsive_sync.elapsed()
                < self.effective_probe_interval(super::SIDEBAR_RESPONSIVE_SYNC_INTERVAL)
        {
            return;
        }
        self.last_responsive_sync = Instant::now();

        // One geometry probe for client + pane width (avoids dual tmux spawns).
        let Some((client_width, pane_width)) = crate::daemon::tmux::current_pane_geometry() else {
            return;
        };
        let client_changed = self.last_client_width != Some(client_width);
        self.last_client_width = Some(client_width);
        if client_changed {
            self.mark_sidebar_host_resize_grace();
        }
        let in_host_resize = client_changed || self.in_sidebar_host_resize_grace();

        let current = Some(pane_width);
        let preferred = self.preferred_sidebar_width();
        let next_auto =
            ui::sidebar_should_auto_collapse(client_width, preferred, self.sidebar_auto_collapsed);
        let auto_changed = next_auto != self.sidebar_auto_collapsed;
        self.sidebar_auto_collapsed = next_auto;

        if !next_auto && !self.sidebar_user_collapsed {
            // Wide enough and not user-hidden — drop temporary narrow peek.
            self.sidebar_force_expanded = false;
        }

        // Host-driven re-pin (not the same as `force`, which only bypasses rate limit
        // so divider-drag Resize events can still adopt the new preferred width).
        let host_driven = auto_changed || client_changed || in_host_resize;
        let in_grace = self.in_sidebar_expand_grace();

        // --- Divider / mid-drag handling while rail is logical state ---
        // Critical: never re-pin to 4 cols while the user is dragging open, or the
        // expand threshold can never be crossed (felt like "stuck collapsed").
        // During host resize, ignore temporary mid-widths and stay on the rail.
        if self.sidebar_wants_rail() && !self.sidebar_force_expanded {
            if !in_host_resize {
                if let Some(cur) = current {
                    if cur >= ui::DRAG_EXPAND_AT_OR_ABOVE {
                        // Clear open past the rail — finish expand to preferred width.
                        self.expand_sidebar_to_preferred();
                        return;
                    }
                    if cur > ui::COLLAPSED_PANE_WIDTH {
                        // Mid-drag between rail and expand threshold: leave the pane alone.
                        self.user_pane_width = Some(cur);
                        return;
                    }
                }
            }
            // Pin micro rail only when not already applied. Prefer last_applied
            // over live `current` so a lagging pane_width probe cannot re-fork
            // tmux on every animation frame (felt like sticky collapse).
            let target = ui::COLLAPSED_PANE_WIDTH;
            if !host_driven && !force && self.last_applied_sidebar_width == Some(target) {
                self.user_pane_width = Some(target);
                return;
            }
            if host_driven || force || current != Some(target) {
                self.apply_sidebar_width(target, ui::WORKSPACE_MIN_WIDTH);
            } else {
                self.user_pane_width = Some(target);
            }
            return;
        }

        // --- Drag-to-collapse while expanded (skip during post-expand / host-resize grace) ---
        // Host resize can briefly squash the pane ≤16 before we re-pin; that is not
        // a user divider collapse.
        if !host_driven && !in_grace && !in_host_resize {
            if let Some(cur) = current {
                if cur <= ui::DRAG_COLLAPSE_AT_OR_BELOW {
                    self.ensure_usable_preferred_width();
                    self.sidebar_user_collapsed = true;
                    self.sidebar_force_expanded = false;
                    self.apply_sidebar_width(ui::COLLAPSED_PANE_WIDTH, ui::WORKSPACE_MIN_WIDTH);
                    return;
                }
            }
        }

        // --- Peek expand while still narrow ---
        if self.sidebar_force_expanded {
            let target = ui::responsive_sidebar_width(preferred, client_width, true, true)
                .max(ui::MIN_PANE_WIDTH);
            if !host_driven && !force && self.last_applied_sidebar_width == Some(target) {
                self.user_pane_width = current.or(Some(target));
                return;
            }
            if host_driven || force || current != Some(target) {
                self.apply_sidebar_width(target, ui::PEEK_WORKSPACE_MIN);
            } else {
                self.user_pane_width = current.or(Some(target));
            }
            return;
        }

        // --- Fully expanded (fullscreen / wide client) ---
        // Divider drag only: client width stable, not settling from host resize.
        // Pane width changed without us applying it — adopt as preferred.
        if !host_driven {
            if let Some(cur) = current {
                if cur > ui::DRAG_COLLAPSE_AT_OR_BELOW
                    && self
                        .last_applied_sidebar_width
                        .is_some_and(|applied| applied != cur)
                {
                    self.preferred_pane_width = cur.max(ui::MIN_PANE_WIDTH);
                    self.user_pane_width = Some(cur);
                    self.last_applied_sidebar_width = Some(cur);
                    return;
                }
            }
        }

        self.ensure_usable_preferred_width();
        let target = ui::responsive_sidebar_width(
            self.preferred_sidebar_width(),
            client_width,
            false,
            false,
        )
        .max(ui::MIN_PANE_WIDTH);
        let drifted = current.is_some_and(|c| c != target);
        let missing = current.is_none();
        // Recover if state says expanded but pane is still rail-sized (stuck).
        let stuck_rail = current.is_some_and(ui::is_collapsed_sidebar_width);
        // Host resize / outer client change: always re-pin preferred so the list
        // width stays fixed while the workspace absorbs the window shrink/grow.
        if host_driven || drifted || missing || stuck_rail {
            self.apply_sidebar_width(target, ui::WORKSPACE_MIN_WIDTH);
            if stuck_rail {
                self.mark_sidebar_expand_grace();
            }
        } else if let Some(cur) = current {
            self.user_pane_width = Some(cur);
            if self.last_applied_sidebar_width.is_none() {
                self.last_applied_sidebar_width = Some(target);
            }
        }
    }

    /// Expand the rail (user hide or auto-collapse) to a usable list.
    pub(crate) fn expand_sidebar_from_rail(&mut self) {
        // Prefer bookkeeping over a tmux probe so click/▸ responds in one frame.
        let width_is_rail = self
            .last_applied_sidebar_width
            .is_some_and(ui::is_collapsed_sidebar_width)
            || self
                .user_pane_width
                .is_some_and(ui::is_collapsed_sidebar_width)
            || crate::daemon::tmux::current_pane_width()
                .is_some_and(ui::is_collapsed_sidebar_width);
        // Always allow recovery — even if state thinks we're expanded but width is rail.
        if !self.is_sidebar_rail_collapsed() && !width_is_rail {
            return;
        }
        self.expand_sidebar_to_preferred();
    }

    /// Collapse again after a temporary peek (workspace focused, Esc, or `b`).
    pub(crate) fn collapse_sidebar_rail_if_narrow(&mut self) {
        if !self.sidebar_force_expanded {
            return;
        }
        if self.in_sidebar_expand_grace() {
            return;
        }
        self.sidebar_force_expanded = false;
        self.sync_responsive_sidebar_width(true);
    }

    /// Toggle rail vs expanded. `b` — works in fullscreen and narrow modes.
    pub(crate) fn toggle_sidebar_rail(&mut self) {
        let width_is_rail = self
            .last_applied_sidebar_width
            .is_some_and(ui::is_collapsed_sidebar_width)
            || self
                .user_pane_width
                .is_some_and(ui::is_collapsed_sidebar_width)
            || crate::daemon::tmux::current_pane_width()
                .is_some_and(ui::is_collapsed_sidebar_width);
        if self.is_sidebar_rail_collapsed() || width_is_rail {
            self.expand_sidebar_from_rail();
            return;
        }
        // Currently expanded: hide to rail.
        self.sidebar_force_expanded = false;
        self.sidebar_user_collapsed = true;
        self.sidebar_expand_grace_until = None;
        self.ensure_usable_preferred_width();
        // Freeze height before resize so rail chips keep the same Y as expanded
        // session rows (TTY height flicker used to shift list_top_y by one).
        self.freeze_layout_height_for_width_change();
        self.apply_sidebar_width(ui::COLLAPSED_PANE_WIDTH, ui::WORKSPACE_MIN_WIDTH);
        // Same as expand: width may already be rail-sized, but list→rail still
        // needs a paint (see expand_sidebar_to_preferred).
        self.force_redraw();
    }

    /// Grow or shrink the sidebar by keyboard (preferred over IDE edge-drag).
    ///
    /// Positive `delta` widens; negative narrows. Shrinking at/below the drag
    /// collapse threshold snaps to the micro rail (same as divider drag).
    /// Growing from the rail restores the preferred expanded width first.
    pub(crate) fn resize_sidebar_by(&mut self, delta: i16) {
        if delta == 0 {
            return;
        }
        self.clear_close_mode();
        self.edge_resize_active = false;

        let width_is_rail =
            crate::daemon::tmux::current_pane_width().is_some_and(ui::is_collapsed_sidebar_width);
        if self.is_sidebar_rail_collapsed() || width_is_rail {
            if delta < 0 {
                return;
            }
            // Grow from rail → open to preferred list width (same as `b` / click).
            self.expand_sidebar_from_rail();
            return;
        }

        let current = self
            .user_pane_width
            .or(self.last_applied_sidebar_width)
            .or_else(crate::daemon::tmux::current_pane_width)
            .unwrap_or(self.preferred_sidebar_width());

        let desired = if delta > 0 {
            current.saturating_add(delta as u16)
        } else {
            current.saturating_sub((-delta) as u16)
        };

        if desired <= ui::DRAG_COLLAPSE_AT_OR_BELOW {
            self.ensure_usable_preferred_width();
            self.sidebar_user_collapsed = true;
            self.sidebar_force_expanded = false;
            self.apply_sidebar_width(ui::COLLAPSED_PANE_WIDTH, ui::WORKSPACE_MIN_WIDTH);
            return;
        }

        self.sidebar_user_collapsed = false;
        self.sidebar_force_expanded = self.sidebar_auto_collapsed;
        let target = desired.max(ui::MIN_PANE_WIDTH);
        self.preferred_pane_width = target;
        let workspace_min = ui::workspace_min_for_sidebar_target(self.sidebar_force_expanded);
        self.apply_sidebar_width(target, workspace_min);
        self.mark_sidebar_expand_grace();
    }

    pub(crate) fn terminal_size(
        terminal: &Terminal<CrosstermBackend<io::Stdout>>,
    ) -> ratatui::layout::Size {
        terminal
            .size()
            .unwrap_or(ratatui::layout::Size::new(80, 24))
    }
    pub(crate) fn sidebar_snapshot<'a>(
        &'a self,
        coming_soon_frames: &'a [(ui::ToolbarAction, usize)],
        clipboard_notice: Option<&'a str>,
    ) -> ui::SidebarSnapshot<'a> {
        let drag_active = self.group_drag.active() || self.note_drag.active();
        // When a workspace panel owns the right pane, the selected backdrop moves to
        // that toolbar/settings row — keep real selection in App so Esc restores it.
        let visual_selected = if self.workspace_panel_open() {
            usize::MAX
        } else {
            self.effective_selected_row()
        };
        ui::SidebarSnapshot {
            sessions: ui::SessionsView {
                rows: &self.rows,
                selected: visual_selected,
                scroll: self.scroll,
                digit_buffer: &self.digit_buffer,
                close_modifier_held: self.close_modifier_held,
                hover_row: if drag_active { None } else { self.hover_row },
                close_target: self.close_target_row,
                group_hover_row: if drag_active {
                    None
                } else {
                    self.group_hover_row
                },
                sessions_expanded: self.sessions_expanded,
                folded_groups: &self.folded_groups,
                group_order: &self.group_order,
                group_drag: &self.group_drag,
                sessions_title_hover: self.sessions_title_hover,
                sessions_title_add_hover: self.sessions_title_add_hover,
                anim_frame: self.anim_frame,
                group_launch: &self.group_launch,
            },
            notepad: ui::NotepadView {
                notes: self.display_notes(),
                expanded: self.notepad_expanded,
                notes_list_expanded: self.notes_list_expanded,
                active_note_index: self.active_note_index(),
                text: self.active_note_text(),
                cursor: self.notepad_editor.cursor,
                scroll: self.notepad_editor.scroll,
                focused: self.notepad_focused,
                section_header_hover: self.notepad_section_header_hover,
                section_add_hover: self.notepad_section_add_hover,
                note_hover: self.effective_note_hover(),
                note_drag: &self.note_drag,
                selection: self.notepad_editor.selection,
                checkbox_literal_edit: self.notepad_editor.checkbox_literal_edit,
                suppress_terminal_cursor: self.notepad_editor.suppress_terminal_cursor,
                last_saved_at: self.notepad_last_saved_at,
            },
            chrome: ui::ChromeView {
                toolbar_hover: self.toolbar_hover,
                coming_soon_frames,
                settings_hover: self.settings_hover,
                leave_hover: self.leave_hover,
                workspace_settings_open: self.workspace_settings_open,
                workspace_new_session_open: self.workspace_new_session_open,
                workspace_automations_open: self.workspace_automations_open,
                workspace_mcps_open: self.workspace_mcps_open,
                workspace_skills_open: self.workspace_skills_open,
                collapse_control_hover: self.collapse_control_hover,
            },
            overlay: ui::OverlayView {
                context_menu: self.context_menu.as_ref(),
                rename: self.rename.as_ref(),
                delete_note_confirm: self.delete_note_confirm.as_ref(),
                clipboard_notice,
                update_banner: self.update_banner.as_ref(),
                update_upgrade_hover: self.update_upgrade_hover,
                update_dismiss_hover: self.update_dismiss_hover,
            },
        }
    }
    pub(crate) fn redraw_if_needed(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let raw_size = Self::terminal_size(terminal);
        let size = self.layout_size_for_metrics(raw_size);
        let metrics = self.layout_metrics(size);
        let display_notes = if self.note_drag.active() && !self.notes_preview.is_empty() {
            self.notes_preview.as_slice()
        } else {
            self.notes.as_slice()
        };
        let active_note_index = self
            .active_note_id
            .as_ref()
            .and_then(|id| display_notes.iter().position(|note| &note.id == id));
        let notepad_state = ui::notepad_list_state(
            display_notes,
            self.notepad_expanded,
            self.notes_list_expanded,
            active_note_index,
        );
        let total_rows =
            ui::total_list_rows(self.rows.len(), self.sessions_expanded, &notepad_state);
        self.scroll = ui::clamp_list_scroll(self.scroll, total_rows, metrics.list_height);
        if self.notepad_scroll_pending {
            self.scroll = ui::clamp_list_scroll(
                ui::ensure_active_note_visible(
                    self.scroll,
                    metrics.list_height,
                    ui::sidebar_trail_base_row(self.rows.len(), self.sessions_expanded),
                    &notepad_state,
                ),
                total_rows,
                metrics.list_height,
            );
            self.notepad_scroll_pending = false;
        }
        if self.selection_scroll_sync {
            self.scroll =
                ui::ensure_selection_visible(self.selected, self.scroll, metrics.list_height);
            self.selection_scroll_sync = false;
        }

        if self.last_time_tick.elapsed() >= Duration::from_secs(30) {
            self.last_time_tick = Instant::now();
            self.rows_version = self.rows_version.wrapping_add(1);
        }

        // Auto-collapse / rail only — never snap expanded fullscreen widths here
        // (divider drags update preferred width inside the sync helper).
        self.sync_responsive_sidebar_width(false);

        // Paint the micro rail whenever state wants it *or* the pane is already
        // rail-width — never squash the full list (with selection highlights)
        // into a 4-col strip if flags and tmux width briefly disagree.
        let rail = self.is_sidebar_rail_collapsed() || ui::is_collapsed_sidebar_width(size.width);
        let rail_mode_changed = self.render_cache.sidebar_rail != Some(rail);

        let layout_changed = self.render_cache.size != Some((size.width, size.height))
            || self.render_cache.scroll != Some(self.scroll);
        let close_visual_changed = self.close_modifier_held != self.render_cache.close_mode;
        let hover_visual_changed = self.hover_row != self.render_cache.hover_row
            || self.close_target_row != self.render_cache.close_target_row
            || self.group_hover_row != self.render_cache.group_hover_row
            || self.toolbar_hover != self.render_cache.toolbar_hover
            || self.settings_hover != self.render_cache.settings_hover
            || self.leave_hover != self.render_cache.leave_hover
            || self.workspace_settings_open != self.render_cache.workspace_settings_open
            || self.workspace_new_session_open != self.render_cache.workspace_new_session_open
            || self.workspace_automations_open != self.render_cache.workspace_automations_open
            || self.workspace_mcps_open != self.render_cache.workspace_mcps_open
            || self.workspace_skills_open != self.render_cache.workspace_skills_open
            || self.notepad_section_header_hover != self.render_cache.notepad_section_header_hover
            || self.notepad_section_add_hover != self.render_cache.notepad_section_add_hover
            || self.notepad_note_hover != self.render_cache.notepad_note_hover
            || self.sessions_title_hover != self.render_cache.sessions_title_hover
            || self.sessions_title_add_hover != self.render_cache.sessions_title_add_hover
            || self.collapse_control_hover != self.render_cache.collapse_control_hover
            || self.update_upgrade_hover != self.render_cache.update_upgrade_hover
            || self.update_dismiss_hover != self.render_cache.update_dismiss_hover;
        let notepad_visual_changed = self.sessions_expanded != self.render_cache.sessions_expanded
            || self.notepad_expanded != self.render_cache.notepad_expanded
            || self.notes_list_expanded != self.render_cache.notes_list_expanded
            || self.notepad_focused != self.render_cache.notepad_focused
            || self.notes != self.render_cache.notes
            || self.active_note_id != self.render_cache.active_note_id
            || self.notepad_editor.cursor != self.render_cache.notepad_cursor
            || self.notepad_editor.scroll != self.render_cache.notepad_scroll
            || self.notepad_editor.selection != self.render_cache.notepad_selection
            || self.notepad_save_badge_label() != self.render_cache.notepad_save_badge;
        let anim_visual_changed = self.needs_continuous_animation()
            && self.render_cache.anim_frame != Some(self.anim_frame);
        let coming_soon_frames = self.active_coming_soon_frames();
        let coming_soon_changed = coming_soon_frames != self.render_cache.coming_soon_frames;
        let clipboard_notice = self.active_status_notice();
        let clipboard_notice_changed = clipboard_notice != self.render_cache.clipboard_notice;
        let update_banner_label = self.update_banner.as_ref().map(|b| b.label.clone());
        let update_banner_changed = update_banner_label != self.render_cache.update_banner;
        let hover_only_redraw = hover_visual_changed
            && !close_visual_changed
            && !anim_visual_changed
            && !notepad_visual_changed
            && !rail_mode_changed
            && !self.group_drag.active()
            && !self.note_drag.active()
            && !layout_changed
            && self.render_cache.rows_version == self.rows_version;
        let data_changed = !hover_only_redraw
            && !anim_visual_changed
            && self.rows_version != self.render_cache.rows_version;
        if close_visual_changed
            || hover_visual_changed
            || notepad_visual_changed
            || anim_visual_changed
            || coming_soon_changed
            || clipboard_notice_changed
            || update_banner_changed
            || rail_mode_changed
            || self.group_drag.active()
            || self.note_drag.active()
            || data_changed
            || layout_changed
        {
            // Always allow the first render so the sidebar isn't blank on startup.
            let snap = self.sidebar_snapshot(&coming_soon_frames, clipboard_notice.as_deref());
            // Metrics use full frame height so list_top_y matches expanded layout.
            let rail_metrics = metrics;
            let rail_items = if rail {
                ui::rail_status_items(&self.rows)
            } else {
                Vec::new()
            };
            let anim_frame = self.anim_frame;
            let scroll = self.scroll;
            terminal.draw(|f| {
                if rail {
                    ui::draw_collapsed_rail(
                        f,
                        &rail_items,
                        scroll,
                        rail_metrics.list_top_y,
                        rail_metrics.list_height,
                        anim_frame,
                    );
                } else {
                    ui::draw(f, &snap);
                }
            })?;
            if !hover_only_redraw {
                self.render_cache.rows_version = self.rows_version;
            }
            self.render_cache.size = Some((size.width, size.height));
            self.render_cache.scroll = Some(self.scroll);
            self.render_cache.close_mode = self.close_modifier_held;
            self.render_cache.hover_row = self.hover_row;
            self.render_cache.close_target_row = self.close_target_row;
            self.render_cache.group_hover_row = self.group_hover_row;
            self.render_cache.toolbar_hover = self.toolbar_hover;
            self.render_cache.coming_soon_frames = coming_soon_frames;
            self.render_cache.settings_hover = self.settings_hover;
            self.render_cache.leave_hover = self.leave_hover;
            self.render_cache.workspace_settings_open = self.workspace_settings_open;
            self.render_cache.workspace_new_session_open = self.workspace_new_session_open;
            self.render_cache.workspace_automations_open = self.workspace_automations_open;
            self.render_cache.workspace_mcps_open = self.workspace_mcps_open;
            self.render_cache.workspace_skills_open = self.workspace_skills_open;
            self.render_cache.sessions_expanded = self.sessions_expanded;
            self.render_cache.notepad_expanded = self.notepad_expanded;
            self.render_cache.notes_list_expanded = self.notes_list_expanded;
            self.render_cache.notepad_focused = self.notepad_focused;
            self.render_cache.notes = self.notes.clone();
            self.render_cache.active_note_id = self.active_note_id.clone();
            self.render_cache.notepad_cursor = self.notepad_editor.cursor;
            self.render_cache.notepad_scroll = self.notepad_editor.scroll;
            self.render_cache.notepad_section_header_hover = self.notepad_section_header_hover;
            self.render_cache.notepad_section_add_hover = self.notepad_section_add_hover;
            self.render_cache.notepad_note_hover = self.notepad_note_hover;
            self.render_cache.notepad_selection = self.notepad_editor.selection;
            self.render_cache.notepad_save_badge = self.notepad_save_badge_label();
            self.render_cache.sessions_title_hover = self.sessions_title_hover;
            self.render_cache.sessions_title_add_hover = self.sessions_title_add_hover;
            self.render_cache.collapse_control_hover = self.collapse_control_hover;
            self.render_cache.sidebar_rail = Some(rail);
            self.render_cache.anim_frame = Some(self.anim_frame);
            self.render_cache.clipboard_notice = clipboard_notice.clone();
            self.render_cache.update_banner = update_banner_label;
            self.render_cache.update_upgrade_hover = self.update_upgrade_hover;
            self.render_cache.update_dismiss_hover = self.update_dismiss_hover;
            self.sync_sidebar_mouse_cursor(Some(&metrics));
        }
        Ok(())
    }
    pub(crate) fn active_status_notice(&self) -> Option<String> {
        let text = self.clipboard_notice_text.as_ref()?;
        match self.clipboard_notice_until {
            // Sticky notice (e.g. reconnecting while sessions stay on screen).
            None => Some(text.clone()),
            Some(until) if Instant::now() < until => Some(text.clone()),
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// Regression: rapid collapse→expand can leave the pane already at preferred
    /// width so `apply_sidebar_width` is a no-op. Expand must still dirty the
    /// frame (rail→list), otherwise the bar stays blank until hover/click.
    #[test]
    fn expand_from_rail_dirties_redraw_when_width_already_open() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.sidebar_user_collapsed = true;
        app.sidebar_force_expanded = false;
        app.sidebar_auto_collapsed = false;
        app.preferred_pane_width = ui::DEFAULT_PANE_WIDTH;
        // Simulate a full-width pane that never finished collapsing (or already
        // re-opened) while logical state is still rail-collapsed.
        app.last_applied_sidebar_width = Some(ui::DEFAULT_PANE_WIDTH);
        app.user_pane_width = Some(ui::DEFAULT_PANE_WIDTH);
        app.render_cache.size = Some((ui::DEFAULT_PANE_WIDTH, 24));
        app.render_cache.sidebar_rail = Some(true);
        app.render_cache.rows_version = app.rows_version;

        let version_before = app.rows_version;
        assert!(app.is_sidebar_rail_collapsed());

        app.expand_sidebar_from_rail();

        assert!(!app.is_sidebar_rail_collapsed());
        assert_ne!(
            app.rows_version, version_before,
            "expand must force_redraw even when pane width is already open"
        );
    }

    #[test]
    fn toggle_to_rail_dirties_redraw() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.sidebar_user_collapsed = false;
        app.sidebar_force_expanded = false;
        app.sidebar_auto_collapsed = false;
        app.preferred_pane_width = ui::DEFAULT_PANE_WIDTH;
        app.last_applied_sidebar_width = Some(ui::DEFAULT_PANE_WIDTH);
        app.user_pane_width = Some(ui::DEFAULT_PANE_WIDTH);
        app.render_cache.size = Some((ui::DEFAULT_PANE_WIDTH, 24));
        app.render_cache.sidebar_rail = Some(false);
        app.render_cache.rows_version = app.rows_version;

        let version_before = app.rows_version;
        app.toggle_sidebar_rail();

        assert!(app.is_sidebar_rail_collapsed());
        assert_ne!(
            app.rows_version, version_before,
            "collapse toggle must force_redraw even when width apply is a no-op"
        );
    }
}
