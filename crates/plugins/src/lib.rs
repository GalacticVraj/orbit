//! Out-of-process plugins: any executable or script described by a `plugin.json`, launched hidden
//! per pick, so a crashing plugin can never take Orbit down. Format and protocol: docs/PLUGINS.md.

use orbit_core::{Action, Outcome, Progress};
use serde::Deserialize;
use std::io::{self, BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

const PUZZLE: char = '\u{EA86}';
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Deserialize)]
struct Manifest {
    id: String,
    name: String,
    #[serde(default)]
    glyph: String,
    actions: Vec<Def>,
}

#[derive(Deserialize)]
struct Def {
    id: String,
    label: String,
    #[serde(default)]
    glyph: String,
    #[serde(default, rename = "match")]
    when: Match,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    output: Output,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Each,
    All,
}

#[derive(Deserialize)]
#[serde(default)]
struct Match {
    extensions: Vec<String>,
    folders: bool,
    min: usize,
    max: usize,
}

impl Default for Match {
    fn default() -> Self {
        Match { extensions: vec!["*".into()], folders: false, min: 1, max: usize::MAX }
    }
}

impl Match {
    /// `sel` = (is_dir, lower-case extension) per selected path; every path must be accepted.
    fn accepts(&self, sel: &[(bool, String)]) -> bool {
        (self.min..=self.max).contains(&sel.len())
            && sel.iter().all(|(is_dir, ext)| {
                if *is_dir {
                    self.folders
                } else {
                    self.extensions.iter().any(|e| e == "*" || e.trim_start_matches('.').eq_ignore_ascii_case(ext))
                }
            })
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct Output {
    suffix: String,
    /// Empty = keep the source extension.
    ext: String,
}

impl Default for Output {
    fn default() -> Self {
        Output { suffix: "_out".into(), ext: String::new() }
    }
}

/// Folder that holds installed plugins (one sub-folder per plugin).
pub fn dir() -> PathBuf {
    let d = orbit_core::data_dir().join("plugins");
    let _ = fs::create_dir_all(&d);
    d
}

/// Plugin actions matching this selection. Ids are "plugin:<plugin-id>:<action-id>".
/// Reads manifests on each call (cheap, no watchers, no background work).
pub fn actions_for(paths: &[PathBuf]) -> Vec<Action> {
    actions_in(&dir(), paths)
}

/// Run a plugin action. Worker thread.
pub fn run(id: &str, paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    run_in(&dir(), id, paths, progress)
}

/// Install a plugin from a folder or a .zip containing a manifest. Returns the plugin's display name.
pub fn install(source: &Path) -> Result<String, String> {
    install_into(&dir(), source)
}

// ---------- discovery ----------

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Parses and sanity-checks `dir/plugin.json` (no file-system checks beyond reading it).
fn load(dir: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(dir.join("plugin.json")).map_err(|_| "plugin.json not found.".to_string())?;
    let m: Manifest =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("Invalid plugin.json: {e}"))?;
    if !valid_id(&m.id) {
        return Err(format!("Invalid plugin id \"{}\" (use a-z, 0-9 and -).", m.id));
    }
    if m.name.trim().is_empty() || m.actions.is_empty() {
        return Err("plugin.json needs a name and at least one action.".into());
    }
    for (i, a) in m.actions.iter().enumerate() {
        if !valid_id(&a.id) || m.actions[..i].iter().any(|b| b.id == a.id) {
            return Err(format!("Invalid or duplicate action id \"{}\".", a.id));
        }
        if a.label.trim().is_empty() || a.command.trim().is_empty() {
            return Err(format!("Action \"{}\" needs a label and a command.", a.id));
        }
    }
    Ok(m)
}

/// Installed plugins with a readable manifest, in folder order. Broken ones are skipped silently.
fn plugins(root: &Path) -> Vec<(PathBuf, Manifest)> {
    let Ok(rd) = fs::read_dir(root) else { return Vec::new() };
    let mut v: Vec<_> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.')) // staging folders
        .filter_map(|e| load(&e.path()).ok().map(|m| (e.path(), m)))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// "E8D2" (hex codepoint) or a single literal character; anything else => `fallback`.
fn glyph(s: &str, fallback: char) -> char {
    let mut c = s.chars();
    match (c.next(), c.next()) {
        (Some(ch), None) => ch,
        _ => u32::from_str_radix(s, 16).ok().and_then(char::from_u32).unwrap_or(fallback),
    }
}

fn actions_in(root: &Path, paths: &[PathBuf]) -> Vec<Action> {
    let installed = plugins(root);
    if paths.is_empty() || installed.is_empty() {
        return Vec::new();
    }
    let sel: Vec<(bool, String)> = paths.iter().map(|p| (p.is_dir(), orbit_core::ext(p))).collect();
    let mut out = Vec::new();
    for (_, m) in installed {
        let mut acts: Vec<Action> = m
            .actions
            .iter()
            .filter(|a| a.when.accepts(&sel))
            .map(|a| Action::new(&format!("plugin:{}:{}", m.id, a.id), &a.label, glyph(&a.glyph, PUZZLE)))
            .collect();
        if acts.len() > 1 {
            out.push(Action::group(&m.name, glyph(&m.glyph, PUZZLE), acts));
        } else {
            out.append(&mut acts);
        }
    }
    out
}

// ---------- running ----------

fn on_path(name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if Path::new(name).extension().is_some() { &[""] } else { &[".exe", ".com", ".cmd", ".bat"] };
    env::split_paths(&env::var_os("PATH")?)
        .flat_map(|d| exts.iter().map(move |e| d.join(format!("{name}{e}"))))
        .find(|p| p.is_file())
}

/// Absolute path, a file inside the plugin (no `..`), or a bare tool name found on PATH.
fn resolve(dir: &Path, command: &str) -> Option<PathBuf> {
    let p = Path::new(command);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let local = dir.join(p);
    if local.is_file() {
        Some(local)
    } else if p.components().count() == 1 {
        on_path(command)
    } else {
        None
    }
}

fn command(dir: &Path, def: &Def) -> Result<Command, String> {
    let exe = resolve(dir, &def.command).ok_or_else(|| format!("Plugin command not found: {}", def.command))?;
    let mut c = match orbit_core::ext(&exe).as_str() {
        "ps1" => {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&exe);
            c
        }
        "py" => {
            let py = on_path("py").or_else(|| on_path("python")).ok_or("This plugin needs Python, which isn't installed.")?;
            let mut c = Command::new(py);
            c.arg(&exe);
            c
        }
        // .cmd/.bat go through cmd.exe via std, which escapes arguments safely for batch files.
        _ => Command::new(&exe),
    };
    c.current_dir(dir).creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    Ok(c)
}

/// Replaces placeholders in one pass (so values containing "{...}" are never re-expanded).
/// A standalone "{files}" argument becomes one argument per file.
fn expand(args: &[String], files: &[PathBuf], out: Option<&Path>, plugin_dir: &Path) -> Vec<String> {
    let first = files.first().map(PathBuf::as_path).unwrap_or(Path::new(""));
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let os = |o: Option<&std::ffi::OsStr>| o.map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let all: Vec<String> = files.iter().map(|f| s(f)).collect();
    let vars = [
        ("{files}", all.join(" ")),
        ("{file}", s(first)),
        ("{dir}", s(first.parent().unwrap_or(Path::new("")))),
        ("{name}", os(first.file_name())),
        ("{stem}", os(first.file_stem())),
        ("{ext}", os(first.extension())),
        ("{out}", out.map(s).unwrap_or_default()),
        ("{plugin_dir}", s(plugin_dir)),
    ];
    let mut v = Vec::new();
    for a in args {
        if a == "{files}" {
            v.extend(all.iter().cloned());
            continue;
        }
        let (mut r, mut rest) = (String::new(), a.as_str());
        while let Some(i) = rest.find('{') {
            r.push_str(&rest[..i]);
            rest = &rest[i..];
            match vars.iter().find(|(k, _)| rest.starts_with(k)) {
                Some((k, val)) => {
                    r.push_str(val);
                    rest = &rest[k.len()..];
                }
                None => {
                    r.push('{');
                    rest = &rest[1..];
                }
            }
        }
        r.push_str(rest);
        v.push(r);
    }
    v
}

#[derive(Debug, PartialEq)]
enum Msg {
    Progress(f32),
    Message(String),
    Reveal(PathBuf),
}

fn parse(line: &str) -> Option<Msg> {
    let (key, val) = line.trim_start_matches('\u{feff}').trim().split_once(':')?;
    let val = val.trim();
    match key.trim() {
        "progress" => val.replace(',', ".").parse::<f32>().ok().filter(|p| p.is_finite()).map(|p| Msg::Progress(p.clamp(0.0, 1.0))),
        "message" if !val.is_empty() => Some(Msg::Message(val.into())),
        "reveal" if !val.is_empty() => Some(Msg::Reveal(val.into())),
        _ => None,
    }
}

/// One process launch over `files`; protocol lines update `o` as they arrive.
fn invoke(dir: &Path, def: &Def, files: &[PathBuf], o: &mut Outcome, progress: &dyn Fn(f32)) -> Result<(), String> {
    let out = def.args.iter().any(|a| a.contains("{out}")).then(|| {
        let ext = if def.output.ext.is_empty() { orbit_core::ext(&files[0]) } else { def.output.ext.clone() };
        orbit_core::output_path(&files[0], &def.output.suffix, &ext)
    });
    let mut child = command(dir, def)?
        .args(expand(&def.args, files, out.as_deref(), dir))
        .spawn()
        .map_err(|e| format!("Couldn't start {}: {e}", def.command))?;
    let mut err = child.stderr.take().expect("piped");
    let errs = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    for line in BufReader::new(child.stdout.take().expect("piped")).split(b'\n').map_while(Result::ok) {
        match parse(&String::from_utf8_lossy(&line)) {
            Some(Msg::Progress(p)) => progress(p),
            Some(Msg::Message(m)) => o.message = m,
            Some(Msg::Reveal(p)) => o.reveal.push(dir.join(p)),
            None => {}
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = errs.join().unwrap_or_default();
    if !status.success() {
        let text = String::from_utf8_lossy(&stderr);
        let last = text.lines().map(str::trim).filter(|l| !l.is_empty()).last();
        return Err(last.map(String::from).unwrap_or_else(|| format!("Plugin failed (exit {})", status.code().unwrap_or(-1))));
    }
    o.reveal.extend(out);
    Ok(())
}

fn run_in(root: &Path, id: &str, paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    let mut parts = id.splitn(3, ':');
    let (Some("plugin"), Some(pid), Some(aid)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("Unknown plugin action: {id}"));
    };
    if paths.is_empty() {
        return Err("Nothing selected.".into());
    }
    let (dir, m) = plugins(root).into_iter().find(|(_, m)| m.id == pid).ok_or("This plugin is no longer installed.")?;
    let def = m.actions.iter().find(|a| a.id == aid).ok_or("This plugin action no longer exists.")?;
    let batches: Vec<&[PathBuf]> = match def.mode {
        Mode::All => vec![paths],
        Mode::Each => paths.chunks(1).collect(),
    };
    let n = batches.len() as f32;
    let mut o = Outcome::new("Done", Vec::new());
    for (i, files) in batches.into_iter().enumerate() {
        invoke(&dir, def, files, &mut o, &|p| progress((i as f32 + p) / n))?;
        progress((i + 1) as f32 / n);
    }
    o.reveal.retain(|p| p.exists());
    Ok(o)
}

// ---------- installing ----------

fn unique() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{t}", std::process::id())
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &t)?;
        } else {
            fs::copy(e.path(), t)?;
        }
    }
    Ok(())
}

