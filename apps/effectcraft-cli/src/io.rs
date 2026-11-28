//! The program's two output streams. They are the caller's, so `run` can be tested in-process.

use std::fmt::Arguments;
use std::io::{ErrorKind, Write};

/// What a command writes to: stdout for results and stderr for diagnostics.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    /// Why stdout stopped taking output, once it has.
    out_failed: Option<ErrorKind>,
}

impl<'a> Io<'a> {
    pub fn new(out: &'a mut dyn Write, err: &'a mut dyn Write) -> Self {
        Io { out, err, out_failed: None }
    }

    /// Write a line to stdout. Once that fails, later output is dropped but the command still
    /// finishes its work (a `run … --save` sequence isn't cut short). A reader that closes stdout
    /// early (`info --json | head`) has read what it wanted; any other failure is reported and makes
    /// the exit status 1 ([`Io::stdout_broken`]).
    pub fn say(&mut self, text: Arguments) {
        if self.out_failed.is_some() {
            return;
        }
        let r = self.out.write_fmt(text).and_then(|()| self.out.write_all(b"\n"));
        if let Err(e) = r {
            self.out_failed = Some(e.kind());
            if e.kind() != ErrorKind::BrokenPipe {
                self.note(format_args!("effectcraft-cli: cannot write to stdout: {e}"));
            }
        }
    }

    /// Write a line to stderr; a diagnostic that can't be written is dropped.
    pub fn note(&mut self, text: Arguments) {
        let _ = writeln!(self.err, "{text}");
    }

    /// Write to stderr as it is, with no newline (a progress line that redraws itself).
    pub fn raw_err(&mut self, text: Arguments) {
        let _ = self.err.write_fmt(text);
    }

    /// Whether stdout failed for a reason other than its reader closing it early.
    pub fn stdout_broken(&self) -> bool {
        self.out_failed.is_some_and(|k| k != ErrorKind::BrokenPipe)
    }

    /// Flush stdout at the end of the run; a failure is recorded like a failed write.
    pub fn flush(&mut self) {
        if self.out_failed.is_some() {
            return;
        }
        if let Err(e) = self.out.flush() {
            self.out_failed = Some(e.kind());
            if e.kind() != ErrorKind::BrokenPipe {
                self.note(format_args!("effectcraft-cli: cannot write to stdout: {e}"));
            }
        }
    }
}

/// `println!` into [`Io`]: see [`Io::say`].
macro_rules! say {
    ($io:expr, $($t:tt)*) => {
        $io.say(format_args!($($t)*))
    };
}

/// `eprintln!` into [`Io`]: see [`Io::note`].
macro_rules! note {
    ($io:expr, $($t:tt)*) => {
        $io.note(format_args!($($t)*))
    };
}
