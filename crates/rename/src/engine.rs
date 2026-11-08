//! Rename rules (pure) plus the safe two-phase apply / undo on disk.

use orbit_core::{kind_of, Kind, Outcome};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub path: PathBuf,
    pub stem: String,
    /// Original extension without the dot, original case ("" for folders / none).
    pub ext: String,
    pub kind: Kind,
    /// FILETIME of last write (sort key).
    pub modified: u64,
    /// Local modified date, YYYY-MM-DD.
    pub date: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Case {
    Keep,
    Lower,
    Upper,
    Title,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sort {
    Name,
    Date,
}

#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub pattern: String,
    pub start: u64,
    pub find: String,
    pub replace: String,
    pub case: Case,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Problem {
    Empty,
    BadChar,
    Trailing,
    Reserved,
    TooLong,
    Duplicate,
    Exists,
}

impl Problem {
    pub fn label(self) -> &'static str {
        match self {
            Problem::Empty => "Empty name",
            Problem::BadChar => "Invalid character",
            Problem::Trailing => "Ends with dot or space",
            Problem::Reserved => "Reserved name",
            Problem::TooLong => "Too long",
            Problem::Duplicate => "Duplicate",
            Problem::Exists => "Already exists",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Same,
    Ok,
    Bad(Problem),
}

pub(crate) fn load(paths: &[PathBuf]) -> Vec<Entry> {
    use std::os::windows::fs::MetadataExt;
    paths
        .iter()
        .filter_map(|p| {
            let m = std::fs::metadata(p).ok()?;
            let name = p.file_name()?.to_string_lossy().into_owned();
            let (stem, ext) = match name.rfind('.') {
                Some(i) if i > 0 && !m.is_dir() => (name[..i].to_string(), name[i + 1..].to_string()),
                _ => (name, String::new()),
            };
            let t = m.last_write_time();
            Some(Entry { path: p.clone(), stem, ext, kind: kind_of(p), modified: t, date: local_date(t) })
        })
        .collect()
}

fn local_date(ft: u64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let f = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
    let (mut utc, mut loc) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&f, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut loc).is_err() {
            return String::new();
        }
    }
    format!("{:04}-{:02}-{:02}", loc.wYear, loc.wMonth, loc.wDay)
}

/// Lower-cased full paths of everything in the entries' folders (collision check snapshot).
pub(crate) fn existing(entries: &[Entry]) -> HashSet<String> {
    let dirs: HashSet<&Path> = entries.iter().filter_map(|e| e.path.parent()).collect();
    dirs.into_iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| key(&e.path()))
        .collect()
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// Natural order: digit runs compare numerically, text case-insensitively (IMG_2 < IMG_10).
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(|c| c.is_ascii_digit()) {
                        s.push(c);
                    }
                    s
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = m.len().cmp(&n.len()).then_with(|| m.cmp(n));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn file_name(e: &Entry) -> String {
    e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub(crate) fn sort(entries: &mut [Entry], by: Sort) {
    entries.sort_by(|a, b| {
        let n = natural_cmp(&file_name(a), &file_name(b));
        match by {
            Sort::Name => n,
            Sort::Date => a.modified.cmp(&b.modified).then(n),
        }
    });
}

/// New file names, one per entry (in order).
pub(crate) fn render(entries: &[Entry], o: &Options) -> Vec<String> {
    let has_ext = o.pattern.contains("{ext}");
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let n = o.start.saturating_add(i as u64);
            let stem = if o.find.is_empty() { e.stem.clone() } else { e.stem.replace(&o.find, &o.replace) };
            let mut out = String::new();
            let mut rest = o.pattern.as_str();
            while let Some(c) = rest.chars().next() {
                if c == '#' {
                    let w = rest.len() - rest.trim_start_matches('#').len();
                    out += &format!("{n:0w$}");
                    rest = &rest[w..];
                    continue;
                }
                let token = [("{name}", stem.as_str()), ("{date}", e.date.as_str()), ("{ext}", e.ext.as_str())]
                    .into_iter()
                    .find(|(t, _)| rest.starts_with(t));
                if let Some((t, v)) = token {
                    out += v;
                    rest = &rest[t.len()..];
                } else {
                    out.push(c);
                    rest = &rest[c.len_utf8()..];
                }
            }
            let mut out = recase(&out, o.case);
            if !has_ext && !e.ext.is_empty() {
                out.push('.');
                out += &e.ext;
            }
            out
        })
        .collect()
}

