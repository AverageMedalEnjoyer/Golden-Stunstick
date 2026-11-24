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
    /// true: every needle must appear; false: any one of them is enough.
    all: bool,
    short: &'static str,
    fix: &'static str,
}

const RULES: &[Rule] = &[
    Rule { needles: &["too many bones"], all: false, short: "Too many bones (studiomdl limit is 128 per model / 53 per mesh batch)", fix: "Remove unused bones, merge helper bones, or split the model into several models joined with bonemerge." },
    Rule { needles: &["bad command", "unknown command"], all: false, short: "Unknown QC command", fix: "Check the command's spelling against the Valve Developer Wiki. Commands must start with '$' and options must be inside the right { } block." },
    Rule { needles: &["unknown bone", "couldn't find bone", "could not find bone", "unknown bonename", "can't find bone", "bone not found"], all: false, short: "A bone named in the QC does not exist in the meshes", fix: "Check the bone's spelling (it is case sensitive in some commands) and that the SMD that defines the bone is loaded before this line." },
    Rule { needles: &["could not load file", "can't open", "cannot open", "unable to open", "failed to open", "file not found", "couldn't open", "could not open"], all: false, short: "A file referenced by the QC could not be opened", fix: "Make sure the path is relative to the QC's folder, the file exists, and the extension is right (.smd / .dmx / .vta / .qci)." },
    Rule { needles: &["too many verts", "too many vertices", "vertex limit", "max verts"], all: false, short: "Mesh has too many vertices", fix: "Split the mesh into several meshes/materials or bodygroups (limit is 65535 vertices per mesh, 32k per strip group on older branches)." },
    Rule { needles: &["material", "not found"], all: true, short: "Material not found", fix: "Check $cdmaterials points at the folder that contains the .vmt, and that the material name in the SMD matches the VMT file name." },
    Rule { needles: &["unexpected end of file", "end of file", "eof"], all: false, short: "Unexpected end of file (usually a missing '}' or quote)", fix: "Count the { and } in the QC/QCI; every block opened with '{' must be closed with '}'. Also check for a missing closing quote." },
    Rule { needles: &["expecting '{'", "expected '{'", "missing '{'"], all: false, short: "Missing '{'", fix: "This command needs a { } block. Put '{' after the command's arguments." },
    Rule { needles: &["duplicate sequence", "sequence name", "already exists"], all: false, short: "Duplicate name", fix: "Every $sequence/$animation must have a unique name. Rename one of them." },
    Rule { needles: &["bone weight", "weight"], all: false, short: "Bad vertex bone weights", fix: "Normalize the weights in your 3D package and limit vertices to at most 3 bone influences." },
    Rule { needles: &["no triangles", "0 triangles", "empty mesh"], all: false, short: "Mesh contains no triangles", fix: "Re-export the SMD with the mesh selected/visible, or remove the empty $body / studio line." },
    Rule { needles: &["too many textures", "too many materials", "max materials"], all: false, short: "Too many materials", fix: "Reduce the number of unique materials (combine textures into an atlas)." },
    Rule { needles: &["$modelname"], all: false, short: "Problem with $modelname", fix: "$modelname must be set once, with a path relative to models/ ending in .mdl, e.g.  $modelname \"props/thing.mdl\"" },
    Rule { needles: &["gameinfo", "-game"], all: false, short: "studiomdl could not find the game", fix: "Check the session's gameinfo.txt path; studiomdl is run with -game <folder that contains gameinfo.txt>." },
    Rule { needles: &["$includemodel", "include model"], all: false, short: "Included model problem", fix: "Make sure every $includemodel exists in the mounted game (e.g. models/player/heavy_animations.mdl) and has the same skeleton." },
    Rule { needles: &["collision", "convex", "phy"], all: false, short: "Collision model problem", fix: "Keep collision meshes convex per piece, under 20 pieces (or use $maxconvexpieces), and give every piece a bone. Use $concave for multi-piece props." },
    Rule { needles: &["frame", "out of range"], all: true, short: "Frame number out of range", fix: "An event, ikrule or option references a frame past the end of the animation. Use a frame between 0 and (number of frames - 1)." },
];

