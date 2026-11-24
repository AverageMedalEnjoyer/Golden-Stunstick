//! Sessions: a named folder with a `session_settings.txt` pointing at the
//! game's gameinfo.txt and (optionally) studiomdl.exe.

use std::path::{Path, PathBuf};

pub const SETTINGS_FILE: &str = "session_settings.txt";
/// How many previously used paths each recent list keeps.
pub const MAX_RECENT: usize = 10;

#[derive(Clone, Debug, Default)]
pub struct Session {
    pub name: String,
    pub dir: PathBuf,
    pub gameinfo: PathBuf,
    pub studiomdl: Option<PathBuf>,
    /// Previously used gameinfo.txt paths, most recent first (never includes the current one).
    pub recent_gameinfo: Vec<PathBuf>,
    /// Previously used studiomdl.exe paths, most recent first (never includes the current one).
    pub recent_studiomdl: Vec<PathBuf>,
}

/// Put `p` at the front of a recent list (case-insensitive dedupe, Windows paths), capped.
pub fn push_recent(list: &mut Vec<PathBuf>, p: PathBuf) {
    let key = p.to_string_lossy().to_lowercase();
    list.retain(|q| q.to_string_lossy().to_lowercase() != key);
    list.insert(0, p);
    list.truncate(MAX_RECENT);
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

impl Session {
    /// Switch to a new gameinfo.txt, remembering the old one in the recent list.
    pub fn set_gameinfo(&mut self, p: PathBuf) {
        let old = std::mem::replace(&mut self.gameinfo, p);
        if !same_path(&old, &self.gameinfo) {
            push_recent(&mut self.recent_gameinfo, old);
        }
        let cur = self.gameinfo.clone();
        self.recent_gameinfo.retain(|q| !same_path(q, &cur));
    }

    /// Switch to a new studiomdl.exe, remembering the old one in the recent list.
    pub fn set_studiomdl(&mut self, p: PathBuf) {
        if let Some(old) = self.studiomdl.replace(p) {
            if !same_path(&old, self.studiomdl.as_deref().unwrap()) {
                push_recent(&mut self.recent_studiomdl, old);
            }
        }
        let cur = self.studiomdl.clone().unwrap();
        self.recent_studiomdl.retain(|q| !same_path(q, &cur));
    }

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
        // previously used paths (right-click the path in the session header to switch back)
        for p in &self.recent_gameinfo {
            s.push_str(&format!("recent_gameinfo = {}\n", p.display()));
        }
        for p in &self.recent_studiomdl {
            s.push_str(&format!("recent_studiomdl = {}\n", p.display()));
        }
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
        let mut recent_gameinfo: Vec<PathBuf> = Vec::new();
        let mut recent_studiomdl: Vec<PathBuf> = Vec::new();
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
                "recent_gameinfo" if !v.is_empty() && recent_gameinfo.len() < MAX_RECENT => recent_gameinfo.push(PathBuf::from(v)),
                "recent_studiomdl" if !v.is_empty() && recent_studiomdl.len() < MAX_RECENT => recent_studiomdl.push(PathBuf::from(v)),
                _ => {}
            }
        }
        let gameinfo = gameinfo.ok_or_else(|| format!("{} has no 'gameinfo = ...' line", file.display()))?;
        recent_gameinfo.retain(|p| !same_path(p, &gameinfo));
        if let Some(cur) = &studiomdl {
            recent_studiomdl.retain(|p| !same_path(p, cur));
        }
        Ok(Session { name: name.to_string(), dir: dir.to_path_buf(), gameinfo, studiomdl, recent_gameinfo, recent_studiomdl })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_paths_round_trip() {
        let dir = std::env::temp_dir().join(format!("gs_session_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session { name: "t".into(), dir: dir.clone(), gameinfo: PathBuf::from(r"C:\a\gameinfo.txt"), studiomdl: None, ..Default::default() };
        s.set_studiomdl(PathBuf::from(r"C:\x\studiomdl.exe")); // first one: nothing to remember
        assert!(s.recent_studiomdl.is_empty());
        s.set_studiomdl(PathBuf::from(r"C:\y\studiomdl.exe"));
        s.set_gameinfo(PathBuf::from(r"C:\b\gameinfo.txt"));
        s.set_gameinfo(PathBuf::from(r"C:\A\GAMEINFO.TXT")); // back to a (case differs): b remembered, a not duplicated
        assert_eq!(s.recent_gameinfo, vec![PathBuf::from(r"C:\b\gameinfo.txt")]);
        assert_eq!(s.recent_studiomdl, vec![PathBuf::from(r"C:\x\studiomdl.exe")]);
        s.save().unwrap();
        let l = Session::load("t", &dir).unwrap();
        assert_eq!(l.recent_gameinfo, s.recent_gameinfo);
        assert_eq!(l.recent_studiomdl, s.recent_studiomdl);
        assert_eq!(l.studiomdl, s.studiomdl);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_list_is_capped() {
        let mut v = Vec::new();
        for i in 0..15 {
            push_recent(&mut v, PathBuf::from(format!(r"C:\{i}\gameinfo.txt")));
        }
        assert_eq!(v.len(), MAX_RECENT);
        assert_eq!(v[0], PathBuf::from(r"C:\14\gameinfo.txt"));
    }
}
