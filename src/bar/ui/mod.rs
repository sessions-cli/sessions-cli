mod snapshot;
mod theme;

pub use snapshot::{
    ChromeView, NotepadView, OverlayView, SessionsView, SidebarSnapshot,
};
pub use theme::*;

mod widgets;
mod notepad;
mod layout;
mod sessions;
mod chrome;
mod draw;

pub use widgets::*;
pub use notepad::*;
pub use layout::*;
pub use sessions::*;
pub use chrome::*;
pub use draw::*;

pub(crate) use chrome::{
    chrome_button_style, chrome_row_backdrop_bg, coming_soon_label_spans,
    coming_soon_label_text,
};
pub(crate) use draw::list_row_backdrop_bg;
pub(crate) use sessions::dim_style;

#[cfg(test)]
mod tests;