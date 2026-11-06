//! The FilmCraft instance that owns a link. Found through Craft Link discovery.
//!
//! `link.back` sends `link.focus` and then `app.activate` over one connection. This module finds
//! the instance, says hello, and makes those two calls. It never reports success it did not get
//! from the peer.

use craft_ipc::client::{Client, HelloClient};
use craft_ipc::server::Activation;
use craft_ipc::wire::{PROTOCOL_MAX, PROTOCOL_MIN};
use craft_ipc::{Discovery, InstanceRecord, LinkError, RuntimeDir, random_uuid};
use serde_json::{Value, json};

/// App id FilmCraft publishes in its discovery record.
const FILMCRAFT: &str = "filmcraft";

/// A connection to the FilmCraft instance that accepted `link.focus`.
pub struct FilmCraftPeer {
    client: Client,
    pid: u32,
}

/// Why no FilmCraft took the focus request.
#[derive(Debug)]
pub enum PeerError {
    /// No FilmCraft record in discovery, or none answered. The user has to start FilmCraft.
    NotRunning(String),
    /// A FilmCraft answered and refused the request.
    Rejected(LinkError),
}

impl PeerError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotRunning(_) => "not_ready",
            // The client maps an error object with an unknown code to `Invalid`, so that is not a bad request of ours.
            Self::Rejected(LinkError::Invalid(_)) => "internal",
            Self::Rejected(err) => err.code(),
        }
    }
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning(why) => write!(f, "{why}"),
            Self::Rejected(err) => write!(f, "FilmCraft refused: {err}"),
        }
    }
}

impl std::error::Error for PeerError {}

impl FilmCraftPeer {
    /// Send `link.focus` with `params` to the FilmCraft instance named by `source`.
    ///
    /// `source` is the `link.open` source object. Its `doc_id` picks the instance. An instance
    /// that lists documents and does not list `doc_id` is skipped. An instance that lists none
    /// is tried after the matching ones, because the record can lag behind an open document.
    /// The first instance that accepts `link.focus` is returned.
    ///
    /// `runtime` `None` scans [`RuntimeDir::resolve`].
    pub fn focus(runtime: Option<&RuntimeDir>, source: &Value, params: &Value) -> Result<Self, PeerError> {
        let dir = match runtime {
            Some(dir) => dir.clone(),
            None => RuntimeDir::resolve().map_err(|err| PeerError::NotRunning(format!("FilmCraft cannot be found: {err}")))?,
        };
        let records = Discovery::scan(&dir).map_err(|err| PeerError::NotRunning(format!("FilmCraft cannot be found: {err}")))?;
        let candidates = candidates(records, source.get("doc_id").and_then(Value::as_str));
        if candidates.is_empty() {
            return Err(PeerError::NotRunning("FilmCraft is not running. Start it and open the project that holds this link.".into()));
        }
        let mut last_connect: Option<LinkError> = None;
        let mut last_reject: Option<LinkError> = None;
        for record in &candidates {
            let mut client = match connect(record) {
                Ok(client) => client,
                Err(err) => {
                    last_connect = Some(err);
                    continue;
                }
            };
            match client.call("link.focus", params.clone()) {
                Ok(_) => return Ok(Self { client, pid: record.pid }),
                Err(err @ (LinkError::Io(_) | LinkError::Closed | LinkError::Timeout)) => last_connect = Some(err),
                Err(err) => last_reject = Some(err),
            }
        }
        match (last_reject, last_connect) {
            (Some(err), _) => Err(PeerError::Rejected(err)),
            (None, Some(err)) => Err(PeerError::NotRunning(format!("FilmCraft did not answer: {err}"))),
            (None, None) => Err(PeerError::NotRunning("FilmCraft is not running.".into())),
        }
    }

    /// Pid of the instance that took the focus request.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// `app.activate` on the instance. `token` is an activation token, or `None`.
    pub fn activate(&mut self, token: Option<&str>, open: Value) -> Result<Activation, LinkError> {
        let params = json!({"token": token, "from_pid": std::process::id(), "open": open});
        let result = self.client.call("app.activate", params)?;
        match result.get("activated") {
            Some(Value::Bool(true)) => Ok(Activation::Yes),
            Some(Value::Bool(false)) => Ok(Activation::No),
            Some(Value::String(text)) if text == "requested" => Ok(Activation::Requested),
            other => Err(LinkError::Invalid(format!("app.activate result has no valid `activated`: {other:?}"))),
        }
    }
}

fn connect(record: &InstanceRecord) -> Result<Client, LinkError> {
    let mut client = Client::connect(record)?;
    client.hello(&HelloClient {
        app: "effectcraft".to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        instance: random_uuid()?,
        protocol_min: PROTOCOL_MIN,
        protocol_max: PROTOCOL_MAX,
    })?;
    Ok(client)
}

/// FilmCraft records in the order to try them. Newest first inside each group.
fn candidates(records: Vec<InstanceRecord>, doc_id: Option<&str>) -> Vec<InstanceRecord> {
    let mut matching = Vec::new();
    let mut unknown = Vec::new();
    for record in records.into_iter().filter(|record| record.app == FILMCRAFT) {
        if record.docs.is_empty() {
            unknown.push(record);
        } else if doc_id.is_some_and(|id| record.docs.iter().any(|doc| doc.doc_id == id)) {
            matching.push(record);
        }
    }
    matching.sort_by_key(|record| std::cmp::Reverse(record.started));
    unknown.sort_by_key(|record| std::cmp::Reverse(record.started));
    matching.extend(unknown);
    matching
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use craft_ipc::{DocInfo, Endpoint};

    use super::*;

    fn record(app: &str, pid: u32, started: u64, docs: &[&str]) -> InstanceRecord {
        let mut record = InstanceRecord::new(app, "1.0.0", pid, Endpoint::Tcp { port: 40000 }).unwrap();
        record.started = started;
        record.docs = docs.iter().map(|id| DocInfo { doc_id: (*id).to_string(), title: (*id).to_string(), path: None, extra: Default::default() }).collect();
        record
    }

    fn pids(list: Vec<InstanceRecord>) -> Vec<u32> {
        list.into_iter().map(|record| record.pid).collect()
    }

    #[test]
    fn candidates_prefer_the_instance_that_lists_the_document() {
        let records = vec![
            record("filmcraft", 1, 10, &["other"]),
            record("filmcraft", 2, 20, &[]),
            record("filmcraft", 3, 5, &["doc"]),
            record("filmcraft", 4, 30, &["doc"]),
            record("effectcraft", 5, 99, &["doc"]),
        ];
        // 3 and 4 list the document (newest first), 2 lists nothing yet, 1 lists another
        // document and 5 is not FilmCraft.
        assert_eq!(pids(candidates(records, Some("doc"))), [4, 3, 2]);
    }

    #[test]
    fn candidates_without_a_doc_id_use_only_instances_with_no_documents() {
        let records = vec![record("filmcraft", 1, 10, &["a"]), record("filmcraft", 2, 20, &[])];
        assert_eq!(pids(candidates(records, None)), [2]);
    }
}