/// Messages studiomdl prints that do not affect the compile. Reported as Info.
const BENIGN: &[Rule] = &[
    Rule { needles: &["p4lib.dll"], all: false, short: "Perforce library not found (harmless)", fix: "Nothing to fix: p4lib.dll is Valve's Perforce integration, and Golden Stunstick runs studiomdl with -nop4 so it is never used." },
    Rule { needles: &["leaking", "elements"], all: true, short: "DMX elements not freed (harmless)", fix: "Nothing to fix: studiomdl reports this while cleaning up, usually after an earlier error stopped the compile. Fix the errors above it." },
];

fn rule_hit(r: &Rule, lower: &str) -> bool {
    if r.all { r.needles.iter().all(|n| lower.contains(n)) } else { r.needles.iter().any(|n| lower.contains(n)) }
}

pub fn suggest(line: &str) -> Option<(&'static str, &'static str)> {
    let l = line.to_ascii_lowercase();
    RULES.iter().find(|r| rule_hit(r, &l)).map(|r| (r.short, r.fix))
}

/// studiomdl failed to launch a helper program (e.g. dmxconvert):
///
/// ```text
/// WARNING: Could not execute the command:
///    dmxconvert -allowdebug -i ... -o ...
/// Windows gave the error message:
///    "The system cannot find the file specified."
/// ```
struct ExecFailure {
    /// Index of the "Could not execute the command" line.
    at: usize,
    /// Index of the "Windows gave the error message" line, if present.
    reason_at: Option<usize>,
    /// Program name, e.g. "dmxconvert".
    tool: String,
    /// Windows' reason, without quotes.
    reason: String,
}

fn next_nonempty(lines: &[String], from: usize) -> Option<(usize, &str)> {
    lines.iter().enumerate().skip(from).map(|(i, l)| (i, l.trim())).find(|(_, l)| !l.is_empty())
}

fn find_exec_failures(lines: &[String]) -> Vec<ExecFailure> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.to_ascii_lowercase().contains("could not execute the command") {
            continue;
        }
        let Some((cmd_at, cmd)) = next_nonempty(lines, i + 1) else { continue };
        let first = cmd.split_whitespace().next().unwrap_or("").trim_matches('"');
        let tool = Path::new(first).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut reason_at = None;
        let mut reason = String::new();
        if let Some((w, wl)) = next_nonempty(lines, cmd_at + 1) {
            if wl.to_ascii_lowercase().starts_with("windows gave the error message") {
                reason_at = Some(w);
                if let Some((_, r)) = next_nonempty(lines, w + 1) {
                    reason = r.trim_matches('"').to_string();
                }
            }
        }
        out.push(ExecFailure { at: i, reason_at, tool, reason });
    }
    out
}

fn exec_failure_issue(f: &ExecFailure, qc: &Path) -> Issue {
    let exe = format!("{}.exe", f.tool);
    let not_found = f.reason.to_ascii_lowercase().contains("cannot find the file");
    let message = if not_found {
        format!("studiomdl could not find {exe}: {}", f.reason)
    } else if f.reason.is_empty() {
        format!("studiomdl could not run {exe}")
    } else {
        format!("studiomdl could not run {exe}: {}", f.reason)
    };
    let suggestion = if f.tool.eq_ignore_ascii_case("dmxconvert") {
        "studiomdl runs dmxconvert.exe to upgrade DMX files saved in an older format. It is missing from this studiomdl's bin folder (some branches, such as Left 4 Dead, don't ship it). Use a studiomdl whose bin folder has dmxconvert.exe (e.g. Left 4 Dead 2, Team Fortress 2, Source SDK Base 2013), copy a matching dmxconvert.exe next to studiomdl.exe, or re-export the DMX in a current format so no conversion is needed.".to_string()
    } else {
        format!("studiomdl looks for {exe} next to studiomdl.exe, in the QC's folder, and on PATH. Put {exe} in the same bin folder as studiomdl.exe.")
    };
    Issue { severity: Severity::Error, file: qc.to_path_buf(), line: 0, message, suggestion }
}

