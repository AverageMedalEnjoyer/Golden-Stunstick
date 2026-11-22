//! studiomdl runner and compile-output analysis.

use crate::formats::qc::{Issue, Severity};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

pub enum CompileMsg {
    Line(String),
    Finished { success: bool, exit_code: Option<i32> },
}

#[derive(Clone, Debug)]
pub struct CompileJob {
    pub studiomdl: PathBuf,
    pub game_dir: PathBuf,
    pub qc: PathBuf,
    pub extra_args: Vec<String>,
}

impl CompileJob {
    pub fn command_line(&self) -> Vec<String> {
        let mut v = vec!["-game".to_string(), self.game_dir.display().to_string(), "-nop4".into()];
        v.extend(self.extra_args.iter().cloned());
        v.push(self.qc.display().to_string());
        v
    }
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// Run studiomdl, streaming every output line to `tx`. Blocks until done.
pub fn run(job: &CompileJob, tx: &Sender<CompileMsg>) {
    let mut cmd = Command::new(&job.studiomdl);
    cmd.args(job.command_line());
    if let Some(dir) = job.qc.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    hide_window(&mut cmd);
    let _ = tx.send(CompileMsg::Line(format!("> \"{}\" {}", job.studiomdl.display(), job.command_line().iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" "))));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(CompileMsg::Line(format!("ERROR: could not start studiomdl: {e}")));
            let _ = tx.send(CompileMsg::Finished { success: false, exit_code: None });
            return;
        }
    };
    let stderr = child.stderr.take();
    let tx2 = tx.clone();
    let err_thread = std::thread::spawn(move || {
        if let Some(e) = stderr {
            for line in BufReader::new(e).split(b'\n').map_while(Result::ok) {
                let _ = tx2.send(CompileMsg::Line(String::from_utf8_lossy(&line).trim_end().to_string()));
            }
        }
    });
    let mut saw_error = false;
    let mut saw_complete = false;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            let l = String::from_utf8_lossy(&line).trim_end().to_string();
            if l.contains("ERROR") {
                saw_error = true;
            }
            if l.contains("Completed \"") || l.starts_with("Completed") {
                saw_complete = true;
            }
            let _ = tx.send(CompileMsg::Line(l));
        }
    }
    let _ = err_thread.join();
    let status = child.wait().ok();
    let code = status.and_then(|s| s.code());
    let success = code == Some(0) && !saw_error && (saw_complete || code == Some(0));
    let _ = tx.send(CompileMsg::Finished { success, exit_code: code });
}

/// Extract `path(line)` from a studiomdl message.
fn locate(line: &str) -> Option<(PathBuf, usize)> {
    let lower = line.to_ascii_lowercase();
    for ext in [".qc(", ".qci(", ".smd(", ".vrd(", ".vta(", ".dmx("] {
        if let Some(i) = lower.find(ext) {
            let end_path = i + ext.len() - 1;
            let rest = &line[end_path + 1..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let n: usize = num.parse().ok()?;
            // walk back to start of the path
            let head = &line[..end_path];
            let start = head.rfind(": ").map(|p| p + 2).or_else(|| head.rfind("ERROR:").map(|p| p + 6)).unwrap_or(0);
            let path = head[start..].trim().trim_matches('"').trim_matches('\'');
            return Some((PathBuf::from(path), n));
        }
    }
    None
}

struct Rule {
    needles: &'static [&'static str],
    short: &'static str,
    fix: &'static str,
}

