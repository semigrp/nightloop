use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

use anyhow::{Context, Result};

static VERBOSE_COMMANDS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.status_code == 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandRunOptions<'a> {
    pub stdin: Option<&'a str>,
    pub stream_to_stderr: bool,
    pub print_command: bool,
    pub label: Option<&'a str>,
}

impl<'a> CommandRunOptions<'a> {
    pub fn streaming(label: &'a str) -> Self {
        Self {
            stdin: None,
            stream_to_stderr: true,
            print_command: true,
            label: Some(label),
        }
    }

    pub fn with_stdin(mut self, stdin: &'a str) -> Self {
        self.stdin = Some(stdin);
        self
    }
}

pub fn set_verbose_commands(enabled: bool) {
    VERBOSE_COMMANDS.store(enabled, Ordering::SeqCst);
}

pub fn run_shell_command(
    command: &str,
    workdir: &Path,
    envs: &[(String, String)],
    options: CommandRunOptions<'_>,
) -> Result<CommandResult> {
    let verbose = VERBOSE_COMMANDS.load(Ordering::SeqCst);
    let stream_to_stderr = verbose && options.stream_to_stderr;
    let print_command = verbose && options.print_command;
    let label = options.label.unwrap_or("command");

    if print_command {
        eprintln!("running[{label}]: {command}");
    }

    let mut child = Command::new("/bin/sh")
        .arg("-lc")
        .arg(command)
        .current_dir(workdir)
        .envs(envs.iter().cloned())
        .stdin(if options.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn command: {command}"))?;

    if let Some(input) = options.stdin {
        if let Some(mut handle) = child.stdin.take() {
            handle.write_all(input.as_bytes())?;
        }
    }

    let stdout = child.stdout.take().context("failed to capture stdout")?;
    let stderr = child.stderr.take().context("failed to capture stderr")?;
    let stderr_gate = Arc::new(Mutex::new(()));
    let stdout_handle = spawn_reader(stdout, stream_to_stderr, Arc::clone(&stderr_gate));
    let stderr_handle = spawn_reader(stderr, stream_to_stderr, stderr_gate);
    let status = child.wait()?;
    let stdout = join_reader(stdout_handle)?;
    let stderr = join_reader(stderr_handle)?;

    Ok(CommandResult {
        status_code: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
    })
}

fn spawn_reader<R>(
    mut reader: R,
    stream_to_stderr: bool,
    stderr_gate: Arc<Mutex<()>>,
) -> thread::JoinHandle<Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buf = [0_u8; 8192];
        loop {
            let read = reader.read(&mut buf)?;
            if read == 0 {
                break;
            }
            captured.extend_from_slice(&buf[..read]);
            if stream_to_stderr {
                let _guard = stderr_gate.lock().expect("stderr mutex poisoned");
                let mut stderr = std::io::stderr().lock();
                stderr.write_all(&buf[..read])?;
                stderr.flush()?;
            }
        }
        Ok(captured)
    })
}

fn join_reader(handle: thread::JoinHandle<Result<Vec<u8>>>) -> Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("command_output_reader_panicked"))?
}
