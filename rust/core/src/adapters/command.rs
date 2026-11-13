//! Running external programs: [`StdCommandRunner`], and the wake method that runs a command.
//!
//! [`StdCommandRunner`] never uses a shell: the program and its arguments are passed as they
//! are, so nothing needs quoting and nothing can be injected.
//!
//! - **Timeout.** The program is waited for at most the timeout and then killed. On Unix it
//!   runs in its own process group and the whole group is killed (by running `kill -KILL --
//!   -<pid>`, so no extra dependency is needed; if no `kill` program exists only the program
//!   itself is killed). On Windows only the program itself is killed; its children, if any,
//!   are not (the programs this application runs, such as `ssh`, have none that matter).
//! - **Output.** Standard error and standard output are read concurrently, at most
//!   [`StdCommandRunner::max_output`] bytes of each are kept (the rest is discarded while
//!   draining, so the program never blocks on a full pipe) and decoded with a pluggable
//!   function, so the Windows app can supply an OEM-code-page decoder.
//! - **Windows.** The program is started with `CREATE_NO_WINDOW`, so no console window
//!   flashes.
//! - **Program lookup.** By default programs are found through `PATH` by the operating
//!   system. [`StdCommandRunner::with_program_dir`] makes a directory the only place bare
//!   program names are looked up (an explicit path in the first argument is always used as
//!   is), to bundle or to substitute tools, for example in tests.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::WakeMethod;
use crate::ports::{CommandOutput, CommandRunner, WakeError, WakeRequest, WakeSender};
use crate::ssh::explain_command;

/// Turns the bytes a program wrote into text.
pub type Decoder = Arc<dyn Fn(&[u8]) -> String + Send + Sync>;

/// The default decoder: UTF-8, invalid bytes replaced.
pub fn utf8_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Default limit of kept output per stream, in bytes.
pub const DEFAULT_MAX_OUTPUT: usize = 16 * 1024;

/// How long to wait for the output readers after the program ended.
const READER_GRACE: Duration = Duration::from_millis(500);
/// Longest sleep between checks whether the program has exited.
const MAX_POLL: Duration = Duration::from_millis(20);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The portable [`CommandRunner`].
#[derive(Clone)]
pub struct StdCommandRunner {
    decode: Decoder,
    max_output: usize,
    program_dir: Option<PathBuf>,
}

impl Default for StdCommandRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl StdCommandRunner {
    /// A runner that decodes output as UTF-8 and keeps [`DEFAULT_MAX_OUTPUT`] bytes per stream.
    pub fn new() -> Self {
        Self {
            decode: Arc::new(utf8_lossy),
            max_output: DEFAULT_MAX_OUTPUT,
            program_dir: None,
        }
    }

    /// Uses `decode` to turn output bytes into text.
    pub fn with_decoder(mut self, decode: Decoder) -> Self {
        self.decode = decode;
        self
    }

    /// Keeps at most `bytes` bytes of each output stream.
    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output = bytes;
        self
    }

    /// Looks bare program names up only in `dir`.
    pub fn with_program_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.program_dir = Some(dir.into());
        self
    }

    /// The most bytes kept of each output stream.
    pub fn max_output(&self) -> usize {
        self.max_output
    }

    fn program(&self, name: &str) -> Result<OsString, CommandOutput> {
        let has_path = name.contains('/') || name.contains('\\');
        match (&self.program_dir, has_path) {
            (Some(dir), false) => {
                let path = dir.join(name);
                if path.is_file() {
                    Ok(path.into_os_string())
                } else {
                    Err(CommandOutput::not_started(format!(
                        "{name}: not found in {}",
                        dir.display()
                    )))
                }
            }
            _ => Ok(OsString::from(name)),
        }
    }

    fn text(&self, stderr: &Captured, stdout: &Captured) -> String {
        let mut parts = Vec::new();
        for captured in [stderr, stdout] {
            if captured.bytes.is_empty() {
                continue;
            }
            let text = (self.decode)(&captured.bytes);
            let text = text.trim();
            if !text.is_empty() {
                let more = if captured.truncated {
                    " …(truncated)"
                } else {
                    ""
                };
                parts.push(format!("{text}{more}"));
            }
        }
        parts.join("\n")
    }
}