const RULES: &[Rule] = &[
    Rule { needles: &["too many bones"], short: "Too many bones (studiomdl limit is 128 per model / 53 per mesh batch)", fix: "Remove unused bones, merge helper bones, or split the model into several models joined with bonemerge." },
    Rule { needles: &["bad command", "unknown command"], short: "Unknown QC command", fix: "Check the command's spelling against the Valve Developer Wiki. Commands must start with '$' and options must be inside the right { } block." },
    Rule { needles: &["unknown bone", "couldn't find bone", "could not find bone", "unknown bonename", "can't find bone", "bone not found"], short: "A bone named in the QC does not exist in the meshes", fix: "Check the bone's spelling (it is case sensitive in some commands) and that the SMD that defines the bone is loaded before this line." },
    Rule { needles: &["could not load file", "can't open", "cannot open", "unable to open", "failed to open", "file not found", "couldn't open", "could not open"], short: "A file referenced by the QC could not be opened", fix: "Make sure the path is relative to the QC's folder, the file exists, and the extension is right (.smd / .dmx / .vta / .qci)." },
    Rule { needles: &["too many verts", "too many vertices", "vertex limit", "max verts"], short: "Mesh has too many vertices", fix: "Split the mesh into several meshes/materials or bodygroups (limit is 65535 vertices per mesh, 32k per strip group on older branches)." },
    Rule { needles: &["material", "not found"], short: "Material not found", fix: "Check $cdmaterials points at the folder that contains the .vmt, and that the material name in the SMD matches the VMT file name." },
    Rule { needles: &["unexpected end of file", "end of file", "eof"], short: "Unexpected end of file (usually a missing '}' or quote)", fix: "Count the { and } in the QC/QCI; every block opened with '{' must be closed with '}'. Also check for a missing closing quote." },
    Rule { needles: &["expecting '{'", "expected '{'", "missing '{'"], short: "Missing '{'", fix: "This command needs a { } block. Put '{' after the command's arguments." },
    Rule { needles: &["duplicate sequence", "sequence name", "already exists"], short: "Duplicate name", fix: "Every $sequence/$animation must have a unique name. Rename one of them." },
    Rule { needles: &["bone weight", "weight"], short: "Bad vertex bone weights", fix: "Normalize the weights in your 3D package and limit vertices to at most 3 bone influences." },
    Rule { needles: &["no triangles", "0 triangles", "empty mesh"], short: "Mesh contains no triangles", fix: "Re-export the SMD with the mesh selected/visible, or remove the empty $body / studio line." },
    Rule { needles: &["too many textures", "too many materials", "max materials"], short: "Too many materials", fix: "Reduce the number of unique materials (combine textures into an atlas)." },
    Rule { needles: &["$modelname"], short: "Problem with $modelname", fix: "$modelname must be set once, with a path relative to models/ ending in .mdl, e.g.  $modelname \"props/thing.mdl\"" },
    Rule { needles: &["gameinfo", "-game"], short: "studiomdl could not find the game", fix: "Check the session's gameinfo.txt path; studiomdl is run with -game <folder that contains gameinfo.txt>." },
    Rule { needles: &["$includemodel", "include model"], short: "Included model problem", fix: "Make sure every $includemodel exists in the mounted game (e.g. models/player/heavy_animations.mdl) and has the same skeleton." },
    Rule { needles: &["collision", "convex", "phy"], short: "Collision model problem", fix: "Keep collision meshes convex per piece, under 20 pieces (or use $maxconvexpieces), and give every piece a bone. Use $concave for multi-piece props." },
    Rule { needles: &["frame", "out of range"], short: "Frame number out of range", fix: "An event, ikrule or option references a frame past the end of the animation. Use a frame between 0 and (number of frames - 1)." },
];

pub fn suggest(line: &str) -> Option<(&'static str, &'static str)> {
    let l = line.to_ascii_lowercase();
    for r in RULES {
        let all_needed = r.needles == ["material", "not found"];
        let hit = if all_needed { r.needles.iter().all(|n| l.contains(n)) } else { r.needles.iter().any(|n| l.contains(n)) };
        if hit {
            return Some((r.short, r.fix));
        }
    }
    None
}

