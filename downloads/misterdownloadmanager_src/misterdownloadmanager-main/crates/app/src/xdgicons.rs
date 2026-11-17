//! The freedesktop icon lookup behind the Linux file icons (unix.rs `file_icon`): a file's MIME
//! type from shared-mime-info (`mime/globs2`), that type's icon names (`mime/icons`,
//! `mime/generic-icons`, else the specification's defaults), and those names found in the
//! desktop's icon theme, the themes it inherits, then hicolor, by the Icon Theme Specification's
//! size rules. Plain file reading: portable, so its tests run on every system against made-up
//! folders.

use std::collections::HashMap;
use std::path::PathBuf;

/// The XDG folders to read, most important first.
pub struct Dirs {
    /// `$XDG_DATA_HOME` (else `~/.local/share`), then `$XDG_DATA_DIRS` (else `/usr/local/share`
    /// and `/usr/share`).
    pub data: Vec<PathBuf>,
    /// The icon themes' base folders: `~/.icons`, each data folder's `icons`.
    pub icons: Vec<PathBuf>,
}

impl Dirs {
    /// From the environment's values (`HOME`, `XDG_DATA_HOME`, `XDG_DATA_DIRS`).
    pub fn new(home: Option<&str>, data_home: Option<&str>, data_dirs: Option<&str>) -> Self {
        let home = home.filter(|h| h.starts_with('/')).map(PathBuf::from);
        let mut data = Vec::new();
        match data_home.filter(|d| d.starts_with('/')) {
            Some(d) => data.push(PathBuf::from(d)),
            None => data.extend(home.as_ref().map(|h| h.join(".local/share"))),
        }
        let dirs: Vec<PathBuf> = data_dirs
            .unwrap_or("")
            .split(':')
            .filter(|d| d.starts_with('/'))
            .map(PathBuf::from)
            .collect();
        if dirs.is_empty() {
            data.extend(["/usr/local/share", "/usr/share"].map(PathBuf::from));
        } else {
            data.extend(dirs);
        }
        let mut icons: Vec<PathBuf> = home.iter().map(|h| h.join(".icons")).collect();
        icons.extend(data.iter().map(|d| d.join("icons")));
        Dirs { data, icons }
    }
}

/// `key`'s value in `[section]` of an INI-style file (index.theme, kdeglobals, settings.ini).
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name == section;
        } else if inside
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            let v = v.trim();
            return (!v.is_empty()).then(|| v.to_string());
        }
    }
    None
}

// ---- MIME types --------------------------------------------------------------------------------

struct Glob {
    weight: u32,
    mime: String,
    /// Lowercase unless `case_sensitive`.
    pattern: String,
    case_sensitive: bool,
}

/// shared-mime-info's file name patterns and icon names.
#[derive(Default)]
pub struct MimeDb {
    globs: Vec<Glob>,
    icons: HashMap<String, String>,
    generic: HashMap<String, String>,
}

impl MimeDb {
    /// Read `mime/` in each data folder; a more important folder's entries win.
    pub fn load(data: &[PathBuf]) -> Self {
        let mut db = MimeDb::default();
        for dir in data.iter().rev() {
            let read = |f: &str| std::fs::read_to_string(dir.join("mime").join(f)).ok();
            if let Some(t) = read("globs2") {
                db.add_globs(&t);
            }
            for (file, map) in [("icons", &mut db.icons), ("generic-icons", &mut db.generic)] {
                if let Some(t) = read(file) {
                    for (m, i) in t.lines().filter_map(|l| l.trim().split_once(':')) {
                        map.insert(m.to_string(), i.to_string());
                    }
                }
            }
        }
        db
    }

    /// globs2 lines: `weight:type:pattern[:flags]`; `__NOGLOBS__` drops a type's earlier ones.
    fn add_globs(&mut self, text: &str) {
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut f = line.splitn(4, ':');
            let (Some(w), Some(mime), Some(pat)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            if pat == "__NOGLOBS__" {
                self.globs.retain(|g| g.mime != mime);
                continue;
            }
            let case_sensitive = f.next().is_some_and(|fl| fl.split(',').any(|x| x == "cs"));
            self.globs.push(Glob {
                weight: w.parse().unwrap_or(50),
                mime: mime.to_string(),
                pattern: if case_sensitive {
                    pat.to_string()
                } else {
                    pat.to_lowercase()
                },
                case_sensitive,
            });
        }
    }

    /// The MIME type of a file named `name`: the matching pattern of the highest weight, then
    /// the longest (the specification's order).
    pub fn mime_for(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        self.globs
            .iter()
            .filter(|g| glob_match(&g.pattern, if g.case_sensitive { name } else { &lower }))
            .max_by_key(|g| (g.weight, g.pattern.len()))
            .map(|g| g.mime.as_str())
    }