/// Turn studiomdl output into a short list of issues.
pub fn analyze_output(lines: &[String], qc: &Path) -> Vec<Issue> {
    let mut out: Vec<Issue> = Vec::new();
    let exec_failures = find_exec_failures(lines);
    let dmxconvert_failed = exec_failures.iter().any(|f| f.tool.eq_ignore_ascii_case("dmxconvert"));
    for (i, line) in lines.iter().enumerate() {
        if let Some(f) = exec_failures.iter().find(|f| f.at == i) {
            out.push(exec_failure_issue(f, qc));
            continue;
        }
        if exec_failures.iter().any(|f| f.reason_at == Some(i)) {
            continue;
        }
        let l = line.trim();
        let lower = l.to_ascii_lowercase();
        if let Some(b) = BENIGN.iter().find(|b| rule_hit(b, &lower)) {
            let message = l.trim_start_matches("ERROR:").trim_start_matches("WARNING:").trim();
            out.push(Issue { severity: Severity::Info, file: qc.to_path_buf(), line: 0, message: format!("{}: {message}", b.short), suggestion: b.fix.into() });
            continue;
        }
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
        let dmx_not_converted = dmxconvert_failed && lower.contains("could not load file") && lower.contains(".dmx");
        let (short, fix) = if dmx_not_converted {
            (
                "DMX could not be converted (dmxconvert.exe failed), so it could not be loaded".to_string(),
                "The DMX file is probably fine: it is in an older format and studiomdl could not run dmxconvert.exe to upgrade it. Fix the dmxconvert error above.".to_string(),
            )
        } else {
            suggest(l).map(|(s, f)| (s.to_string(), f.to_string())).unwrap_or_else(|| (String::new(), "Read the full message in the log; studiomdl stops at the first fatal error, so fix this one and compile again.".into()))
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Real Left 4 Dead studiomdl output: an old-format DMX needs dmxconvert.exe, which that branch lacks.
    const MISSING_DMXCONVERT: &str = r#"WARNING: AppFramework : Unable to load module p4lib.dll!
qdir:    "d:\models\john_xsi\"
Working on "Male_07.qc"
converting file d:\models\john_xsi\dmx\male_07_high.dmx from model 1 to model 7
WARNING: Could not execute the command:
   dmxconvert -allowdebug -i d:\models\john_xsi\dmx\male_07_high.dmx -o d:\models\john_xsi\dmx\_temp_conversion_file_.dmx -oe binary -of model
Windows gave the error message:
   "The system cannot find the file specified."
ERROR: d:\models\john_xsi\Male_07.qc(304): - could not load file 'dmx\male_07_high.dmx'
ERROR: Aborted Processing on 'player/john/john_freeman.mdl'
WARNING: Leaking 1 elements"#;

    #[test]
    fn missing_dmxconvert_is_explained() {
        let lines: Vec<String> = MISSING_DMXCONVERT.lines().map(String::from).collect();
        let issues = analyze_output(&lines, Path::new(r"d:\models\john_xsi\Male_07.qc"));
        for i in &issues {
            println!("{:?} {}:{} {}", i.severity, i.file.display(), i.line, i.message);
        }
        assert_eq!(issues.len(), 4, "p4lib info, dmxconvert error, dmx load error, leak info");

        assert_eq!(issues[0].severity, Severity::Info);
        assert!(issues[0].message.contains("Perforce"));
        assert!(!issues[0].message.contains("Frame number"));

        assert_eq!(issues[1].severity, Severity::Error);
        assert_eq!(issues[1].message, "studiomdl could not find dmxconvert.exe: The system cannot find the file specified.");
        assert!(issues[1].suggestion.contains("dmxconvert.exe"));

        assert_eq!(issues[2].severity, Severity::Error);
        assert_eq!(issues[2].line, 304);
        assert!(issues[2].message.starts_with("DMX could not be converted"));

        assert_eq!(issues[3].severity, Severity::Info);
        assert!(issues[3].message.contains("Leaking 1 elements"));
    }

    #[test]
    fn frame_rule_needs_both_words() {
        assert!(suggest("WARNING: AppFramework : Unable to load module foo.dll!").is_none());
        assert_eq!(suggest("ERROR: event frame 40 out of range").map(|s| s.0), Some("Frame number out of range"));
    }

    #[test]
    fn plain_missing_file_keeps_generic_advice() {
        let lines = vec!["ERROR: c:\\m\\a.qc(3): - could not load file 'body.smd'".to_string()];
        let issues = analyze_output(&lines, Path::new(r"c:\m\a.qc"));
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.starts_with("A file referenced by the QC could not be opened"));
    }
}
