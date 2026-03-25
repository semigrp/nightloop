use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result};

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

pub fn run_shell_command(
    command: &str,
    workdir: &Path,
    envs: &[(String, String)],
    stdin: Option<&str>,
) -> Result<CommandResult> {
    let mut child = Command::new("/bin/sh")
        .arg("-lc")
        .arg(command)
        .current_dir(workdir)
        .envs(envs.iter().cloned())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn command: {command}"))?;

    if let Some(input) = stdin {
        if let Some(mut handle) = child.stdin.take() {
            handle.write_all(input.as_bytes())?;
        }
    }

    let output = child.wait_with_output()?;
    Ok(CommandResult {
        status_code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}