fn recase(s: &str, case: Case) -> String {
    match case {
        Case::Keep => s.to_string(),
        Case::Lower => s.to_lowercase(),
        Case::Upper => s.to_uppercase(),
        Case::Title => {
            let mut start = true;
            let mut out = String::new();
            for c in s.chars() {
                if start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                start = !c.is_alphanumeric();
            }
            out
        }
    }
}

/// Camera / OS prefixes that say nothing about the content.
const JUNK: &[&str] = &[
    "img", "image", "images", "dsc", "dscn", "dscf", "dcim", "pxl", "vid", "video", "mvimg", "screenshot",
    "screen shot", "screen recording", "scan", "photo", "pic", "picture", "file", "document", "doc", "new",
    "copy", "untitled", "mov", "gopr", "dji", "wp", "whatsapp image", "whatsapp video", "capture", "pdf",
];

/// `Image_###`, `Vacation_###`, ... from the selection. Pad = max(3, digits of count).
pub(crate) fn smart_pattern(entries: &[Entry]) -> String {
    let width = 3.max(entries.len().to_string().len());
    let base = common_prefix(entries).unwrap_or_else(|| {
        let all = |k| entries.iter().all(|e| e.kind == k);
        if all(Kind::Image) {
            "Image"
        } else if all(Kind::Video) {
            "Video"
        } else if all(Kind::Pdf) {
            "Document"
        } else if all(Kind::Folder) {
            "Folder"
        } else {
            "File"
        }
        .to_string()
    });
    format!("{base}_{}", "#".repeat(width))
}

fn common_prefix(entries: &[Entry]) -> Option<String> {
    let first: Vec<char> = entries.first()?.stem.chars().collect();
    let mut n = first.len();
    for e in &entries[1..] {
        n = n.min(first.iter().zip(e.stem.chars()).take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase())).count());
    }
    n = first[..n].iter().take_while(|c| c.is_alphabetic() || matches!(c, ' ' | '_' | '-')).count();
    // Whole words only: never cut "Vacation" out of "Vacations".
    let boundary = |n: usize| entries.iter().all(|e| e.stem.chars().nth(n).map_or(true, |c| !c.is_alphabetic()));
    while n > 0 && !boundary(n) {
        n -= 1;
    }
    let mut p: String = first[..n].iter().collect();
    p = p.trim_end_matches([' ', '_', '-']).to_string();
    // Drop a dangling one-letter word ("Report v2" -> "Report").
    if let Some(i) = p.rfind([' ', '_', '-']) {
        if p[i + 1..].chars().count() == 1 {
            p = p[..i].trim_end_matches([' ', '_', '-']).to_string();
        }
    }
    let letters = p.chars().filter(|c| c.is_alphabetic()).count();
    (letters >= 3 && !JUNK.contains(&p.to_lowercase().as_str())).then_some(p)
}

fn invalid(name: &str) -> Option<Problem> {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if name.is_empty() || name.starts_with('.') {
        Some(Problem::Empty)
    } else if name.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c)) {
        Some(Problem::BadChar)
    } else if name.ends_with(['.', ' ']) {
        Some(Problem::Trailing)
    } else if RESERVED.contains(&name.split('.').next().unwrap_or("").trim_end().to_uppercase().as_str()) {
        Some(Problem::Reserved)
    } else if name.encode_utf16().count() > 255 {
        Some(Problem::TooLong)
    } else {
        None
    }
}

fn target(e: &Entry, name: &str) -> PathBuf {
    e.path.with_file_name(name)
}

