use super::super::App;
use crate::bar::ui;
use crate::model::ServerEvent;

impl App {

    pub(crate) fn close_selected_session(&mut self) {
        self.close_row(self.selected);
    }
    pub(crate) fn close_row(&mut self, row_idx: usize) {
        if ui::group_toggle_at(&self.rows, row_idx).is_some() {
            return;
        }
        let session_id = self.session_at(row_idx).map(|session| session.id.clone());
        let Some(session_id) = session_id else {
            return;
        };
        self.close_session_by_id(&session_id);
    }
    pub(crate) fn close_session_by_id(&mut self, session_id: &str) {
        if let Ok(Some(ServerEvent::Snapshot { sessions, version })) =
            self.client.close_session(session_id)
        {
            self.sessions = sessions;
            self.version = version;
            self.disengage_close_mode();
            self.rebuild_rows();
        }
    }
    pub(crate) fn close_group(&mut self, cwd_label: &str) {
        let session_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| session.cwd_label == cwd_label)
            .map(|session| session.id.clone())
            .collect();
        for session_id in session_ids {
            if let Ok(Some(ServerEvent::Snapshot { sessions, version })) =
                self.client.close_session(&session_id)
            {
                self.sessions = sessions;
                self.version = version;
            } else {
                break;
            }
        }
        self.disengage_close_mode();
        self.rebuild_rows();
    }
}