impl CommandRunner for StdCommandRunner {
    fn run(&self, argv: &[String], timeout: Duration) -> CommandOutput {
        let Some((name, args)) = argv.split_first().filter(|(name, _)| !name.is_empty()) else {
            return CommandOutput::not_started("empty command");
        };
        let program = match self.program(name) {
            Ok(program) => program,
            Err(output) => return output,
        };
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure(&mut command);
        let mut child = match spawn(&mut command) {
            Ok(child) => child,
            Err(err) => return CommandOutput::not_started(format!("{name}: {err}")),
        };
        let stdout = spawn_reader(child.stdout.take(), self.max_output);
        let stderr = spawn_reader(child.stderr.take(), self.max_output);

        let (status, timed_out) = wait(&mut child, timeout);
        let stderr = stderr.collect();
        let stdout = stdout.collect();
        let text = self.text(&stderr, &stdout);
        if timed_out {
            return CommandOutput::timed_out(timeout, &text);
        }
        match status {
            Some(code) => CommandOutput::exited(code, text),
            None => CommandOutput::not_started(format!("{name}: cannot wait for the program")),
        }
    }
}

/// Starts the program. A file that another thread has just finished writing can briefly be
/// "busy" when this runs next to a `fork` in the same process (`ETXTBSY`); that passes in
/// moments, so a few short retries are made.
fn spawn(command: &mut Command) -> std::io::Result<Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 10 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

#[cfg(unix)]
fn configure(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // Its own process group, so that a timeout can kill the program and what it started.
    command.process_group(0);
}

#[cfg(windows)]
fn configure(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
fn configure(_command: &mut Command) {}

/// Waits for the program; kills it when `timeout` passes. Returns the exit code (if it could
/// be read) and whether it was killed for taking too long.
fn wait(child: &mut Child, timeout: Duration) -> (Option<i32>, bool) {
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (Some(exit_code(status)), false),
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    kill(child);
                    return (None, true);
                }
                thread::sleep(pause.min(deadline - now));
                pause = (pause * 2).min(MAX_POLL);
            }
            Err(_) => {
                kill(child);
                return (None, false);
            }
        }
    }
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    // A program killed by a signal has no exit code; use the shell convention.
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// Kills the program, on Unix the whole process group, and reaps it.
fn kill(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// What one output stream held.
#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

/// A thread reading one stream; [`Reader::collect`] waits for it, briefly.
struct Reader(Option<Receiver<Captured>>);

impl Reader {
    fn collect(self) -> Captured {
        match self.0 {
            Some(rx) => rx.recv_timeout(READER_GRACE).unwrap_or_default(),
            None => Captured::default(),
        }
    }
}

fn spawn_reader<R: Read + Send + 'static>(stream: Option<R>, max: usize) -> Reader {
    let Some(mut stream) = stream else {
        return Reader(None);
    };
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("homelink-output".into())
        .spawn(move || {
            let mut captured = Captured::default();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let room = max.saturating_sub(captured.bytes.len());
                        let keep = n.min(room);
                        captured
                            .bytes
                            .extend_from_slice(chunk.get(..keep).unwrap_or_default());
                        if keep < n {
                            captured.truncated = true; // keep reading, and discard
                        }
                    }
                }
            }
            let _ = tx.send(captured);
        });
    match spawned {
        Ok(_) => Reader(Some(rx)),
        Err(_) => Reader(None),
    }
}

/// Wakes a PC with [`WakeMethod::Command`]: `{host}` and `{mac}` are replaced in the
/// configured arguments, and the program runs through a [`CommandRunner`].
pub struct CommandWake {
    runner: Arc<dyn CommandRunner>,
}

impl CommandWake {
    /// A sender that runs commands through `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

impl WakeSender for CommandWake {
    fn wake(&self, request: &WakeRequest<'_>) -> Result<(), WakeError> {
        let WakeMethod::Command { argv, mac, timeout } = request.method else {
            return Err(WakeError::Unsupported);
        };
        let argv = argv.expand(&request.pc.host, mac.as_ref());
        log::info!("waking {} with a command", request.pc.id);
        let output = self.runner.run(&argv, *timeout);
        match explain_command(&output) {
            None => Ok(()),
            Some(reason) => Err(WakeError::Failed(reason)),
        }
    }
}