/// Per-row status. `existing` = lower-cased paths already on disk (see [`existing`]).
pub(crate) fn validate(entries: &[Entry], names: &[String], existing: &HashSet<String>) -> Vec<Status> {
    let changed = |i: usize| file_name(&entries[i]) != names[i];
    let moving: HashSet<String> = (0..entries.len()).filter(|&i| changed(i)).map(|i| key(&entries[i].path)).collect();
    let mut count: HashMap<String, usize> = HashMap::new();
    for (e, n) in entries.iter().zip(names) {
        *count.entry(key(&target(e, n))).or_default() += 1;
    }
    (0..entries.len())
        .map(|i| {
            let k = key(&target(&entries[i], &names[i]));
            if !changed(i) {
                Status::Same
            } else if let Some(p) = invalid(&names[i]) {
                Status::Bad(p)
            } else if count[&k] > 1 {
                Status::Bad(Problem::Duplicate)
            } else if existing.contains(&k) && !moving.contains(&k) {
                Status::Bad(Problem::Exists)
            } else {
                Status::Ok
            }
        })
        .collect()
}

/// Footer line and whether Apply is allowed.
pub(crate) fn summary(status: &[Status], noun: &str) -> (String, bool) {
    let bad = status.iter().filter(|s| matches!(s, Status::Bad(_))).count();
    let same = status.iter().filter(|s| **s == Status::Same).count();
    let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    if let Some(Status::Bad(p)) = status.iter().find(|s| matches!(s, Status::Bad(_))) {
        let why = match p {
            Problem::Exists if bad == 1 => "already exists in this folder",
            Problem::Exists => "already exist in this folder",
            Problem::Duplicate => "would be duplicated",
            _ if bad == 1 => "isn't valid on Windows",
            _ => "aren't valid on Windows",
        };
        return (format!("Can't rename: {} {why}", plural(bad, "name")), false);
    }
    let go = status.len() - same;
    if go == 0 {
        return ("Nothing to change".into(), false);
    }
    let mut s = format!("Ready to rename {}", plural(go, noun));
    if same > 0 {
        s += &format!(" · {same} unchanged");
    }
    (s, true)
}

#[derive(Serialize, Deserialize)]
struct Move {
    from: PathBuf,
    to: PathBuf,
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};
    // No REPLACE_EXISTING: never overwrites, even if something appeared since the preview.
    unsafe { MoveFileExW(&HSTRING::from(from.as_os_str()), &HSTRING::from(to.as_os_str()), MOVE_FILE_FLAGS(0)) }
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

