//! Handing a long report to a pager, the way a person expects of a terminal tool.
//!
//! A price report runs to more than a screen, and a reader who has to scroll their
//! terminal back up to find the class has lost the answer the report led with. So: page
//! it, but only when there is a person on the other end of stdout, and only when there is
//! enough of it to be worth the interruption.

use std::io::{self, IsTerminal, Write};
use std::process::{Child, Command, Stdio};

/// `less`, told to behave: quit rather than page something that already fits on one
/// screen (`-F`), search case-insensitively (`-I`), pass the colour through instead of
/// printing the escape codes at the reader (`-R`), and leave the report on screen after
/// quitting rather than wiping the terminal back to the prompt (`-X`).
const LESS_ARGUMENTS: [&str; 4] = ["-F", "-I", "-R", "-X"];

/// Renders the report into a buffer, then sends it to a pager or straight to stdout.
///
/// Buffered rather than streamed because the decision needs the length, and the length is
/// only known once the thing is rendered. A report is kilobytes; holding one in memory
/// costs nothing next to the request that produced it.
pub fn paged(
    no_pager: bool,
    render: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
) -> io::Result<()> {
    let mut rendered = Vec::new();
    render(&mut rendered)?;
    let pager = if worth_paging(no_pager, &rendered) {
        Pager::start()
    } else {
        None
    };
    send(&rendered, pager, &mut io::stdout().lock())
}

/// Whether this report is worth handing to a pager.
///
/// Only an interactive terminal gets one. Stdout redirected to a file or into another
/// program is not a reader, and paging it would hand that program a paused process
/// instead of a report.
fn worth_paging(no_pager: bool, rendered: &[u8]) -> bool {
    if no_pager || !io::stdout().is_terminal() {
        return false;
    }
    longer_than(rendered, screen_height())
}

/// Whether `rendered` needs more than `height` lines to show.
///
/// A report that already fits is better left where the reader can see it alongside the
/// command that produced it. `less -F` would quit straight back out of it anyway.
fn longer_than(rendered: &[u8], height: usize) -> bool {
    rendered.iter().filter(|byte| **byte == b'\n').count() > height
}

/// The screen the report has to fit on, in lines.
///
/// The fallback is the classic terminal, which is the safe way to be wrong: guessing too
/// short pages something that would have fitted, guessing too tall scrolls the answer off
/// the top.
fn screen_height() -> usize {
    terminal_size::terminal_size()
        .map(|(_, terminal_size::Height(rows))| usize::from(rows))
        .unwrap_or(24)
}

/// Sends a rendered report to `pager`, or to `fallback` when there is no pager to send it
/// to.
fn send(rendered: &[u8], pager: Option<Pager>, fallback: &mut impl Write) -> io::Result<()> {
    let Some(mut pager) = pager else {
        return delivered(fallback.write_all(rendered).and_then(|()| fallback.flush()));
    };
    let written = pager.write_all(rendered).and_then(|()| pager.flush());
    pager.finish()?;
    delivered(written)
}

/// Reads a broken pipe as a report delivered rather than a run failed.
///
/// Whoever was reading has stopped: a pager quit at the third band, or a `| head` that
/// had its three lines. Both read what they came for, and neither wants an error about it
/// on the way out.
fn delivered(written: io::Result<()>) -> io::Result<()> {
    match written {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(error),
        _ => Ok(()),
    }
}

/// The pager the report is being written into.
///
/// Owns the child process as well as the pipe, because the two have to be finished with
/// in that order: the pipe closes to tell the pager the report has ended, and the wait
/// keeps gearprice alive until the reader quits. Exiting first would return the shell
/// prompt underneath a pager still holding the screen, and the two then fight over it.
struct Pager {
    child: Child,
    waited: bool,
}

impl Pager {
    /// Starts the reader's pager, or `None` if there is none to start.
    fn start() -> Option<Self> {
        Self::spawn(command()?)
    }

    fn spawn(mut command: Command) -> Option<Self> {
        // A pager that will not start is not worth a word to the reader: the report is
        // about to be printed either way, and it is the report they asked for.
        let child = command.stdin(Stdio::piped()).spawn().ok()?;
        Some(Self {
            child,
            waited: false,
        })
    }

    /// Ends the report and waits for the reader to quit the pager.
    ///
    /// Dropping a `Pager` does the same, so an error on the way out cannot leave
    /// gearprice racing its own pager back to the prompt. This is the way to say so on
    /// purpose, and the way to hear about it if the wait fails.
    fn finish(mut self) -> io::Result<()> {
        self.close()
    }

