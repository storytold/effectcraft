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
    /// Refused because parts would be lost in a conversion; nothing changed. `losses` names them,
    /// `advice` how to go on (`file.importAep` / `file.exportAep` without `allowLosses`).
    #[error("{losses} ({advice})")]
    Refused { losses: String, advice: &'static str },
}

pub type Result<T> = std::result::Result<T, EngineError>;