/// Turn studiomdl output into a short list of issues.
pub fn analyze_output(lines: &[String], qc: &Path) -> Vec<Issue> {
    let mut out: Vec<Issue> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let l = line.trim();
        let upper = l.to_ascii_uppercase();
        let sev = if upper.contains("ERROR") {
            Severity::Error
        } else if upper.starts_with("WARNING") || upper.contains("WARNING:") {
            Severity::Warning
        } else {
            continue;
        };
        if upper.contains("ABORTED PROCESSING") {
            continue;
        }
        let (file, ln) = locate(l).unwrap_or_else(|| (qc.to_path_buf(), 0));
        let mut message = l.trim_start_matches("ERROR:").trim_start_matches("WARNING:").trim().to_string();
        if let Some(p) = message.find("): - ") {
            message = message[p + 5..].to_string();
        }
        // include the next line when studiomdl splits the message
        if message.len() < 8 {
            if let Some(n) = lines.get(i + 1) {
                message.push(' ');
                message.push_str(n.trim());
            }
        }
        let (short, fix) = suggest(l).map(|(s, f)| (s.to_string(), f.to_string())).unwrap_or_else(|| (String::new(), "Read the full message in the log; studiomdl stops at the first fatal error, so fix this one and compile again.".into()));
        let message = if short.is_empty() { message } else { format!("{short}: {message}") };
        if out.iter().any(|o| o.message == message && o.line == ln) {
            continue;
        }
        out.push(Issue { severity: sev, file, line: ln, message, suggestion: fix });
    }
    out
}

/// Append an error report to `<logs>/compileerror_MMDDYYYY.txt`. Returns the file path.
pub fn write_error_log(logs_dir: &Path, job: &CompileJob, lines: &[String], issues: &[Issue]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(logs_dir)?;
    let now = chrono::Local::now();
    let path = logs_dir.join(format!("compileerror_{}.txt", now.format("%m%d%Y")));
    let mut s = String::new();
    s.push_str(&format!("==== Golden Stunstick compile error - {} ====\n", now.format("%Y-%m-%d %H:%M:%S")));
    s.push_str(&format!("QC:        {}\nstudiomdl: {}\ngame:      {}\n\n", job.qc.display(), job.studiomdl.display(), job.game_dir.display()));
    s.push_str("---- Summary ----\n");
    for i in issues {
        let sev = match i.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Info => "INFO",
        };
        if i.line > 0 {
            s.push_str(&format!("[{sev}] {}:{} - {}\n    fix: {}\n", i.file.display(), i.line, i.message, i.suggestion));
        } else {
            s.push_str(&format!("[{sev}] {}\n    fix: {}\n", i.message, i.suggestion));
        }
    }
    s.push_str("\n---- Full studiomdl output ----\n");
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s.push('\n');
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(s.as_bytes())?;
    Ok(path)
}

/// Create a throw-away game folder whose gameinfo.txt mounts the same content as `fs`,
/// so studiomdl can compile without writing into the real game.
pub fn make_temp_game(dir: &Path, fs: Option<&crate::gamefs::GameFs>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut paths = String::new();
    if let Some(fs) = fs {
        for s in &fs.sources {
            let p = match &s.kind {
                crate::gamefs::SourceKind::Dir(d) => d.display().to_string(),
                crate::gamefs::SourceKind::Vpk(v) => {
                    let d = v.dir_path.display().to_string();
                    d.trim_end_matches("_dir.vpk").to_string() + ".vpk"
                }
            };
            paths.push_str(&format!("\t\t\tgame\t\"{}\"\n", p.replace('\\', "/")));
        }
    }
    let text = format!(
        "\"GameInfo\"\n{{\n\tgame \"Golden Stunstick verify\"\n\ttype multiplayer_only\n\tFileSystem\n\t{{\n\t\tSteamAppId 243750\n\t\tSearchPaths\n\t\t{{\n\t\t\tgame+mod+mod_write+game_write+default_write_path |gameinfo_path|.\n{paths}\t\t}}\n\t}}\n}}\n"
    );
    std::fs::write(dir.join("gameinfo.txt"), text)
}
