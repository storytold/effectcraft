use super::{ItemId, Serialize, Value};

/// Events for frontends (drained each frame).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Event {
    ProjectChanged {
        revision: u64,
    },
    Toast {
        message: String,
        error: bool,
    },
    OpenComp(ItemId),
    OpenUrl(String),
    /// A frontend-only command (viewer zoom, panels, dialogs…) ran: the UI performs it. Headless
    /// sessions ignore these.
    Frontend {
        command: String,
        params: Value,
    },
    /// Drop cached frames / renders (Edit ▸ Purge).
    PurgeCaches,
    /// Put an image on the system clipboard (Composition ▸ Copy Frame to Clipboard): `rgba` is
    /// `width` × `height` straight-alpha RGBA8 rows. Headless sessions ignore it; the pixels are
    /// not serialized.
    CopyImage {
        width: u32,
        height: u32,
        #[serde(skip)]
        rgba: Vec<u8>,
    },
}
