//! Running external tools.
//!
//! Checkers run in the background: results arrive over a channel and are
//! dropped if the buffer changed while the tool was thinking. Formatters run
//! synchronously with a timeout, because the caller is waiting for the result
//! and writing an unformatted file first would be worse than a brief pause.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Placeholder replaced with a path to the buffer's current contents.
const FILE_PLACEHOLDER: &str = "$FILE";
/// Placeholder replaced with the buffer's real name, for tools that report it
/// back or pick a parser from it.
const NAME_PLACEHOLDER: &str = "$NAME";

#[derive(Debug)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub status: Option<i32>,
}

impl Output {
    /// Checkers write to either stream depending on the tool, so callers that
    /// parse diagnostics look at both.
    pub fn combined(&self) -> String {
        if self.stderr.trim().is_empty() {
            self.stdout.clone()
        } else if self.stdout.trim().is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

/// A temporary copy of the buffer, kept alive for as long as the tool needs it.
struct Scratch {
    path: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write the buffer somewhere a tool can read it, preserving the extension so
/// tools that sniff the file type still work.
fn scratch_copy(name: &str, text: &str) -> std::io::Result<Scratch> {
    let extension = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("buffer");
    let unique = format!(
        "miv-{}-{:?}-{stem}{extension}",
        std::process::id(),
        thread_counter()
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::write(&path, text)?;
    Ok(Scratch { path })
}

fn thread_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Run `command`, feeding the buffer through a temporary file when the command
/// mentions `$FILE` and through stdin otherwise.
pub fn run(
    command: &[String],
    name: &str,
    text: &str,
    working_directory: Option<&Path>,
    timeout: Duration,
) -> std::io::Result<Output> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"))?;

    let wants_file = arguments.iter().any(|a| a.contains(FILE_PLACEHOLDER));
    let scratch = if wants_file {
        Some(scratch_copy(name, text)?)
    } else {
        None
    };
    let file_path = scratch
        .as_ref()
        .map(|s| s.path.display().to_string())
        .unwrap_or_default();

    let arguments: Vec<String> = arguments
        .iter()
        .map(|argument| {
            argument
                .replace(FILE_PLACEHOLDER, &file_path)
                .replace(NAME_PLACEHOLDER, name)
        })
        .collect();

    let mut process = Command::new(program);
    process
        .args(&arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = working_directory {
        process.current_dir(directory);
    }
    let mut child = process.spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        if !wants_file {
            let _ = stdin.write_all(text.as_bytes());
        }
        drop(stdin);
    }

    // std has no wait-with-timeout, so wait on a thread and give up on the
    // channel instead. A wedged tool must not wedge the editor.
    let (done_tx, done_rx) = channel();
    thread::spawn(move || {
        let _ = done_tx.send(child.wait_with_output());
    });

    match done_rx.recv_timeout(timeout) {
        Ok(Ok(output)) => {
            drop(scratch);
            Ok(Output {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                status: output.status.code(),
            })
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("{program} did not finish within {:?}", timeout),
        )),
    }
}

/// Does a tool configured for these filetypes and extensions apply to this
/// buffer? Matching on either the detected syntax or the file name keeps
/// configuration predictable: extensions are the reliable half for the
/// configuration files this is mostly aimed at.
pub fn applies(
    filetypes: &[String],
    extensions: &[String],
    syntax_name: &str,
    path: Option<&Path>,
) -> bool {
    if filetypes.is_empty() && extensions.is_empty() {
        return true;
    }
    let syntax = syntax_name.to_ascii_lowercase();
    if filetypes
        .iter()
        .any(|wanted| syntax.contains(wanted.as_str()))
    {
        return true;
    }
    let extension = path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let name = path
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase());
    extensions.iter().any(|wanted| {
        extension.as_deref() == Some(wanted.as_str()) || name.as_deref() == Some(wanted.as_str())
    })
}

/// What a finished background job produced.
pub struct Failure {
    pub message: String,
    /// The command does not exist on this machine. Worth saying once, not
    /// every time the file is saved.
    pub missing: bool,
}

pub enum Finished {
    Checked {
        buffer_id: usize,
        revision: usize,
        tool: String,
        result: Result<Vec<crate::diagnostics::Diagnostic>, Failure>,
    },
    Diffed {
        buffer_id: usize,
        revision: usize,
        result: Result<crate::vcs::LineStatuses, String>,
    },
}

/// Background jobs and their results.
pub struct Runner {
    sender: Sender<Finished>,
    results: Receiver<Finished>,
}

impl Default for Runner {
    fn default() -> Self {
        let (sender, results) = channel();
        Self { sender, results }
    }
}

/// A snapshot of a buffer for a background tool to work on.
///
/// The revision travels with it so a result arriving after the text moved on
/// can be thrown away rather than shown against the wrong lines.
#[derive(Clone)]
pub struct Snapshot {
    pub buffer_id: usize,
    pub revision: usize,
    pub name: String,
    pub text: String,
    pub directory: Option<PathBuf>,
    pub timeout: Duration,
}

impl Runner {
    /// Run one checker over a snapshot of the buffer.
    pub fn check(&self, checker: crate::diagnostics::Checker, snapshot: Snapshot) {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let Snapshot {
                buffer_id,
                revision,
                name,
                text,
                directory,
                timeout,
            } = snapshot;
            let outcome = run(
                &checker.command,
                &name,
                &text,
                directory.as_deref(),
                timeout,
            );
            let result = match outcome {
                Ok(output) => Ok(checker.parse(&output.combined())),
                Err(e) => Err(Failure {
                    missing: e.kind() == std::io::ErrorKind::NotFound,
                    message: if e.kind() == std::io::ErrorKind::NotFound {
                        format!("{} is not installed", checker.command[0])
                    } else {
                        format!("{}: {e}", checker.name)
                    },
                }),
            };
            let _ = sender.send(Finished::Checked {
                buffer_id,
                revision,
                tool: checker.name,
                result,
            });
        });
    }

    /// Diff the buffer against its committed version.
    pub fn diff(&self, path: PathBuf, snapshot: Snapshot) {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = crate::vcs::diff_against_head(&path, &snapshot.text, snapshot.timeout);
            let _ = sender.send(Finished::Diffed {
                buffer_id: snapshot.buffer_id,
                revision: snapshot.revision,
                result,
            });
        });
    }

    pub fn poll(&self) -> Vec<Finished> {
        self.results.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    const QUICK: Duration = Duration::from_secs(10);

    #[test]
    fn a_command_without_the_file_placeholder_gets_the_buffer_on_stdin() {
        let output = run(&command(&["cat"]), "x.txt", "hello\nworld\n", None, QUICK).unwrap();
        assert_eq!(output.stdout, "hello\nworld\n");
        assert_eq!(output.status, Some(0));
    }

    #[test]
    fn the_file_placeholder_becomes_a_readable_copy_of_the_buffer() {
        let output = run(
            &command(&["cat", "$FILE"]),
            "x.txt",
            "from a file\n",
            None,
            QUICK,
        )
        .unwrap();
        assert_eq!(output.stdout, "from a file\n");
    }

    #[test]
    fn the_temporary_copy_keeps_the_extension_so_tools_can_sniff_it() {
        let output = run(
            &command(&["sh", "-c", "basename \"$0\"", "$FILE"]),
            "deploy.yaml",
            "a: 1\n",
            None,
            QUICK,
        )
        .unwrap();
        assert!(
            output.stdout.trim().ends_with(".yaml"),
            "got {:?}",
            output.stdout
        );
        assert!(output.stdout.contains("deploy"));
    }

    #[test]
    fn the_name_placeholder_passes_the_real_file_name() {
        let output = run(&command(&["echo", "$NAME"]), "deploy.yaml", "", None, QUICK).unwrap();
        assert_eq!(output.stdout.trim(), "deploy.yaml");
    }

    #[test]
    fn the_temporary_copy_is_removed_afterwards() {
        let output = run(
            &command(&["sh", "-c", "echo \"$0\"", "$FILE"]),
            "x.txt",
            "gone\n",
            None,
            QUICK,
        )
        .unwrap();
        let path = std::path::PathBuf::from(output.stdout.trim());
        assert!(!path.exists(), "{} was left behind", path.display());
    }

    #[test]
    fn a_tool_that_never_finishes_is_abandoned() {
        let error = run(
            &command(&["sleep", "30"]),
            "x.txt",
            "",
            None,
            Duration::from_millis(150),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_missing_tool_is_reported_as_not_found() {
        let error = run(
            &command(&["miv-no-such-tool-exists"]),
            "x.txt",
            "",
            None,
            QUICK,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn stderr_is_read_too_because_tools_disagree_about_where_to_write() {
        let output = run(
            &command(&["sh", "-c", "echo oops >&2"]),
            "x.txt",
            "",
            None,
            QUICK,
        )
        .unwrap();
        assert!(output.stdout.trim().is_empty());
        assert_eq!(output.combined().trim(), "oops");
    }

    #[test]
    fn filetype_matching_accepts_a_syntax_name_or_a_file_name() {
        let filetypes = vec!["yaml".to_string()];
        let extensions = vec!["yml".to_string(), "dockerfile".to_string()];
        let path = std::path::Path::new("/tmp/deploy.yml");

        assert!(applies(&filetypes, &[], "YAML", None));
        assert!(applies(&[], &extensions, "Plain Text", Some(path)));
        assert!(applies(
            &[],
            &extensions,
            "Plain Text",
            Some(std::path::Path::new("/srv/Dockerfile"))
        ));
        assert!(!applies(&filetypes, &extensions, "Rust", None));
        // No filters at all means "everything".
        assert!(applies(&[], &[], "Rust", None));
    }
}
