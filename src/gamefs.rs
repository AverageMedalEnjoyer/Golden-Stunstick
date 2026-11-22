//! The mounted game filesystem built from a session's gameinfo.txt.
//!
//! Search paths are mounted in the exact order gameinfo.txt lists them and
//! lookups always return the FIRST source that has the file — top of the
//! list wins, just like the engine.

use crate::formats::keyvalues;
use crate::formats::vpk::{self, Vpk};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub enum SourceKind {
    Dir(PathBuf),
    Vpk(Vpk),
}

pub struct MountSource {
    /// What gameinfo.txt said (after macro expansion).
    pub label: String,
    pub kind: SourceKind,
}

impl MountSource {
    pub fn short_name(&self) -> String {
        match &self.kind {
            SourceKind::Vpk(v) => {
                let parent = v.dir_path.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                format!("{parent}/{}", v.display_name())
            }
            SourceKind::Dir(p) => {
                let comps: Vec<String> = p.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
                let mut s: Vec<String> = comps.into_iter().rev().collect();
                if s.is_empty() {
                    s.push(p.display().to_string());
                }
                format!("{}/ (folder)", s.join("/"))
            }
        }
    }
    pub fn is_vpk(&self) -> bool {
        matches!(self.kind, SourceKind::Vpk(_))
    }
}

#[derive(Clone, Debug)]
pub struct MountReportLine {
    pub raw: String,
    pub resolved: String,
    pub ok: bool,
    pub note: String,
}

/// A model found in the mounted filesystem.
#[derive(Clone, Debug)]
pub struct ModelEntry {
    /// normalized relative path, e.g. `models/player/heavy.mdl`
    pub path: String,
    /// index of the source that wins (first in search order)
    pub source: usize,
    /// other sources that also contain this path (shadowed)
    pub shadowed: Vec<usize>,
}

pub struct GameFs {
    pub gameinfo: PathBuf,
    pub game_dir: PathBuf,
    pub engine_root: PathBuf,
    pub game_name: String,
    pub sources: Vec<MountSource>,
    pub report: Vec<MountReportLine>,
}

fn steam_app_folder(appid: u32) -> Option<&'static str> {
    Some(match appid {
        243750 => "Source SDK Base 2013 Multiplayer",
        243730 => "Source SDK Base 2013 Singleplayer",
        218 => "Source SDK Base 2007",
        215 => "Source SDK Base",
        440 => "Team Fortress 2",
        220 => "Half-Life 2",
        240 => "Counter-Strike Source",
        300 => "Day of Defeat Source",
        320 => "Half-Life 2 Deathmatch",
        500 => "Left 4 Dead",
        550 => "Left 4 Dead 2",
        620 => "Portal 2",
        400 => "Portal",
        _ => return None,
    })
}

/// Lexically normalize `a/b/../c` -> `a/c` without touching the disk.
pub fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Guess `|all_source_engine_paths|` for a gameinfo.txt.
pub fn guess_engine_root(gameinfo: &Path, appid: Option<u32>) -> PathBuf {
    let game_dir = gameinfo.parent().unwrap_or(Path::new("."));
    let parent = game_dir.parent().unwrap_or(game_dir);
    let parent_name = parent.file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if parent_name == "sourcemods" {
        // steamapps/sourcemods/<mod>/gameinfo.txt -> steamapps/common/<engine>
        if let (Some(steamapps), Some(folder)) = (parent.parent(), appid.and_then(steam_app_folder)) {
            return steamapps.join("common").join(folder);
        }
    }
    parent.to_path_buf()
}

impl GameFs {
    pub fn read_appid(gameinfo: &Path) -> Option<u32> {
        let text = std::fs::read_to_string(gameinfo).ok()?;
        let kv = keyvalues::parse_root(&text)?;
        kv.path(&["FileSystem", "SteamAppId"]).and_then(|k| k.str()).and_then(|s| s.trim().parse().ok())
    }

