use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Result};

/// Generate a commit message by piping `git diff --cached` via stdin to the configured command.
#[allow(dead_code)]
pub fn generate_commit_message(repo_path: &Path, generate_command: &str) -> Result<String> {
    let cancel = Arc::new(AtomicBool::new(false));
    match generate_commit_message_cancellable(repo_path, generate_command, cancel)? {
        Some(message) => Ok(message),
        None => bail!("AI commit generation cancelled"),
    }
}

/// Generate a commit message like [`generate_commit_message`], but return
/// `Ok(None)` when `cancel` is set while the external generator is running.
pub fn generate_commit_message_cancellable(
    repo_path: &Path,
    generate_command: &str,
    cancel: Arc<AtomicBool>,
) -> Result<Option<String>> {
    if generate_command.is_empty() {
        bail!("No generateCommand configured. Set git.commit.generateCommand in your config.");
    }

    // Get the staged diff
    let diff_output = Command::new("git")
        .args(["diff", "--cached"])
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    if !diff_output.status.success() {
        bail!("Failed to get staged diff");
    }

    let diff = String::from_utf8_lossy(&diff_output.stdout);
    if diff.trim().is_empty() {
        bail!("No staged changes to generate a commit message for");
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }

    // Run the generate command via shell, piping diff via stdin
    let mut child = Command::new("sh")
        .args(["-c", generate_command])
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(diff.as_bytes())?;
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_handle = thread::spawn(move || read_pipe(stdout));
    let stderr_handle = thread::spawn(move || read_pipe(stderr));

    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Ok(None);
        }

        if let Some(status) = child.try_wait()? {
            break status;
        }

        thread::sleep(Duration::from_millis(50));
    };

    let stdout = stdout_handle
        .join()
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "stdout reader panicked",
            ))
        })?;
    let stderr = stderr_handle
        .join()
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "stderr reader panicked",
            ))
        })?;

    if !status.success() {
        bail!("Generate command failed: {}", stderr.trim());
    }

    Ok(Some(strip_markdown_fences(&stdout)))
}

fn read_pipe(pipe: Option<impl Read>) -> std::io::Result<String> {
    let mut output = String::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_string(&mut output)?;
    }
    Ok(output)
}

/// Strip markdown code fences and any preamble text before the commit message.
fn strip_markdown_fences(raw: &str) -> String {
    let trimmed = raw.trim();

    // If the output contains a code fence, extract content from within it
    if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        // Skip the language identifier on the opening fence line
        let content_start = after_fence.find('\n').map(|i| i + 1).unwrap_or(0);
        let content = &after_fence[content_start..];

        if let Some(end) = content.find("```") {
            return content[..end].trim().to_string();
        }
        // No closing fence — use everything after the opening
        return content.trim().to_string();
    }

    // Strip single backticks from the first line (e.g. `feat: blah blah`)
    // The AI sometimes wraps only the subject line in backticks.
    let mut lines: Vec<&str> = trimmed.lines().collect();
    if let Some(first) = lines.first_mut() {
        if let Some(stripped) = first.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
            *first = stripped;
        }
    }

    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_markdown_fences_plain() {
        assert_eq!(
            strip_markdown_fences("fix: update login"),
            "fix: update login"
        );
    }

    #[test]
    fn test_strip_markdown_fences_with_fences() {
        let input = "Here's a commit message:\n\n```\nfeat: add user auth\n\nAdded JWT-based authentication.\n```\n";
        assert_eq!(
            strip_markdown_fences(input),
            "feat: add user auth\n\nAdded JWT-based authentication."
        );
    }

    #[test]
    fn test_strip_single_backticks() {
        assert_eq!(
            strip_markdown_fences("`feat: blah blah blah`"),
            "feat: blah blah blah"
        );
    }

    #[test]
    fn test_strip_single_backticks_first_line_only() {
        let input = "`feat: something something`\n\nother content of the commit here stuff\nblah blah blah";
        assert_eq!(
            strip_markdown_fences(input),
            "feat: something something\n\nother content of the commit here stuff\nblah blah blah"
        );
    }

    #[test]
    fn test_strip_markdown_fences_with_language() {
        let input = "```text\nfix: resolve race condition\n```";
        assert_eq!(strip_markdown_fences(input), "fix: resolve race condition");
    }
}
