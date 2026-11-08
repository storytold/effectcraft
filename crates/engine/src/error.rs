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
    /// Refused because parts of the data would be lost; nothing changed. `file.importAep` and
    /// `file.exportAep` without `allowLosses`: `losses` lists them, `advice` tells a caller how to
    /// go on (#3103: File ▸ Import ▸ File… shows `losses` with its own pointer instead).
    #[error("{losses} ({advice})")]
    Refused { losses: String, advice: &'static str },
}

pub type Result<T> = std::result::Result<T, EngineError>;