    /// The icon names to try for `mime`, in order: its own (from `icons`, else the type with `-`
    /// for `/`), then its generic one (from `generic-icons`, else `<media>-x-generic`).
    pub fn icon_names(&self, mime: &str) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(i) = self.icons.get(mime) {
            names.push(i.clone());
        }
        names.push(mime.replace('/', "-"));
        match self.generic.get(mime) {
            Some(g) => names.push(g.clone()),
            None => {
                let media = mime.split('/').next().unwrap_or("application");
                names.push(format!("{media}-x-generic"));
            }
        }
        names.dedup();
        names
    }
}

/// A shell glob: `*` any run, `?` one character, `[...]` a set; the rest literal.
fn glob_match(pat: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| go(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some('[') => match p.iter().position(|c| *c == ']') {
                Some(end) if end > 1 && !n.is_empty() => {
                    let set = &p[1..end];
                    let (neg, set) = match set.first() {
                        Some('!') => (true, &set[1..]),
                        _ => (false, set),
                    };
                    let mut hit = false;
                    let mut i = 0;
                    while i < set.len() {
                        if i + 2 < set.len() && set[i + 1] == '-' {
                            hit |= (set[i]..=set[i + 2]).contains(&n[0]);
                            i += 3;
                        } else {
                            hit |= set[i] == n[0];
                            i += 1;
                        }
                    }
                    hit != neg && go(&p[end + 1..], &n[1..])
                }
                _ => !n.is_empty() && n[0] == '[' && go(&p[1..], &n[1..]),
            },
            Some(c) => n.first() == Some(c) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

// ---- icon themes -------------------------------------------------------------------------------

#[derive(Debug)]
enum DirType {
    Fixed,
    Scalable { min: u32, max: u32 },
    Threshold(u32),
}

#[derive(Debug)]
struct SubDir {
    path: String,
    /// Size × scale: the pixels its icons are drawn for.
    px: u32,
    scale: u32,
    kind: DirType,
}

impl SubDir {
    fn matches(&self, px: u32) -> bool {
        match self.kind {
            DirType::Fixed => self.px == px,
            DirType::Scalable { min, max } => (min * self.scale..=max * self.scale).contains(&px),
            DirType::Threshold(t) => self.px.abs_diff(px) <= t * self.scale,
        }
    }

    fn distance(&self, px: u32) -> u32 {
        match self.kind {
            DirType::Fixed => self.px.abs_diff(px),
            DirType::Scalable { min, max } => {
                let (min, max) = (min * self.scale, max * self.scale);
                if px < min {
                    min - px
                } else {
                    px.saturating_sub(max)
                }
            }
            DirType::Threshold(t) => {
                let t = t * self.scale;
                if px + t < self.px {
                    self.px - t - px
                } else {
                    px.saturating_sub(self.px + t)
                }
            }
        }
    }
}

struct Theme {
    /// This theme's folder in each base folder that has it.
    roots: Vec<PathBuf>,
    dirs: Vec<SubDir>,
    inherits: Vec<String>,
}

impl Theme {
    fn load(bases: &[PathBuf], name: &str) -> Option<Theme> {
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return None;
        }
        let roots: Vec<PathBuf> = bases
            .iter()
            .map(|b| b.join(name))
            .filter(|r| r.is_dir())
            .collect();
        let index = roots
            .iter()
            .find_map(|r| std::fs::read_to_string(r.join("index.theme")).ok())?;
        Some(Theme::parse(&index, roots))
    }

    fn parse(index: &str, roots: Vec<PathBuf>) -> Theme {
        let list = |key: &str| -> Vec<String> {
            ini_value(index, "Icon Theme", key)
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut dirs = list("Directories");
        dirs.extend(list("ScaledDirectories"));
        let num = |sec: &str, key: &str| ini_value(index, sec, key).and_then(|v| v.parse().ok());
        let dirs = dirs
            .into_iter()
            .filter_map(|path| {
                let size: u32 = num(&path, "Size")?;
                let scale: u32 = num(&path, "Scale").unwrap_or(1).max(1);
                let kind = match ini_value(index, &path, "Type").as_deref() {
                    Some("Fixed") => DirType::Fixed,
                    Some("Scalable") => DirType::Scalable {
                        min: num(&path, "MinSize").unwrap_or(size),
                        max: num(&path, "MaxSize").unwrap_or(size),
                    },
                    _ => DirType::Threshold(num(&path, "Threshold").unwrap_or(2)),
                };
                Some(SubDir {
                    path,
                    px: size * scale,
                    scale,
                    kind,
                })
            })
            .collect();
        Theme {
            roots,
            dirs,
            inherits: list("Inherits"),
        }
    }

    /// `name`'s file in this theme for `px` pixels: one made for that size, else the nearest.
    fn find(&self, name: &str, px: u32) -> Option<PathBuf> {
        let file = |dir: &SubDir| {
            self.roots.iter().find_map(|root| {
                ["png", "svg"].iter().find_map(|ext| {
                    let p = root.join(&dir.path).join(format!("{name}.{ext}"));
                    p.is_file().then_some(p)
                })
            })
        };
        if let Some(p) = self.dirs.iter().filter(|d| d.matches(px)).find_map(file) {
            return Some(p);
        }
        let mut best: Option<(u32, PathBuf)> = None;
        for dir in &self.dirs {
            let d = dir.distance(px);
            if best.as_ref().is_none_or(|(b, _)| d < *b)
                && let Some(p) = file(dir)
            {
                best = Some((d, p));
            }
        }
        best.map(|(_, p)| p)
    }
}

/// The icon themes to search, in order.
pub struct Icons {
    themes: Vec<Theme>,
}

impl Icons {
    /// `themes` (the desktop's first) with what each inherits, then hicolor last, as found under
    /// `bases`. A theme that isn't installed is skipped.
    pub fn new(bases: &[PathBuf], themes: &[&str]) -> Self {
        let mut order: Vec<String> = Vec::new();
        let mut loaded = Vec::new();
        let mut queue: Vec<String> = themes.iter().map(|t| t.to_string()).collect();
        queue.reverse();
        while let Some(name) = queue.pop() {
            if name == "hicolor" || order.contains(&name) || order.len() > 32 {
                continue;
            }
            order.push(name.clone());
            if let Some(t) = Theme::load(bases, &name) {
                // Depth first: a theme's parents come right after it.
                queue.extend(t.inherits.iter().rev().cloned());
                loaded.push(t);
            }
        }
        loaded.extend(Theme::load(bases, "hicolor"));
        Icons { themes: loaded }
    }

    /// The first of `names` that a theme has, theme by theme (the desktop's own generic icon
    /// before another theme's specific one, as GTK does), for `px` pixels.
    pub fn find(&self, names: &[String], px: u32) -> Option<PathBuf> {
        self.themes
            .iter()
            .find_map(|t| names.iter().find_map(|n| t.find(n, px)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn the_folders_come_from_the_environment() {
        let d = Dirs::new(Some("/home/u"), None, None);
        assert_eq!(
            d.data,
            ["/home/u/.local/share", "/usr/local/share", "/usr/share"].map(PathBuf::from)
        );
        assert_eq!(d.icons[0], PathBuf::from("/home/u/.icons"));
        assert_eq!(d.icons[3], PathBuf::from("/usr/share/icons"));
        let d = Dirs::new(Some("/h"), Some("/x"), Some("/a:relative:/b"));
        assert_eq!(d.data, ["/x", "/a", "/b"].map(PathBuf::from));
    }

    #[test]
    fn ini_values_by_section() {
        let t = "[General]\nTheme=wrong\n[Icons]\n Theme = Papirus-Dark \n";
        assert_eq!(
            ini_value(t, "Icons", "Theme").as_deref(),
            Some("Papirus-Dark")
        );
        assert_eq!(ini_value(t, "Icons", "Size"), None);
    }

    #[test]
    fn globs_match_as_shells_do() {
        assert!(glob_match("*.pdf", "x.pdf"));
        assert!(!glob_match("*.pdf", "x.pdfx"));
        assert!(glob_match("*.tar.gz", "a.tar.gz"));
        assert!(glob_match("readme*", "readme.md"));
        assert!(glob_match("*.[1-9]", "ls.1"));
        assert!(!glob_match("*.[!1-9]", "ls.1"));
        assert!(glob_match("?.c", "a.c"));
    }

    #[test]
    fn a_files_type_and_its_icon_names() {
        let tmp = tempfile::tempdir().unwrap();
        let sys = tmp.path().join("sys");
        let user = tmp.path().join("user");
        write(
            &sys,
            "mime/globs2",
            "# comment\n50:application/pdf:*.pdf\n50:application/gzip:*.gz\n\
             50:application/x-compressed-tar:*.tar.gz\n50:video/mp4:*.mp4\n\
             50:text/x-csrc:*.c:cs\n50:text/x-c++src:*.C:cs\n10:text/plain:*.txt\n",
        );
        write(
            &sys,
            "mime/generic-icons",
            "application/pdf:x-office-document\n",
        );
        write(&sys, "mime/icons", "video/mp4:video-mp4-special\n");
        // The user's own: .txt is theirs now.
        write(&user, "mime/globs2", "60:text/x-mine:*.txt\n");
        let db = MimeDb::load(&[user, sys]);
        assert_eq!(db.mime_for("file.PDF"), Some("application/pdf"));
        assert_eq!(
            db.mime_for("a.tar.gz"),
            Some("application/x-compressed-tar")
        );
        assert_eq!(db.mime_for("a.gz"), Some("application/gzip"));
        assert_eq!(db.mime_for("x.c"), Some("text/x-csrc"));
        assert_eq!(db.mime_for("x.C"), Some("text/x-c++src"));
        assert_eq!(db.mime_for("notes.txt"), Some("text/x-mine"));
        assert_eq!(db.mime_for("file.xyz"), None);
        assert_eq!(
            db.icon_names("application/pdf"),
            ["application-pdf", "x-office-document"]
        );
        assert_eq!(
            db.icon_names("video/mp4"),
            ["video-mp4-special", "video-mp4", "video-x-generic"]
        );
        assert_eq!(
            db.icon_names("application/octet-stream"),
            ["application-octet-stream", "application-x-generic"]
        );
    }

    #[test]
    fn icons_are_found_in_the_theme_its_parents_then_hicolor_by_size() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("icons");
        write(
            &base,
            "Mine/index.theme",
            "[Icon Theme]\nName=Mine\nInherits=Parent\nDirectories=16x16/mimetypes,48x48/mimetypes,scalable/mimetypes\n\
             ScaledDirectories=16x16@2/mimetypes\n\
             [16x16/mimetypes]\nSize=16\nType=Fixed\n\
             [16x16@2/mimetypes]\nSize=16\nScale=2\nType=Fixed\n\
             [48x48/mimetypes]\nSize=48\nType=Threshold\n\
             [scalable/mimetypes]\nSize=64\nType=Scalable\nMinSize=64\nMaxSize=256\n",
        );
        write(&base, "Mine/16x16/mimetypes/application-pdf.png", "png");
        write(&base, "Mine/16x16@2/mimetypes/application-pdf.png", "png");
        write(&base, "Mine/48x48/mimetypes/application-pdf.png", "png");
        write(
            &base,
            "Mine/scalable/mimetypes/application-pdf.svg",
            "<svg/>",
        );
        write(
            &base,
            "Parent/index.theme",
            "[Icon Theme]\nDirectories=32x32/mimetypes\n[32x32/mimetypes]\nSize=32\nType=Fixed\n",
        );
        write(&base, "Parent/32x32/mimetypes/x-office-document.png", "png");
        write(&base, "Parent/32x32/mimetypes/video-x-generic.png", "png");
        write(
            &base,
            "hicolor/index.theme",
            "[Icon Theme]\nDirectories=48x48/mimetypes\n[48x48/mimetypes]\nSize=48\nType=Fixed\n",
        );
        write(&base, "hicolor/48x48/mimetypes/video-mp4.png", "png");
        write(
            &base,
            "hicolor/48x48/mimetypes/application-x-generic.png",
            "png",
        );
        let icons = Icons::new(std::slice::from_ref(&base), &["Missing", "Mine"]);
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let rel = |p: Option<PathBuf>| {
            p.map(|p| {
                p.strip_prefix(&base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
        };
        let pdf = names(&["application-pdf", "x-office-document"]);
        // Made for the size: 16, 16 at scale 2 for 32 pixels, 48 within its threshold (46), the
        // scalable one from 64 up; the nearest otherwise (24 → 16).
        assert_eq!(
            rel(icons.find(&pdf, 16)).unwrap(),
            "Mine/16x16/mimetypes/application-pdf.png"
        );
        assert_eq!(
            rel(icons.find(&pdf, 32)).unwrap(),
            "Mine/16x16@2/mimetypes/application-pdf.png"
        );
        assert_eq!(
            rel(icons.find(&pdf, 46)).unwrap(),
            "Mine/48x48/mimetypes/application-pdf.png"
        );
        assert_eq!(
            rel(icons.find(&pdf, 128)).unwrap(),
            "Mine/scalable/mimetypes/application-pdf.svg"
        );
        assert_eq!(
            rel(icons.find(&pdf, 26)).unwrap(),
            "Mine/16x16@2/mimetypes/application-pdf.png"
        );
        // The desktop's theme (through its parent) before hicolor's more specific icon.
        let mp4 = names(&["video-mp4", "video-x-generic"]);
        assert_eq!(
            rel(icons.find(&mp4, 32)).unwrap(),
            "Parent/32x32/mimetypes/video-x-generic.png"
        );
        // Only hicolor has it.
        let bin = names(&["application-octet-stream", "application-x-generic"]);
        assert_eq!(
            rel(icons.find(&bin, 16)).unwrap(),
            "hicolor/48x48/mimetypes/application-x-generic.png"
        );
        assert_eq!(icons.find(&names(&["nothing-here"]), 16), None);
        // No theme at all.
        assert_eq!(
            Icons::new(&[tmp.path().join("none")], &["Mine"]).find(&pdf, 16),
            None
        );
        // A theme name can't leave the base folder.
        assert!(Theme::load(std::slice::from_ref(&base), "../icons/Mine").is_none());
    }
}