    /// Mount everything gameinfo.txt lists. `engine_override` replaces the
    /// automatically guessed `|all_source_engine_paths|`.
    pub fn mount(gameinfo: &Path, engine_override: Option<&Path>) -> Result<GameFs, String> {
        let text = std::fs::read_to_string(gameinfo).map_err(|e| format!("Could not read {}: {e}", gameinfo.display()))?;
        let kv = keyvalues::parse_root(&text).ok_or_else(|| format!("{} is empty or not a KeyValues file", gameinfo.display()))?;
        let game_name = kv.get_str("game").unwrap_or("Unknown game").to_string();
        let game_dir = gameinfo.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let appid = kv.path(&["FileSystem", "SteamAppId"]).and_then(|k| k.str()).and_then(|s| s.trim().parse().ok());
        let engine_root = engine_override.map(|p| p.to_path_buf()).unwrap_or_else(|| guess_engine_root(gameinfo, appid));
        let mut fs = GameFs { gameinfo: gameinfo.to_path_buf(), game_dir: game_dir.clone(), engine_root: engine_root.clone(), game_name, sources: Vec::new(), report: Vec::new() };

        let Some(sp) = kv.path(&["FileSystem", "SearchPaths"]) else {
            return Err("gameinfo.txt has no FileSystem/SearchPaths block".into());
        };
        let mut seen: HashMap<PathBuf, ()> = HashMap::new();
        for entry in sp.children() {
            let ids: Vec<String> = entry.key.split('+').map(|s| s.trim().to_ascii_lowercase()).collect();
            if !ids.iter().any(|i| i == "game") {
                continue;
            }
            let Some(raw) = entry.str() else { continue };
            let raw = raw.trim().to_string();
            let mut rel = raw.clone();
            let base: PathBuf;
            let lower = rel.to_ascii_lowercase();
            if lower.starts_with("|gameinfo_path|") {
                base = game_dir.clone();
                rel = rel["|gameinfo_path|".len()..].to_string();
            } else if lower.starts_with("|all_source_engine_paths|") {
                base = engine_root.clone();
                rel = rel["|all_source_engine_paths|".len()..].to_string();
            } else if Path::new(&rel).is_absolute() {
                base = PathBuf::new();
            } else {
                base = engine_root.clone();
            }
            let joined = lexical_normalize(&base.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)));
            let joined_str = joined.to_string_lossy().to_string();

            // wildcard: custom/*
            if joined_str.ends_with('*') {
                let dir = PathBuf::from(joined_str.trim_end_matches('*').trim_end_matches(['/', '\\']));
                let mut items: Vec<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
                items.sort();
                let mut n = 0;
                for it in items {
                    let name = it.file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                    if it.is_dir() {
                        fs.add_dir(&raw, it, &mut seen);
                        n += 1;
                    } else if name.ends_with("_dir.vpk") || (name.ends_with(".vpk") && !name.chars().rev().skip(4).take(3).all(|c| c.is_ascii_digit())) {
                        fs.add_vpk(&raw, it, &mut seen);
                        n += 1;
                    }
                }
                fs.report.push(MountReportLine { raw: raw.clone(), resolved: dir.display().to_string(), ok: true, note: format!("wildcard expanded to {n} item(s)") });
                continue;
            }

