//! QC tokenizer and lightweight parser.
//!
//! Used by the Animator (to find a model's reference meshes, materials and
//! skeleton) and by the Compiler (to lint a QC before handing it to
//! studiomdl, and to map errors back to lines).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub text: String,
    pub quoted: bool,
    pub line: usize,
    /// index into `Qc::files`
    pub file: usize,
}

impl Token {
    pub fn is(&self, s: &str) -> bool {
        !self.quoted && self.text == s
    }
}

#[derive(Clone, Debug)]
pub struct Command {
    /// lowercase, including the `$`
    pub name: String,
    pub args: Vec<Token>,
    pub line: usize,
    pub file: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub file: PathBuf,
    pub line: usize,
    pub message: String,
    pub suggestion: String,
}

#[derive(Clone, Debug, Default)]
pub struct Qc {
    pub files: Vec<PathBuf>,
    pub commands: Vec<Command>,
    pub issues: Vec<Issue>,
}

pub const KNOWN_COMMANDS: &[&str] = &[
    "$modelname", "$model", "$body", "$bodygroup", "$cdmaterials", "$cd", "$cdtexture", "$scale", "$root", "$controller", "$screenaligned",
    "$sequence", "$animation", "$declaresequence", "$declareanimation", "$weightlist", "$defaultweightlist", "$includemodel", "$poseparameter",
    "$sectionframes", "$origin", "$upaxis", "$eyeposition", "$illumposition", "$attachment", "$bbox", "$cbox", "$hbox", "$hboxset",
    "$skipboneinbbox", "$texturegroup", "$renamematerial", "$surfaceprop", "$jointsurfaceprop", "$contents", "$jointcontents",
    "$collisionmodel", "$collisionjoints", "$collisiontext", "$staticprop", "$ambientboost", "$centerbonesonverts", "$opaque",
    "$mostlyopaque", "$obsolete", "$noforcedfade", "$donotcastshadows", "$castshadows", "$lod", "$shadowlod", "$keyvalues", "$ikchain",
    "$ikautoplaylock", "$definebone", "$bonemerge", "$hierarchy", "$heirarchy", "$alwayscollapse", "$collapsebones", "$proceduralbones",
    "$jigglebone", "$externaltextures", "$pushd", "$popd", "$unlockdefinebones", "$limitrotation", "$lockbonelengths", "$realignbones",
    "$insertbone", "$definevariable", "$definemacro", "$continue", "$animblocksize", "$illumpositionattachment", "$maxeyedeflection",
    "$constantdirectionallight", "$fakevta", "$allowactivityname", "$collisionprecision", "$boneflexdriver", "$maxverts",
    "$preservetriangleorder", "$qcassert", "$lcaseallsequences", "$hgroup", "$bonesaveframe", "$calctransitions", "$minlod",
    "$allowrootlods", "$renamebone", "$skiptransition", "$forcephonemecrossfade", "$append", "$prepend", "$include", "$motionrollback",
    "$autocenter", "$cmdlist", "$checkuv", "$addsearchdir", "$nocheckuv", "$noenvmapcullface", "$subd", "$lockdefinebones",
    "$sequencegroup", "$sequencegroupsize", "$mergebones", "$skinlesshierarchy", "$noautodmxrules", "$bodygroupconstraint",
    "$model_is_wireframe", "$defaultfadein", "$defaultfadeout", "$alias", "$attachmentfromfile", "$hboxsetfromfile", "$addsearchpath",
    "$keepcolorinmaterials", "$weightlistdefault", "$stripboneprefix", "$overridesubdivision",
];

/// Tokenize QC text. Unterminated quotes and comments are reported in `issues`.
pub fn tokenize(text: &str, file: usize, file_path: &Path, issues: &mut Vec<Issue>) -> Vec<Token> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut line = 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let start_line = line;
            i += 2;
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    i += 2;
                    closed = true;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            if !closed {
                issues.push(Issue {
                    severity: Severity::Error,
                    file: file_path.to_path_buf(),
                    line: start_line,
                    message: "Block comment '/*' is never closed".into(),
                    suggestion: "Add '*/' where the comment should end, or remove the '/*'.".into(),
                });
            }
            continue;
        }
        if c == '"' {
            let start_line = line;
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '"' {
                    closed = true;
                    i += 1;
                    break;
                }
                if chars[i] == '\n' {
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            if !closed {
                issues.push(Issue {
                    severity: Severity::Error,
                    file: file_path.to_path_buf(),
                    line: start_line,
                    message: format!("Missing closing quote: \"{}", s.chars().take(40).collect::<String>()),
                    suggestion: "Add the missing '\"' at the end of the string on this line.".into(),
                });
            }
            out.push(Token { text: s, quoted: true, line: start_line, file });
            continue;
        }
        if c == '{' || c == '}' {
            out.push(Token { text: c.to_string(), quoted: false, line, file });
            i += 1;
            continue;
        }
        let mut s = String::new();
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '{' && chars[i] != '}' && chars[i] != '"' {
            if chars[i] == '/' && matches!(chars.get(i + 1), Some('/') | Some('*')) {
                break;
            }
            s.push(chars[i]);
            i += 1;
        }
        out.push(Token { text: s, quoted: false, line, file });
    }
    out
}

