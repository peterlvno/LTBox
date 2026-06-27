//! Window-chrome handlers: titlebar buttons, cursor-drag resize, and
//! debounced geometry persistence. Extracted from `main.rs`.
use crate::*;
use iced::Task;

impl App {
    /// Dispatch a `WindowMsg` — titlebar drag / minimize / maximize /
    /// close / cursor-drag resize. All variants except
    /// `WindowIdReceived` are gated on `self.window_id` being set;
    /// before iced delivers the window id those calls would be
    /// no-ops anyway.
    pub(crate) fn update_window(&mut self, msg: WindowMsg) -> Task<Message> {
        match msg {
            WindowMsg::WindowIdReceived(id) => {
                self.window_id = id;
                self.window_id
                    .map(|id| iced::window::is_maximized(id).map(Message::WindowMaximized))
                    .unwrap_or_else(Task::none)
            }
            WindowMsg::WindowDrag => self
                .window_id
                .map(iced::window::drag)
                .unwrap_or_else(Task::none),
            WindowMsg::WindowMinimize => self
                .window_id
                .map(|id| iced::window::minimize(id, true))
                .unwrap_or_else(Task::none),
            WindowMsg::WindowToggleMaximize => self
                .window_id
                .map(|id| {
                    let maximized = !self.window_maximized;
                    self.window_maximized = maximized;
                    iced::window::maximize(id, maximized)
                })
                .unwrap_or_else(Task::none),
            WindowMsg::WindowClose => self
                .window_id
                .map(iced::window::close)
                .unwrap_or_else(Task::none),
            WindowMsg::WindowResize(direction) => self
                .window_id
                .map(|id| iced::window::drag_resize(id, direction))
                .unwrap_or_else(Task::none),
        }
    }

    /// Cursor-drag resize / maximize / restore funnel through here.
    /// Snap the persisted size to the `MIN_WINDOW_*` floor so a
    /// maximize → store → relaunch sequence still launches at a usable
    /// geometry rather than below the layout floor.
    pub(crate) fn update_window_resized(&mut self, w: f32, h: f32) -> Task<Message> {
        let w = w.max(MIN_WINDOW_WIDTH);
        let h = h.max(MIN_WINDOW_HEIGHT);
        if (w, h) != self.window_size {
            self.window_size = (w, h);
            self.window_size_dirty = true;
        }
        self.window_id
            .map(|id| iced::window::is_maximized(id).map(Message::WindowMaximized))
            .unwrap_or_else(Task::none)
    }

    /// Debounced persistence tick — only flushes when the resize
    /// stream has been quiet for `WINDOW_SIZE_SAVE_INTERVAL`.
    pub(crate) fn update_persist_window_size(&mut self) -> Task<Message> {
        if self.window_size_dirty
            && self.window_size_last_save.elapsed() >= WINDOW_SIZE_SAVE_INTERVAL
        {
            self.persist_settings();
            self.window_size_dirty = false;
            self.window_size_last_save = std::time::Instant::now();
        }
        Task::none()
    }
}