            if joined_str.to_ascii_lowercase().ends_with(".vpk") {
                fs.add_vpk(&raw, joined, &mut seen);
            } else {
                fs.add_dir(&raw, joined, &mut seen);
            }
        }
        if fs.sources.is_empty() {
            return Err("No 'game' search paths in gameinfo.txt could be mounted".into());
        }
        Ok(fs)
    }

    fn add_dir(&mut self, raw: &str, p: PathBuf, seen: &mut HashMap<PathBuf, ()>) {
        if seen.contains_key(&p) {
            return;
        }
        seen.insert(p.clone(), ());
        if p.is_dir() {
            self.report.push(MountReportLine { raw: raw.into(), resolved: p.display().to_string(), ok: true, note: "folder".into() });
            self.sources.push(MountSource { label: raw.into(), kind: SourceKind::Dir(p) });
        } else {
            self.report.push(MountReportLine { raw: raw.into(), resolved: p.display().to_string(), ok: false, note: "folder does not exist (skipped)".into() });
        }
    }

    fn add_vpk(&mut self, raw: &str, p: PathBuf, seen: &mut HashMap<PathBuf, ()>) {
        if seen.contains_key(&p) {
            return;
        }
        seen.insert(p.clone(), ());
        match Vpk::open(&p) {
            Ok(v) => {
                self.report.push(MountReportLine { raw: raw.into(), resolved: v.dir_path.display().to_string(), ok: true, note: format!("vpk, {} files", v.entries.len()) });
                self.sources.push(MountSource { label: raw.into(), kind: SourceKind::Vpk(v) });
            }
            Err(e) => {
                self.report.push(MountReportLine { raw: raw.into(), resolved: p.display().to_string(), ok: false, note: format!("not mounted: {e}") });
            }
        }
    }

    /// Index of the first source containing `path` (top of the search list wins).
    pub fn find(&self, path: &str) -> Option<usize> {
        let n = vpk::normalize(path);
        for (i, s) in self.sources.iter().enumerate() {
            let hit = match &s.kind {
                SourceKind::Vpk(v) => v.entries.contains_key(&n),
                SourceKind::Dir(d) => d.join(&n).is_file(),
            };
            if hit {
                return Some(i);
            }
        }
        None
    }

    pub fn exists(&self, path: &str) -> bool {
        self.find(path).is_some()
    }

    pub fn read_from(&self, source: usize, path: &str) -> Option<Vec<u8>> {
        let n = vpk::normalize(path);
        match &self.sources.get(source)?.kind {
            SourceKind::Vpk(v) => v.read(&n).ok(),
            SourceKind::Dir(d) => std::fs::read(d.join(&n)).ok(),
        }
    }

    /// Read the first match for `path`.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let s = self.find(path)?;
        self.read_from(s, path)
    }

    /// Absolute disk path if the winning source is a folder.
    pub fn disk_path(&self, path: &str) -> Option<PathBuf> {
        let s = self.find(path)?;
        match &self.sources[s].kind {
            SourceKind::Dir(d) => Some(d.join(vpk::normalize(path))),
            _ => None,
        }
    }

    /// All `.mdl` files across every source, deduplicated so the first source wins.
    pub fn list_models(&self) -> Vec<ModelEntry> {
        let mut map: HashMap<String, usize> = HashMap::new();
        let mut out: Vec<ModelEntry> = Vec::new();
        for (i, s) in self.sources.iter().enumerate() {
            let mut paths: Vec<String> = Vec::new();
            match &s.kind {
                SourceKind::Vpk(v) => {
                    for k in v.entries.keys() {
                        if k.ends_with(".mdl") {
                            paths.push(k.clone());
                        }
                    }
                }
                SourceKind::Dir(d) => {
                    walk_mdl(d, &d.join("models"), &mut paths, 0);
                }
            }
            for p in paths {
                if let Some(&idx) = map.get(&p) {
                    out[idx].shadowed.push(i);
                } else {
                    map.insert(p.clone(), out.len());
                    out.push(ModelEntry { path: p, source: i, shadowed: Vec::new() });
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Find a material: tries every `$cdmaterials` directory in order.
    pub fn find_material(&self, cd_dirs: &[String], name: &str) -> Option<String> {
        let name = name.replace('\\', "/");
        let name = name.trim_end_matches(".vmt");
        let mut tried: Vec<String> = Vec::new();
        for cd in cd_dirs {
            let cd = cd.replace('\\', "/");
            let cd = cd.trim_matches('/');
            let p = if cd.is_empty() { format!("materials/{name}.vmt") } else { format!("materials/{cd}/{name}.vmt") };
            if self.exists(&p) {
                return Some(p);
            }
            tried.push(p);
        }
        let p = format!("materials/{name}.vmt");
        if self.exists(&p) {
            return Some(p);
        }
        None
    }
}

fn walk_mdl(root: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 24 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk_mdl(root, &p, out, depth + 1);
        } else if p.extension().map(|x| x.eq_ignore_ascii_case("mdl")).unwrap_or(false) {
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(vpk::normalize(&rel.to_string_lossy()));
            }
        }
    }
}
