//! A view owns cursor/scroll state and points at a shared buffer. Multiple views
//! may reference one buffer with fully independent viewport state.

use crate::app::layout::Viewport;
use crate::session::BufferId;
use crate::term::renderer::Rect;

pub type ViewId = u64;

#[derive(Clone, Debug)]
pub struct View {
    pub id: ViewId,
    pub buffer_id: BufferId,
    pub viewport: Viewport,
    pub desired_col: usize,
    pub rect: Rect,
    pub focused: bool,
}

impl View {
    pub fn new(id: ViewId, buffer_id: BufferId) -> Self {
        Self {
            id,
            buffer_id,
            viewport: Viewport::default(),
            desired_col: 0,
            rect: Rect::default(),
            focused: true,
        }
    }
}