fn unzip(src: &Path, to: &Path) -> Result<(), String> {
    let file = fs::File::open(src).map_err(|e| format!("Couldn't open {}: {e}", src.display()))?;
    let mut z = zip::ZipArchive::new(file).map_err(|e| format!("Not a valid zip: {e}"))?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| format!("Corrupt zip: {e}"))?;
        let rel = f
            .enclosed_name()
            .filter(|p| p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)))
            .ok_or_else(|| format!("Unsafe path in zip: {}", f.name()))?;
        let dest = to.join(rel);
        let r = if f.is_dir() {
            fs::create_dir_all(&dest)
        } else {
            dest.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| io::copy(&mut f, &mut fs::File::create(&dest)?).map(drop))
        };
        r.map_err(|e| format!("Couldn't extract {}: {e}", f.name()))?;
    }
    Ok(())
}

fn install_into(root: &Path, source: &Path) -> Result<String, String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let stage = root.join(format!(".install-{}", unique()));
    let r = stage_and_swap(root, source, &stage);
    let _ = fs::remove_dir_all(&stage);
    r
}

fn stage_and_swap(root: &Path, source: &Path, stage: &Path) -> Result<String, String> {
    if source.is_dir() {
        let (src, rt) = (fs::canonicalize(source), fs::canonicalize(root));
        if matches!((&src, &rt), (Ok(s), Ok(r)) if r.starts_with(s)) {
            return Err("Choose the plugin's own folder.".into());
        }
        copy_dir(source, stage).map_err(|e| format!("Couldn't copy the plugin: {e}"))?;
    } else if source.is_file() && orbit_core::ext(source) == "zip" {
        unzip(source, stage)?;
    } else {
        return Err(format!("Not a plugin folder or .zip: {}", source.display()));
    }
    // Manifest at the root, or inside a single top-level folder.
    let top = if stage.join("plugin.json").is_file() {
        stage.to_path_buf()
    } else {
        let entries: Vec<_> = fs::read_dir(stage).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        match entries.as_slice() {
            [only] if only.is_dir() => only.clone(),
            _ => return Err("plugin.json not found.".into()),
        }
    };
    let m = load(&top)?;
    for a in &m.actions {
        resolve(&top, &a.command).ok_or_else(|| format!("Action \"{}\": command not found: {}", a.id, a.command))?;
    }
    let dest = root.join(&m.id);
    let old = root.join(format!(".old-{}-{}", m.id, unique()));
    if dest.exists() {
        fs::rename(&dest, &old).map_err(|e| format!("Couldn't replace the installed version (is it running?): {e}"))?;
    }
    if let Err(e) = fs::rename(&top, &dest) {
        let _ = fs::rename(&old, &dest);
        return Err(format!("Couldn't install the plugin: {e}"));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(m.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    fn temp(name: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("orbit-plugins-{name}-{}", unique()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sel(items: &[(bool, &str)]) -> Vec<(bool, String)> {
        items.iter().map(|(d, e)| (*d, e.to_string())).collect()
    }

    const MANIFEST: &str = r#"{
        "id": "demo", "name": "Demo", "version": "1.0.0", "description": "test",
        "actions": [
            { "id": "write", "label": "Write", "glyph": "E8D2",
              "match": { "extensions": ["txt", ".MD"], "max": 3 },
              "command": "tool.cmd", "args": ["{file}", "{out}"], "output": { "suffix": "_x", "ext": "log" } },
            { "id": "fail", "label": "Fail", "glyph": "F", "match": { "extensions": ["txt"] }, "command": "fail.cmd" },
            { "id": "dirs", "label": "Dirs", "match": { "extensions": [], "folders": true }, "command": "tool.cmd", "mode": "all" }
        ] }"#;

    const TOOL: &str = "@echo off\r\necho progress: 0.5\r\necho noise line\r\necho message: wrote %~nx2\r\necho hello> \"%~2\"\r\n";
    const FAIL: &str = "@echo off\r\necho first 1>&2\r\necho boom 1>&2\r\nexit /b 3\r\n";

    fn make_plugin(at: &Path) {
        fs::create_dir_all(at).unwrap();
        fs::write(at.join("plugin.json"), MANIFEST).unwrap();
        fs::write(at.join("tool.cmd"), TOOL).unwrap();
        fs::write(at.join("fail.cmd"), FAIL).unwrap();
    }

    #[test]
    fn manifest_parsing_and_validation() {
        let d = temp("manifest");
        make_plugin(&d);
        let m = load(&d).unwrap();
        assert_eq!((m.id.as_str(), m.actions.len()), ("demo", 3));
        assert_eq!(m.actions[2].mode, Mode::All);
        assert_eq!(m.actions[1].output.suffix, "_out");
        assert_eq!(m.actions[1].when.max, usize::MAX);
        for bad in [
            r#"{"id":"Bad_Id","name":"x","actions":[{"id":"a","label":"A","command":"c"}]}"#,
            r#"{"id":"ok","name":"x","actions":[]}"#,
            r#"{"id":"ok","name":"x","actions":[{"id":"a","label":"A","command":"c"},{"id":"a","label":"B","command":"c"}]}"#,
            r#"{"id":"ok","name":"x","actions":[{"id":"a","label":"A"}]}"#,
            "not json",
        ] {
            fs::write(d.join("plugin.json"), bad).unwrap();
            assert!(load(&d).is_err(), "{bad}");
        }
        assert!(valid_id("ocr-2") && !valid_id("") && !valid_id("a b") && !valid_id("A"));
        assert_eq!((glyph("E8D2", PUZZLE), glyph("A", PUZZLE), glyph("zz", PUZZLE), glyph("", PUZZLE)), ('\u{E8D2}', 'A', PUZZLE, PUZZLE));
        // Commands must live inside the plugin unless absolute or a system tool.
        assert!(resolve(&d, "tool.cmd").is_some() && resolve(&d, "cmd").is_some());
        assert!(resolve(&d, "missing.exe").is_none() && resolve(&d, "../tool.cmd").is_none() && resolve(&d, "sub/x.ps1").is_none());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn matching() {
        let any = Match::default();
        assert!(any.accepts(&sel(&[(false, "png"), (false, "")])));
        assert!(!any.accepts(&sel(&[(true, "")])) && !any.accepts(&[]));
        let m = Match { extensions: vec!["png".into(), ".JPG".into()], folders: true, min: 2, max: 3 };
        assert!(m.accepts(&sel(&[(false, "png"), (false, "jpg")])));
        assert!(m.accepts(&sel(&[(false, "png"), (true, "")])));
        assert!(!m.accepts(&sel(&[(false, "png")]))); // below min
        assert!(!m.accepts(&sel(&[(false, "png"); 4]))); // above max
        assert!(!m.accepts(&sel(&[(false, "png"), (false, "gif")]))); // one non-matching file rejects
    }

    #[test]
    fn placeholders() {
        let files = [PathBuf::from(r"C:\a b\photo.PNG"), PathBuf::from(r"C:\x\{out}.jpg")];
        let args: Vec<String> =
            ["--in", "{file}", "{files}", "{dir}|{name}|{stem}|{ext}", "{out}", "{plugin_dir}\\m", "{unknown}{", "all={files}"]
                .map(String::from)
                .to_vec();
        let v = expand(&args, &files, Some(Path::new(r"C:\a b\photo_x.txt")), Path::new(r"C:\p"));
        assert_eq!(
            v,
            [
                "--in",
                r"C:\a b\photo.PNG",
                r"C:\a b\photo.PNG",
                r"C:\x\{out}.jpg",
                r"C:\a b|photo.PNG|photo|PNG",
                r"C:\a b\photo_x.txt",
                r"C:\p\m",
                "{unknown}{",
                r"all=C:\a b\photo.PNG C:\x\{out}.jpg",
            ]
        );
    }

    #[test]
    fn protocol() {
        assert_eq!(parse("progress: 0.42"), Some(Msg::Progress(0.42)));
        assert_eq!(parse("  progress:0,5\r"), Some(Msg::Progress(0.5)));
        assert_eq!(parse("progress: 7"), Some(Msg::Progress(1.0)));
        assert_eq!(parse("progress: NaN"), None);
        assert_eq!(parse("message: Saved: 3 files"), Some(Msg::Message("Saved: 3 files".into())));
        assert_eq!(parse(r"reveal: C:\out\a.txt"), Some(Msg::Reveal(PathBuf::from(r"C:\out\a.txt"))));
        assert_eq!(parse("message:"), None);
        assert_eq!(parse(r"C:\some\path"), None);
        assert_eq!(parse("random output"), None);
    }

    #[test]
    fn install_run_end_to_end() {
        let root = temp("e2e-root");
        let src = temp("e2e-src").join("demo-src");
        make_plugin(&src);
        assert_eq!(install_into(&root, &src).unwrap(), "Demo");
        assert!(root.join("demo").join("tool.cmd").is_file());
        // Reinstall replaces cleanly, no staging leftovers.
        assert_eq!(install_into(&root, &src).unwrap(), "Demo");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

        let work = temp("e2e-work");
        let a = work.join("a b.txt");
        let b = work.join("c.md");
        fs::write(&a, "1").unwrap();
        fs::write(&b, "2").unwrap();

        // Ring: two matching actions => grouped under the plugin; a folder => single action, ungrouped.
        let acts = actions_in(&root, &[a.clone()]);
        assert_eq!(acts.len(), 1);
        assert_eq!((acts[0].label.as_str(), acts[0].glyph, acts[0].children.len()), ("Demo", PUZZLE, 2));
        assert_eq!(acts[0].children[0].id, "plugin:demo:write");
        assert_eq!(acts[0].children[0].glyph, '\u{E8D2}');
        let acts = actions_in(&root, &[a.clone(), b.clone()]);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].id, "plugin:demo:write");
        let acts = actions_in(&root, &[work.clone()]);
        assert_eq!((acts.len(), acts[0].id.as_str()), (1, "plugin:demo:dirs"));
        assert!(actions_in(&root, &[work.join("x.png")]).is_empty());
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(root.join("broken").join("plugin.json"), "{").unwrap();
        assert_eq!(actions_in(&root, &[a.clone()]).len(), 1);

        // Run "each" over two files: aggregated progress, last message wins, {out} files revealed.
        let seen = Mutex::new(Vec::new());
        let report = |p: f32| seen.lock().unwrap().push(p);
        let o = run_in(&root, "plugin:demo:write", &[a.clone(), b.clone()], &report).unwrap();
        assert_eq!(o.message, "wrote c_x.log");
        assert_eq!(o.reveal, vec![work.join("a b_x.log"), work.join("c_x.log")]);
        assert_eq!(fs::read_to_string(work.join("a b_x.log")).unwrap().trim(), "hello");
        assert_eq!(*seen.lock().unwrap(), vec![0.25, 0.5, 0.75, 1.0]);
        // Never overwrites: a second run picks a free name.
        let o = run_in(&root, "plugin:demo:write", &[a.clone()], &|_| {}).unwrap();
        assert_eq!(o.reveal, vec![work.join("a b_x (2).log")]);

        assert_eq!(run_in(&root, "plugin:demo:fail", &[a.clone()], &|_| {}).unwrap_err(), "boom");
        assert!(run_in(&root, "plugin:nope:x", &[a.clone()], &|_| {}).is_err());
        assert!(run_in(&root, "garbage", &[a], &|_| {}).is_err());
        for d in [root, src, work] {
            let _ = fs::remove_dir_all(d);
        }
    }

    fn zip_with(path: &Path, entries: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, body) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn zip_install_and_zip_slip() {
        let root = temp("zip-root");
        let work = temp("zip-work");
        let good = work.join("demo.zip");
        zip_with(&good, &[("demo-1.0/plugin.json", MANIFEST), ("demo-1.0/tool.cmd", TOOL), ("demo-1.0/fail.cmd", FAIL)]);
        assert_eq!(install_into(&root, &good).unwrap(), "Demo");
        assert!(root.join("demo").join("fail.cmd").is_file());

        let evil = work.join("evil.zip");
        zip_with(&evil, &[("plugin.json", MANIFEST), ("tool.cmd", TOOL), ("fail.cmd", FAIL), ("../../evil.txt", "x")]);
        let e = install_into(&root, &evil).unwrap_err();
        assert!(e.contains("Unsafe path"), "{e}");
        assert!(!root.join("evil.txt").exists() && !env::temp_dir().join("evil.txt").exists());

        let missing_cmd = work.join("missing.zip");
        zip_with(&missing_cmd, &[("plugin.json", MANIFEST), ("tool.cmd", TOOL)]);
        assert!(install_into(&root, &missing_cmd).unwrap_err().contains("fail.cmd"));
        // Failed installs leave the existing version and no staging folders behind.
        assert!(root.join("demo").join("fail.cmd").is_file());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        for d in [root, work] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn checksum_example_end_to_end() {
        let root = temp("checksum-root");
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples-plugins/checksum");
        assert_eq!(install_into(&root, &example).unwrap(), "Checksum");
        let work = temp("checksum-work");
        let f = work.join("abc file.txt");
        fs::write(&f, "abc").unwrap();
        let acts = actions_in(&root, &[f.clone()]);
        assert_eq!(acts[0].id, "plugin:checksum:sha256");
        assert!(actions_in(&root, &[work.clone()]).is_empty());
        let o = run_in(&root, "plugin:checksum:sha256", &[f], &|_| {}).unwrap();
        assert_eq!(o.message, "SHA-256 copied");
        let clip = Command::new("powershell").args(["-NoProfile", "-Command", "Get-Clipboard"]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&clip.stdout).trim(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        for d in [root, work] {
            let _ = fs::remove_dir_all(d);
        }
    }
}