impl Qc {
    pub fn qc_dir(&self) -> PathBuf {
        self.files.first().and_then(|f| f.parent()).map(|p| p.to_path_buf()).unwrap_or_default()
    }

    /// Load a QC file and everything it `$include`s.
    pub fn load(path: &Path) -> Result<Qc, String> {
        let mut qc = Qc::default();
        let root_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        qc.load_file(path, &root_dir, 0)?;
        qc.check_braces();
        Ok(qc)
    }

    fn load_file(&mut self, path: &Path, root_dir: &Path, depth: usize) -> Result<(), String> {
        if depth > 16 {
            return Err(format!("$include nesting too deep at {}", path.display()));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        let fi = self.files.len();
        self.files.push(path.to_path_buf());
        let toks = tokenize(&text, fi, path, &mut self.issues);
        let mut depth_b = 0i32;
        let mut cur: Option<Command> = None;
        for t in toks {
            if t.is("{") {
                depth_b += 1;
            } else if t.is("}") {
                depth_b -= 1;
            }
            if !t.quoted && t.text.starts_with('$') && depth_b == 0 && t.text.len() > 1 && !t.text.ends_with('$') {
                if let Some(c) = cur.take() {
                    self.push_command(c, root_dir, depth)?;
                }
                cur = Some(Command { name: t.text.to_ascii_lowercase(), args: Vec::new(), line: t.line, file: fi });
                continue;
            }
            match cur.as_mut() {
                Some(c) => c.args.push(t),
                None => {
                    if !t.is("{") && !t.is("}") {
                        self.issues.push(Issue {
                            severity: Severity::Warning,
                            file: path.to_path_buf(),
                            line: t.line,
                            message: format!("Text '{}' appears before any $command", t.text),
                            suggestion: "Every QC statement must start with a $command (e.g. $modelname). Remove or comment out this text.".into(),
                        });
                    }
                }
            }
        }
        if let Some(c) = cur.take() {
            self.push_command(c, root_dir, depth)?;
        }
        Ok(())
    }

    fn push_command(&mut self, c: Command, root_dir: &Path, depth: usize) -> Result<(), String> {
        if c.name == "$include" {
            if let Some(a) = c.args.first() {
                let p = resolve(root_dir, &self.files[c.file], &a.text);
                match p {
                    Some(p) => {
                        self.commands.push(c.clone());
                        return self.load_file(&p, root_dir, depth + 1);
                    }
                    None => self.issues.push(Issue {
                        severity: Severity::Error,
                        file: self.files[c.file].clone(),
                        line: c.line,
                        message: format!("$include file not found: {}", a.text),
                        suggestion: "Check the path is relative to the QC's folder and the file exists (QCI files are usually next to the QC).".into(),
                    }),
                }
            }
        }
        self.commands.push(c);
        Ok(())
    }

    /// Re-scan every file for unbalanced braces (reports the line of the unmatched brace).
    fn check_braces(&mut self) {
        for (fi, f) in self.files.clone().iter().enumerate() {
            let Ok(text) = std::fs::read_to_string(f) else { continue };
            let mut dummy = Vec::new();
            let toks = tokenize(&text, fi, f, &mut dummy);
            let mut stack: Vec<usize> = Vec::new();
            for t in &toks {
                if t.is("{") {
                    stack.push(t.line);
                } else if t.is("}") && stack.pop().is_none() {
                    self.issues.push(Issue {
                        severity: Severity::Error,
                        file: f.clone(),
                        line: t.line,
                        message: "Unexpected '}' (no matching '{')".into(),
                        suggestion: "Remove this '}' or add the missing '{' after the command it belongs to.".into(),
                    });
                }
            }
            for l in stack {
                self.issues.push(Issue {
                    severity: Severity::Error,
                    file: f.clone(),
                    line: l,
                    message: "Missing '}' - the '{' opened here is never closed".into(),
                    suggestion: "Add a closing '}' at the end of this block.".into(),
                });
            }
        }
    }

    pub fn first(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Command> + 'a {
        self.commands.iter().filter(move |c| c.name == name)
    }

    pub fn model_name(&self) -> Option<String> {
        self.first("$modelname").and_then(|c| c.args.first()).map(|t| t.text.clone())
    }

    pub fn cdmaterials(&self) -> Vec<String> {
        let mut v = Vec::new();
        for c in self.all("$cdmaterials") {
            for a in &c.args {
                if !a.is("{") && !a.is("}") {
                    v.push(a.text.clone());
                }
            }
        }
        v
    }

    /// Mesh files used for the reference (first body / bodygroup choice) in declaration order.
    /// Returns (bodypart name, file path as written, command).
    pub fn mesh_refs(&self) -> Vec<(String, String, &Command)> {
        let mut out = Vec::new();
        for c in &self.commands {
            match c.name.as_str() {
                "$body" | "$model" => {
                    if c.args.len() >= 2 {
                        out.push((c.args[0].text.clone(), c.args[1].text.clone(), c));
                    }
                }
                "$bodygroup" => {
                    let name = c.args.first().map(|t| t.text.clone()).unwrap_or_default();
                    let mut it = c.args.iter().skip(1);
                    while let Some(t) = it.next() {
                        if t.text.eq_ignore_ascii_case("studio") && !t.quoted {
                            if let Some(f) = it.next() {
                                out.push((name.clone(), f.text.clone(), c));
                                break; // first choice is shown by default
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// All `studio` choices of every bodygroup: (group name, Vec<Option<file>>), `None` = blank.
    pub fn bodygroups(&self) -> Vec<(String, Vec<Option<String>>)> {
        let mut out = Vec::new();
        for c in &self.commands {
            match c.name.as_str() {
                "$body" | "$model" if c.args.len() >= 2 => out.push((c.args[0].text.clone(), vec![Some(c.args[1].text.clone())])),
                "$bodygroup" => {
                    let name = c.args.first().map(|t| t.text.clone()).unwrap_or_default();
                    let mut choices = Vec::new();
                    let mut it = c.args.iter().skip(1).peekable();
                    while let Some(t) = it.next() {
                        if t.quoted {
                            continue;
                        }
                        if t.text.eq_ignore_ascii_case("studio") {
                            if let Some(f) = it.next() {
                                choices.push(Some(f.text.clone()));
                            }
                        } else if t.text.eq_ignore_ascii_case("blank") {
                            choices.push(None);
                        }
                    }
                    out.push((name, choices));
                }
                _ => {}
            }
        }
        out
    }
}

/// Resolve a file referenced by a QC: relative to the referencing file, then to the root QC dir.
/// Tries the given name and then `.smd` / `.dmx` / `.qci` extensions.
pub fn resolve(root_dir: &Path, from_file: &Path, name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    let mut bases = vec![];
    if let Some(p) = from_file.parent() {
        bases.push(p.to_path_buf());
    }
    bases.push(root_dir.to_path_buf());
    let p = Path::new(&name);
    if p.is_absolute() && p.is_file() {
        return Some(p.to_path_buf());
    }
    for b in &bases {
        let full = b.join(&name);
        if full.is_file() {
            return Some(full);
        }
        if p.extension().is_none() {
            for ext in ["smd", "dmx", "qci", "vta", "fbx", "obj"] {
                let f = b.join(format!("{name}.{ext}"));
                if f.is_file() {
                    return Some(f);
                }
            }
        }
    }
    None
}

/// Static checks performed before running studiomdl.
pub fn lint(qc: &Qc) -> Vec<Issue> {
    let mut issues = qc.issues.clone();
    let root = qc.qc_dir();
    let f = |c: &Command| qc.files[c.file].clone();

    if qc.first("$modelname").is_none() {
        issues.push(Issue {
            severity: Severity::Error,
            file: qc.files[0].clone(),
            line: 1,
            message: "No $modelname".into(),
            suggestion: "Add a line like:  $modelname \"props/my_model.mdl\"  (path is relative to the game's models/ folder).".into(),
        });
    } else if let Some(c) = qc.first("$modelname") {
        if let Some(a) = c.args.first() {
            if !a.text.to_ascii_lowercase().ends_with(".mdl") {
                issues.push(Issue {
                    severity: Severity::Warning,
                    file: f(c),
                    line: c.line,
                    message: format!("$modelname '{}' does not end in .mdl", a.text),
                    suggestion: format!("Use  $modelname \"{}.mdl\"", a.text),
                });
            }
        }
    }
    if qc.first("$cdmaterials").is_none() && (qc.first("$body").is_some() || qc.first("$model").is_some() || qc.first("$bodygroup").is_some()) {
        issues.push(Issue {
            severity: Severity::Warning,
            file: qc.files[0].clone(),
            line: 1,
            message: "No $cdmaterials - the model will render with missing (purple/black) textures".into(),
            suggestion: "Add  $cdmaterials \"models/your_folder/\"  pointing at the folder under materials/ that holds the VMTs.".into(),
        });
    }
    let anim_names: Vec<String> = qc
        .commands
        .iter()
        .filter(|c| c.name == "$animation" || c.name == "$declareanimation")
        .filter_map(|c| c.args.first().map(|a| a.text.to_ascii_lowercase()))
        .collect();
    let mut seq_names: Vec<(String, usize, PathBuf)> = Vec::new();

    let check_file = |issues: &mut Vec<Issue>, c: &Command, t: &crate::formats::qc::Token, what: &str| {
        if resolve(&root, &qc.files[t.file], &t.text).is_none() {
            issues.push(Issue {
                severity: Severity::Error,
                file: qc.files[t.file].clone(),
                line: t.line,
                message: format!("{what} file not found: \"{}\" (in {})", t.text, c.name),
                suggestion: format!("Make sure \"{}\" exists relative to the QC folder ({}). Check spelling, the extension (.smd/.dmx) and sub-folders.", t.text, root.display()),
            });
        }
    };

    for c in &qc.commands {
        if !KNOWN_COMMANDS.contains(&c.name.as_str()) {
            issues.push(Issue {
                severity: Severity::Warning,
                file: f(c),
                line: c.line,
                message: format!("Unknown command {}", c.name),
                suggestion: "Check the spelling. studiomdl stops with 'bad command' on commands it does not know.".into(),
            });
        }
        match c.name.as_str() {
            "$body" | "$model" => {
                if c.args.len() < 2 {
                    issues.push(Issue { severity: Severity::Error, file: f(c), line: c.line, message: format!("{} needs a name and a mesh file", c.name), suggestion: format!("{} \"body\" \"mesh.smd\"", c.name) });
                } else {
                    check_file(&mut issues, c, &c.args[1], "Mesh");
                }
            }
            "$bodygroup" => {
                let mut it = c.args.iter().skip(1);
                if !c.args.iter().any(|t| t.is("{")) {
                    issues.push(Issue { severity: Severity::Error, file: f(c), line: c.line, message: "$bodygroup is missing its { } block".into(), suggestion: "$bodygroup \"name\" {\n    studio \"mesh.smd\"\n    blank\n}".into() });
                }
                while let Some(t) = it.next() {
                    if t.text.eq_ignore_ascii_case("studio") && !t.quoted {
                        if let Some(file) = it.next() {
                            check_file(&mut issues, c, file, "Bodygroup mesh");
                        }
                    }
                }
            }
            "$sequence" | "$animation" => {
                let Some(name) = c.args.first() else {
                    issues.push(Issue { severity: Severity::Error, file: f(c), line: c.line, message: format!("{} without a name", c.name), suggestion: format!("{} \"idle\" \"idle.smd\"", c.name) });
                    continue;
                };
                if c.name == "$sequence" {
                    if let Some(prev) = seq_names.iter().find(|s| s.0 == name.text.to_ascii_lowercase()) {
                        issues.push(Issue {
                            severity: Severity::Error,
                            file: f(c),
                            line: c.line,
                            message: format!("Duplicate sequence name \"{}\" (first defined on line {})", name.text, prev.1),
                            suggestion: "Rename one of the sequences; studiomdl requires unique sequence names.".into(),
                        });
                    }
                    seq_names.push((name.text.to_ascii_lowercase(), c.line, f(c)));
                }
                if let Some(second) = c.args.get(1) {
                    if !second.is("{") && !anim_names.contains(&second.text.to_ascii_lowercase()) {
                        check_file(&mut issues, c, second, "Animation");
                    }
                }
                // files listed inside the block of a $sequence
                let mut depth = 0;
                for t in c.args.iter().skip(1) {
                    if t.is("{") {
                        depth += 1;
                    } else if t.is("}") {
                        depth -= 1;
                    } else if depth == 1 && t.quoted && (t.text.to_ascii_lowercase().ends_with(".smd") || t.text.to_ascii_lowercase().ends_with(".dmx")) {
                        check_file(&mut issues, c, t, "Animation");
                    }
                }
            }
            "$collisionmodel" | "$collisionjoints" => {
                if let Some(t) = c.args.first() {
                    check_file(&mut issues, c, t, "Collision mesh");
                }
            }
            "$surfaceprop" | "$cdmaterials" | "$modelname" | "$include" => {
                if c.args.is_empty() {
                    issues.push(Issue { severity: Severity::Error, file: f(c), line: c.line, message: format!("{} has no value", c.name), suggestion: format!("Give {} a quoted value", c.name) });
                }
            }
            _ => {}
        }
    }
    issues.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.line.cmp(&b.line)));
    issues
}