    fn close(&mut self) -> io::Result<()> {
        // The pipe goes before the wait, never after: `less` reads until the write end is
        // gone, so waiting with it still open leaves both processes waiting on the other.
        drop(self.child.stdin.take());
        if std::mem::replace(&mut self.waited, true) {
            return Ok(());
        }
        self.child.wait().map(|_status| ())
    }
}

impl Write for Pager {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self.child.stdin.as_mut() {
            Some(stdin) => stdin.write(buffer),
            // Only `close` takes the pipe, and it consumes or drops the pager as it does.
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the pager has already been finished with",
            )),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.child.stdin.as_mut() {
            Some(stdin) => stdin.flush(),
            None => Ok(()),
        }
    }
}

impl Drop for Pager {
    fn drop(&mut self) {
        // Nothing useful to say about a wait that failed while unwinding.
        let _ = self.close();
    }
}

/// The pager to run.
///
/// `PAGER` first, because a reader who has chosen one has chosen it for every tool they
/// use. An empty `PAGER` is the usual way of saying "no pager at all", and is read as it.
fn command() -> Option<Command> {
    // Split on whitespace rather than handed to a shell: `PAGER="less -S"` is common and
    // worth honouring, and spawning a shell to read a variable promises a great deal more
    // than that — quoting, globbing, and whatever else is in the string.
    let mut command = match std::env::var("PAGER") {
        Ok(configured) => {
            let mut words = configured.split_whitespace();
            let mut command = Command::new(words.next()?);
            command.args(words);
            command
        }
        Err(_) => {
            let mut command = Command::new("less");
            command.args(LESS_ARGUMENTS);
            command
        }
    };
    // `less` takes its own defaults from LESS, so a bare `PAGER=less` still passes the
    // colour through rather than printing the escape codes at the reader. A LESS the
    // reader set themselves is theirs, and left alone.
    if std::env::var_os("LESS").is_none() {
        command.env("LESS", "FIRX");
    }
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &[u8] =
        b"GEARPRICE  Gibson Les Paul Standard\n\x1b[2m\xe2\x95\x90\x1b[0m\nSold  $2,400\n";

    #[test]
    fn a_report_that_is_not_going_to_a_terminal_is_never_paged() {
        // Stdout under a test harness is a pipe, which is the case that must not be
        // paged: the thing reading it is another program, not a person.
        let long = [b'\n'; 500];
        assert!(!worth_paging(false, &long));
        assert!(!worth_paging(true, &long));
    }

    #[test]
    fn only_a_report_that_will_not_fit_on_the_screen_is_worth_paging() {
        assert!(!longer_than(&[b'\n'; 24], 24));
        assert!(longer_than(&[b'\n'; 25], 24));
        // A last line with no newline after it still counts for nothing, which is the
        // right way round: it is the line the prompt lands on.
        assert!(!longer_than(b"one line and no newline at all", 24));
        assert!(!longer_than(b"", 24));
    }

    #[test]
    fn a_missing_pager_never_fails_a_run() {
        let missing = Pager::spawn(Command::new("gearprice-has-no-pager-by-this-name"));
        assert!(missing.is_none(), "expected nothing to spawn");
        let mut fallback = Vec::new();
        send(REPORT, missing, &mut fallback).expect("a pager that is not there is not an error");
        // And the report still reached the reader, whole.
        assert_eq!(REPORT, fallback.as_slice());
    }

    /// The pagers here are ordinary Unix commands, so the test is only meaningful there.
    #[cfg(unix)]
    #[test]
    fn a_pager_that_quits_without_reading_is_not_a_failure_either() {
        let quitter = Pager::spawn(Command::new("false")).expect("`false` is on the path");
        let mut fallback = Vec::new();
        send(REPORT, Some(quitter), &mut fallback)
            .expect("a reader who quits early has not failed the run");
        // Nothing was printed underneath it: the report went to the pager, and a pager
        // that threw it away is the reader's own arrangement.
        assert!(fallback.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_report_is_the_same_bytes_through_a_pager_as_without_one() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("paged-report");
        let mut pager = Command::new("sh");
        pager.arg("-c").arg(format!("cat > '{}'", path.display()));
        let pager = Pager::spawn(pager).expect("`sh` is on the path");

        send(REPORT, Some(pager), &mut Vec::new()).expect("the report goes through the pager");
        // `finish` waited for the child, so the file is complete by the time it returns.
        let through_a_pager = std::fs::read(&path).expect("the pager wrote the report out");

        let mut direct = Vec::new();
        send(REPORT, None, &mut direct).expect("the report goes straight out");

        assert_eq!(direct, through_a_pager);
        assert_eq!(REPORT, through_a_pager.as_slice());
    }
}
