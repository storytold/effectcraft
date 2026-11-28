//! EffectCraft side of Craft Link.
//!
//! `open_link` handles `link.open`: import an OpenTimelineIO payload, open the comp, and write
//! the sidecar project. `attach_link` handles `link.attach` for a link that is already bound or
//! whose sidecar project is on disk. `back_to_filmcraft` handles `link.back`: render the bound
//! comp, send `link.focus`, then `app.activate` FilmCraft. `render_result` handles
//! `link.renderResult`: encode the bound comp to the binding's `result_path` as ProRes 4444.
//! `activation` holds the egui focus backend and the incoming `app.activate` handler.
//!
//! `sidecar` keeps `<link_id>.ecproj` current: the live link writes it 2 s after the last change, on
//! close and after Back, so `link.attach` reopens the latest edits. `live::begin_close` renders a
//! missing or stale result before the window closes.
//!
//! The live link: `frames` serves `frames.open` and bulk requests with `Renderer::comp_frame`,
//! `track` counts epochs and decides when the result file is stale, and `live` ties both to the UI
//! thread (`live::install` once, `live::tick` per frame).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod activation;
pub mod attach;
pub mod back;
pub mod close;
pub mod command;
pub mod frames;
pub mod live;
pub mod peer;
pub mod receive;
pub mod result;
pub mod sidecar;
#[cfg(test)]
mod test_support;
pub mod track;

pub use activation::{ActivateRequest, ActivationBackend, EguiActivation, SinkActivate, handle_activate};
pub use attach::{AttachError, attach_link};
pub use back::{BackError, back_enabled, back_to_filmcraft};
pub use close::close_link;
pub use command::{BACK_COMMAND, back_installed, install_back_command, sync_back_enabled};
pub use frames::{BoundLink, CompFrames, FrameWorld};
pub use live::{Closing, Emitter, Live, Waker};
pub use peer::{FilmCraftPeer, PeerError};
pub use receive::{LinkOpenError, OpenedLink, open_link};
pub use result::{RESULT_CAUSE, RenderError, RenderOutcome, RenderPlan, prepare_render, render_result, spawn_render, unusable_footage};
pub use track::Tracker;
