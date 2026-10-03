use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::config::AppConfig;
use crate::git::GitCommands;
use crate::gui::Gui;

#[cfg(unix)]
fn relaunch_repo(path: &Path, debug: bool) -> Result<()> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lazygitrs"));
    let mut cmd = Command::new(exe);
    cmd.arg("--path").arg(path);
    if debug {
        cmd.arg("--debug");
    }
    let err = cmd.exec();
    Err(err).with_context(|| format!("Failed to exec lazygitrs in '{}'", path.display()))
}

#[cfg(not(unix))]
fn relaunch_repo(path: &Path, debug: bool) -> Result<()> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lazygitrs"));
    let mut cmd = Command::new(exe);
    cmd.arg("--path").arg(path);
    if debug {
        cmd.arg("--debug");
    }
    cmd.spawn()
        .with_context(|| format!("Failed to open lazygitrs in '{}'", path.display()))?;
    Ok(())
}

pub struct App {
    pub config: AppConfig,
    pub repo_path: PathBuf,
    pub filter_path: Option<PathBuf>,
}

impl App {
    pub fn new(repo_path: PathBuf, debug: bool, filter_path: Option<PathBuf>) -> Result<Self> {
        let config = AppConfig::load(debug)?;

        // Validate git repo
        if !GitCommands::is_valid_repo(&repo_path) {
            anyhow::bail!("'{}' is not a git repository", repo_path.display());
        }

        Ok(Self {
            config,
            repo_path,
            filter_path,
        })
    }

    pub fn run(mut self) -> Result<()> {
        let git = GitCommands::new(&self.repo_path).context("Failed to initialize git commands")?;

        // Update recent repos
        let repo_str = git.repo_path().to_string_lossy().to_string();
        self.config.app_state.add_recent_repo(&repo_str);
        let _ = self.config.save_state();

        let debug = self.config.debug;

        // Pass `-f` into Gui::new so the initial model stream loads filtered
        // commits immediately (no wait for full refresh + second git log).
        let mut gui = Gui::new(self.config, git, self.filter_path)?;
        gui.run()?;

        if let Some(path) = gui.take_pending_repo_open() {
            return relaunch_repo(&path, debug);
        }

        Ok(())
    }
}
