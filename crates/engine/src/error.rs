#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("no active composition")]
    NoComp,
    /// A `comp` reference (name or id) that names no composition.
    #[error("no composition {0}")]
    NoSuchComp(String),
    #[error("{0}")]
    Project(#[from] effectcraft_project::ProjectError),
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Command(#[from] craft_command::CommandError),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<craft_plugin_host::invoke::CallError> for EngineError {
    fn from(e: craft_plugin_host::invoke::CallError) -> Self {
        EngineError::Other(format!("plug-in: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;
