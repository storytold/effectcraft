//! `run --commands FILE|-`: the steps of a list of commands. This reader, [`parse_commands`] and
//! [`command_step`] are ported from vectorcraft-cli's (storytold/vectorcraft#3630), with a tuple in
//! place of its `Step` enum.

use std::io::Read;

use serde_json::{Value, json};

use crate::error::{CommandsFileError, StepError};

/// Most bytes `run --commands` reads from a file or standard input: the steps are untrusted input.
const MAX_COMMANDS_BYTES: u64 = 64 * 1024 * 1024;
/// Most steps one `run --commands` takes.
const MAX_COMMANDS_STEPS: usize = 100_000;

/// The text of `--commands source`: the file `source`, or standard input for `-`, refused past
/// [`MAX_COMMANDS_BYTES`].
pub fn read_commands(source: &str) -> Result<String, CommandsFileError> {
    let mut bytes = vec![];
    let read = if source == "-" {
        std::io::stdin().lock().take(MAX_COMMANDS_BYTES + 1).read_to_end(&mut bytes)
    } else {
        std::fs::File::open(source).and_then(|f| f.take(MAX_COMMANDS_BYTES + 1).read_to_end(&mut bytes))
    };
    read.map_err(|source_err| CommandsFileError::Read { source_name: source.to_owned(), source: source_err })?;
    if bytes.len() as u64 > MAX_COMMANDS_BYTES {
        return Err(CommandsFileError::TooBig(source.to_owned(), MAX_COMMANDS_BYTES / (1024 * 1024)));
    }
    String::from_utf8(bytes).map_err(|_| CommandsFileError::NotUtf8(source.to_owned()))
}

/// The steps of `--commands source` from its `text`: a JSON array of `{"command": ID, "params":
/// {…}}` objects (`"id"` may stand for `"command"`), or one such object per line (JSON Lines, blank
/// lines skipped). `first` is the run's index of the first of them, which an error names with the
/// entry's place in the text.
pub fn parse_commands(source: &str, text: &str, first: usize) -> Result<Vec<(String, Value)>, CommandsFileError> {
    // Notepad on Windows starts UTF-8 files with a byte order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let too_many = || CommandsFileError::TooMany(source.to_owned(), MAX_COMMANDS_STEPS);
    // Each entry with where it is in the text: its line for JSON Lines.
    let mut entries: Vec<(Option<usize>, Value)> = vec![];
    if text.trim_start().starts_with('[') {
        let v: Value = serde_json::from_str(text).map_err(|e| CommandsFileError::Json(source.to_owned(), e))?;
        let Value::Array(items) = v else { return Err(CommandsFileError::NotAnArray(source.to_owned())) };
        if items.len() > MAX_COMMANDS_STEPS {
            return Err(too_many());
        }
        entries.extend(items.into_iter().map(|v| (None, v)));
    } else {
        for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            if entries.len() == MAX_COMMANDS_STEPS {
                return Err(too_many());
            }
            let v = serde_json::from_str(line).map_err(|error| CommandsFileError::Line {
                source_name: source.to_owned(),
                step: first + entries.len(),
                line: n + 1,
                error,
            })?;
            entries.push((Some(n + 1), v));
        }
    }
    entries
        .into_iter()
        .enumerate()
        .map(|(k, (line, v))| {
            command_step(v).map_err(|error| {
                let place = line.map_or_else(|| format!("entry {}", k + 1), |l| format!("line {l}"));
                CommandsFileError::Step { source_name: source.to_owned(), step: first + k, place, error }
            })
        })
        .collect()
}

/// The command id and params of one `--commands` entry, `{"command": ID, "params": {…}}`.
fn command_step(v: Value) -> Result<(String, Value), StepError> {
    let Value::Object(mut o) = v else { return Err(StepError::NotAnObject) };
    let id = match (o.remove("command"), o.remove("id")) {
        (Some(Value::String(id)), None) | (None, Some(Value::String(id))) => id,
        (Some(_), Some(_)) => return Err(StepError::BothIds),
        (None, None) => return Err(StepError::NoCommand),
        _ => return Err(StepError::IdNotAString),
    };
    let params = match o.remove("params") {
        None | Some(Value::Null) => json!({}),
        Some(p @ Value::Object(_)) => p,
        Some(_) => return Err(StepError::ParamsNotAnObject(id)),
    };
    if let Some(key) = o.keys().next() {
        return Err(StepError::UnknownKey(key.clone()));
    }
    Ok((id, params))
}
