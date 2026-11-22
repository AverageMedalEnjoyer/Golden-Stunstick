//! The `goldenstunstick_files` folder next to the EXE.
//!
//! The folder is created once at startup and re-created (cheaply) whenever it
//! is found missing. `ensure()` only touches the disk when something is
//! actually missing, so calling it often is free of side effects and leaks.

use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const ROOT_NAME: &str = "goldenstunstick_files";

pub struct FilesDir {
    pub root: PathBuf,
    last_check: Option<Instant>,
}

impl FilesDir {
    pub fn new() -> FilesDir {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."));
        let mut f = FilesDir { root: exe_dir.join(ROOT_NAME), last_check: None };
        let _ = f.ensure();
        f
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.txt")
    }

    /// Create the folder tree if (and only if) it is missing.
    pub fn ensure(&mut self) -> std::io::Result<()> {
        self.last_check = Some(Instant::now());
        let sessions = self.sessions_dir();
        if !sessions.is_dir() {
            std::fs::create_dir_all(&sessions)?;
        }
        Ok(())
    }

    /// Re-check at most every few seconds (called every frame).
    pub fn periodic_check(&mut self) {
        let due = self.last_check.map(|t| t.elapsed() > Duration::from_secs(3)).unwrap_or(true);
        if due {
            let _ = self.ensure();
        }
    }

    /// Folder of a session, e.g. `goldenstunstick_files/sessions/newgssession`.
    pub fn session_dir(&self, name: &str) -> PathBuf {
        self.sessions_dir().join(name)
    }

    pub fn list_sessions(&mut self) -> Vec<String> {
        let _ = self.ensure();
        let mut v: Vec<String> = std::fs::read_dir(self.sessions_dir())
            .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default();
        v.sort_by_key(|s| s.to_ascii_lowercase());
        v
    }
}

/// Characters not allowed in a session (folder) name.
pub fn valid_session_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Please enter a session name.".into());
    }
    if n.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Err("Session names cannot contain \\ / : * ? \" < > |".into());
    }
    if n == "." || n == ".." || n.ends_with('.') {
        return Err("Session names cannot end with a dot.".into());
    }
    Ok(n.to_string())
}