/// Two-phase rename (via unique temp names, so swaps and cycles work). All or nothing.
pub(crate) fn rename_all(pairs: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let tag = format!(
        ".orbit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    );
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut run = || -> Result<(), String> {
        let mut temps = Vec::new();
        for (i, (from, _)) in pairs.iter().enumerate() {
            let tmp = from.with_file_name(format!("{tag}-{i}.tmp"));
            move_file(from, &tmp).map_err(|e| format!("Couldn't rename {}: {e}", from.display()))?;
            done.push((from.clone(), tmp.clone()));
            temps.push(tmp);
        }
        for (tmp, (_, to)) in temps.iter().zip(pairs) {
            let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            move_file(tmp, to).map_err(|e| format!("Couldn't rename to {name}: {e}"))?;
            done.push((tmp.clone(), to.clone()));
        }
        Ok(())
    };
    let r = run();
    if r.is_err() {
        for (a, b) in done.iter().rev() {
            let _ = move_file(b, a);
        }
    }
    r
}

/// Rename and record the batch for undo.
pub(crate) fn apply(pairs: &[(PathBuf, PathBuf)], journal: &Path) -> Result<(), String> {
    rename_all(pairs)?;
    let moves: Vec<Move> = pairs.iter().map(|(f, t)| Move { from: f.clone(), to: t.clone() }).collect();
    // The renames already succeeded; a journal that can't be written only costs the undo.
    let _ = serde_json::to_vec(&moves).map(|j| std::fs::write(journal, j));
    Ok(())
}

fn read_journal(journal: &Path) -> Option<Vec<Move>> {
    serde_json::from_slice(&std::fs::read(journal).ok()?).ok()
}

pub(crate) fn can_undo(journal: &Path) -> bool {
    read_journal(journal).is_some_and(|m| m.iter().any(|m| m.to.exists()))
}

pub(crate) fn undo_at(journal: &Path) -> Result<Outcome, String> {
    let moves = read_journal(journal).ok_or("Nothing to undo")?;
    let mut live: Vec<&Move> = moves.iter().filter(|m| m.to.exists()).collect();
    // `from` must be free, or be vacated by another row of this undo (swaps). Repeat until stable.
    loop {
        let leaving: HashSet<String> = live.iter().map(|m| key(&m.to)).collect();
        let n = live.len();
        live.retain(|m| !m.from.exists() || leaving.contains(&key(&m.from)));
        if live.len() == n {
            break;
        }
    }
    if live.is_empty() {
        return Err("Nothing to undo: those files were moved or renamed".into());
    }
    let pairs: Vec<(PathBuf, PathBuf)> = live.iter().map(|m| (m.to.clone(), m.from.clone())).collect();
    rename_all(&pairs)?;
    let _ = std::fs::remove_file(journal);
    let n = pairs.len();
    let mut msg = format!("Restored {n} name{}", if n == 1 { "" } else { "s" });
    if moves.len() > n {
        msg += &format!(" · {} skipped", moves.len() - n);
    }
    Ok(Outcome::new(msg, pairs.into_iter().map(|(_, from)| from).collect()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn entry(dir: &Path, name: &str, modified: u64) -> Entry {
        let path = dir.join(name);
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (name[..i].to_string(), name[i + 1..].to_string()),
            _ => (name.to_string(), String::new()),
        };
        Entry { kind: kind_of(&path), path, stem, ext, modified, date: "2024-07-15".into() }
    }

    fn opts(pattern: &str) -> Options {
        Options { pattern: pattern.into(), start: 1, find: String::new(), replace: String::new(), case: Case::Keep }
    }

    fn entries(names: &[&str]) -> Vec<Entry> {
        names.iter().enumerate().map(|(i, n)| entry(Path::new("C:\\x"), n, i as u64)).collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orbit-rename-test-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn patterns_and_padding() {
        let e = entries(&["a.jpg", "b.PNG", "c"]);
        assert_eq!(render(&e, &opts("Vacation_###")), ["Vacation_001.jpg", "Vacation_002.PNG", "Vacation_003"]);
        let mut o = opts("#-{name}-{date}");
        o.start = 99;
        assert_eq!(render(&e, &o), ["99-a-2024-07-15.jpg", "100-b-2024-07-15.PNG", "101-c-2024-07-15"]);
        assert_eq!(render(&e[..1], &opts("{name}.{ext}.bak")), ["a.jpg.bak"]);
        let mut o = opts("{name} copy");
        o.find = "a".into();
        o.replace = "big apple".into();
        o.case = Case::Title;
        assert_eq!(render(&e[..1], &o), ["Big Apple Copy.jpg"]);
        o.case = Case::Upper;
        assert_eq!(render(&e[..1], &o), ["BIG APPLE COPY.jpg"]);
        o.case = Case::Lower;
        assert_eq!(render(&e[..1], &Options { pattern: "ÄB".into(), ..o }), ["äb.jpg"]);
    }

    #[test]
    fn natural_and_date_sort() {
        let mut e = entries(&["IMG_10.jpg", "img_2.jpg", "IMG_1.jpg", "IMG_002.jpg", "a.jpg"]);
        sort(&mut e, Sort::Name);
        let names: Vec<String> = e.iter().map(file_name).collect();
        assert_eq!(names, ["a.jpg", "IMG_1.jpg", "IMG_002.jpg", "img_2.jpg", "IMG_10.jpg"]);
        sort(&mut e, Sort::Date);
        assert_eq!(file_name(&e[0]), "IMG_10.jpg");
        assert_eq!(file_name(&e[4]), "a.jpg");
    }

    #[test]
    fn smart_defaults() {
        let e = entries(&["IMG_1024.jpg", "IMG_1025.jpg"]);
        assert_eq!(smart_pattern(&e), "Image_###");
        assert_eq!(render(&e, &opts(&smart_pattern(&e)))[0], "Image_001.jpg");
        assert_eq!(smart_pattern(&entries(&["PXL_2024.mp4", "VID_1.mov"])), "Video_###");
        assert_eq!(smart_pattern(&entries(&["a.pdf", "b.pdf"])), "Document_###");
        assert_eq!(smart_pattern(&entries(&["a.pdf", "b.jpg"])), "File_###");
        assert_eq!(smart_pattern(&entries(&["Screenshot 2024-01-01.png", "Screenshot 2024-02-01.png"])), "Image_###");
        assert_eq!(smart_pattern(&entries(&["Vacation 1.jpg", "vacation 22.jpg"])), "Vacation_###");
        assert_eq!(smart_pattern(&entries(&["Vacations.jpg", "Vacation2.jpg"])), "Image_###");
        assert_eq!(smart_pattern(&entries(&["Report v2.pdf"])), "Report_###");
        let many: Vec<String> = (0..1000).map(|i| format!("x{i}.jpg")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(smart_pattern(&entries(&many)), "Image_####");
    }

    #[test]
    fn validation() {
        let e = entries(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        let existing: HashSet<String> =
            ["a.txt", "b.txt", "c.txt", "d.txt", "taken.txt"].iter().map(|n| format!("c:\\x\\{n}")).collect();
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // swap a<->b is fine, c unchanged, d -> taken collides.
        let s = validate(&e, &names(&["b.txt", "A.txt", "c.txt", "Taken.txt"]), &existing);
        assert_eq!(s, [Status::Ok, Status::Ok, Status::Same, Status::Bad(Problem::Exists)]);
        let s = validate(&e, &names(&["x.txt", "X.txt", "C.txt", "c.txt"]), &existing);
        assert_eq!(s[0], Status::Bad(Problem::Duplicate));
        assert_eq!(s[1], Status::Bad(Problem::Duplicate));
        assert_eq!(s[2], Status::Bad(Problem::Duplicate), "case-only rename of c collides with d -> c");
        let s = validate(&e, &names(&["a?.txt", "con.txt", "x.", ".txt"]), &existing);
        assert_eq!(
            s,
            [
                Status::Bad(Problem::BadChar),
                Status::Bad(Problem::Reserved),
                Status::Bad(Problem::Trailing),
                Status::Bad(Problem::Empty)
            ]
        );
        assert_eq!(summary(&s, "file"), ("Can't rename: 4 names aren't valid on Windows".into(), false));
        assert_eq!(summary(&[Status::Bad(Problem::Exists)], "file").0, "Can't rename: 1 name already exists in this folder");
        assert_eq!(summary(&[Status::Same, Status::Ok], "file"), ("Ready to rename 1 file · 1 unchanged".into(), true));
        assert!(!summary(&[Status::Same], "file").1);
    }

    #[test]
    fn swap_journal_and_undo() {
        let d = tmp("swap");
        let (a, b, c) = (d.join("A.txt"), d.join("B.txt"), d.join("c.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        std::fs::write(&c, "c").unwrap();
        let journal = d.join("journal.json");
        apply(&[(a.clone(), b.clone()), (b.clone(), a.clone()), (c.clone(), d.join("C.txt"))], &journal).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "b");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "a");
        assert!(can_undo(&journal));
        let o = undo_at(&journal).unwrap();
        assert_eq!(o.message, "Restored 3 names");
        assert_eq!(o.reveal.len(), 3);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        let names: Vec<String> =
            std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"c.txt".to_string()), "case-only rename restored: {names:?}");
        assert!(!journal.exists() && !can_undo(&journal));
    }

    #[test]
    fn rollback_on_failure() {
        let d = tmp("rollback");
        let (a, b, blocker) = (d.join("a.txt"), d.join("b.txt"), d.join("y.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        std::fs::write(&blocker, "keep").unwrap();
        let journal = d.join("journal.json");
        let r = apply(&[(a.clone(), d.join("x.txt")), (b.clone(), blocker.clone())], &journal);
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "keep");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 3, "no temp or partial files left");
        assert!(!journal.exists());
    }

    #[test]
    fn undo_skips_missing() {
        let d = tmp("skip");
        let (a, b) = (d.join("a.txt"), d.join("b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let journal = d.join("journal.json");
        apply(&[(a.clone(), d.join("1.txt")), (b.clone(), d.join("2.txt"))], &journal).unwrap();
        std::fs::remove_file(d.join("2.txt")).unwrap();
        let o = undo_at(&journal).unwrap();
        assert_eq!(o.message, "Restored 1 name · 1 skipped");
        assert!(a.exists());
        assert!(undo_at(&journal).is_err());
    }
}
