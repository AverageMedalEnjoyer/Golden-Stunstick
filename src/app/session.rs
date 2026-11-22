//! Sessions: a named folder with a `session_settings.txt` pointing at the
//! game's gameinfo.txt and (optionally) studiomdl.exe.

use std::path::{Path, PathBuf};

pub const SETTINGS_FILE: &str = "session_settings.txt";

#[derive(Clone, Debug)]
pub struct Session {
    pub name: String,
    pub dir: PathBuf,
    pub gameinfo: PathBuf,
    pub studiomdl: Option<PathBuf>,
}

impl Session {
    pub fn settings_path(&self) -> PathBuf {
        self.dir.join(SETTINGS_FILE)
    }
    pub fn animations_dir(&self) -> PathBuf {
        self.dir.join("animations")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }
    pub fn game_dir(&self) -> PathBuf {
        self.gameinfo.parent().map(|p| p.to_path_buf()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut s = String::new();
        s.push_str("// Golden Stunstick session settings\n");
        s.push_str(&format!("gameinfo = {}\n", self.gameinfo.display()));
        s.push_str(&format!("studiomdl = {}\n", self.studiomdl.as_ref().map(|p| p.display().to_string()).unwrap_or_default()));
        std::fs::write(self.settings_path(), s)
    }

    /// Load `<dir>/session_settings.txt`. Errors name the missing file.
    pub fn load(name: &str, dir: &Path) -> Result<Session, String> {
        let file = dir.join(SETTINGS_FILE);
        if !file.is_file() {
            return Err(format!("{SETTINGS_FILE} is missing from\n{}", dir.display()));
        }
        let text = std::fs::read_to_string(&file).map_err(|e| format!("Could not read {}: {e}", file.display()))?;
        // tolerate a UTF-8 byte order mark (Notepad / PowerShell can add one)
        let text = text.trim_start_matches('\u{feff}');
        let mut gameinfo = None;
        let mut studiomdl = None;
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with("//") || l.is_empty() {
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.trim().trim_matches('"');
            match k.trim().to_ascii_lowercase().as_str() {
                "gameinfo" if !v.is_empty() => gameinfo = Some(PathBuf::from(v)),
                "studiomdl" if !v.is_empty() => studiomdl = Some(PathBuf::from(v)),
                _ => {}
            }
        }
        let gameinfo = gameinfo.ok_or_else(|| format!("{} has no 'gameinfo = ...' line", file.display()))?;
        Ok(Session { name: name.to_string(), dir: dir.to_path_buf(), gameinfo, studiomdl })
    }
}

/// Look for studiomdl.exe near a gameinfo.txt: `<engine>/bin/studiomdl.exe` (and x64).
pub fn guess_studiomdl(gameinfo: &Path) -> Option<PathBuf> {
    let appid = crate::gamefs::GameFs::read_appid(gameinfo);
    let mut roots = vec![crate::gamefs::guess_engine_root(gameinfo, appid)];
    if let Some(p) = gameinfo.parent().and_then(|p| p.parent()) {
        roots.push(p.to_path_buf());
    }
    for r in roots {
        for rel in ["bin/x64/studiomdl.exe", "bin/studiomdl.exe"] {
            let p = r.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}
