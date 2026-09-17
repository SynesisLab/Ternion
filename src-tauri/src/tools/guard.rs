//! Windows path guard (design §6.3) — the single choke point every FS tool
//! passes through before touching the disk. A path is usable only if it
//! canonicalizes to something strictly inside one of the conversation's bound
//! workspace roots.
//!
//! Defenses, in order:
//! 1. lexical: control chars, wildcards, alternate-data-stream colons,
//!    Windows-reserved names, trailing dots/spaces (Win32 would silently
//!    strip those, so the path the model sees ≠ the path we'd act on);
//! 2. canonicalization via `\\?\` verbatim paths — resolves `..`, symlinks
//!    and junctions to their real target, so prefix checking sees the truth;
//! 3. case-insensitive (NTFS) prefix check against each canonical root,
//!    requiring an exact match or a separator boundary.

use std::fmt;
use std::path::{Path, PathBuf};

/// A path the guard has approved: where it canonicalized to, and which
/// bound root it lives under (the §6.6 permission matrix keys on the root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Canonical `\\?\`-prefixed absolute path — what FS calls should use.
    pub path: PathBuf,
    /// The canonical root this path lives under.
    pub workspace: PathBuf,
}

/// Model-facing guard failures (messages are fed back as tool errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    Empty,
    Invalid(String),
    /// Exists but under no bound root.
    Outside(Vec<String>),
    /// Relative path resolves under more than one root.
    Ambiguous(Vec<String>),
    /// Doesn't exist (or a bound root vanished mid-check).
    NotFound,
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "path is empty"),
            Self::Invalid(msg) => write!(f, "invalid path: {msg}"),
            Self::Outside(roots) => write!(
                f,
                "path is outside the bound workspaces (allowed roots: {})",
                if roots.is_empty() {
                    "none — no workspace is bound to this chat".to_string()
                } else {
                    roots.join(", ")
                }
            ),
            Self::Ambiguous(roots) => write!(
                f,
                "relative path matches more than one workspace ({}); pass an absolute path",
                roots.join(", ")
            ),
            Self::NotFound => write!(f, "path not found"),
        }
    }
}

/// Windows names the Win32 layer refuses to create (any file whose name
/// before the first dot is one of these).
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Resolve `raw` (model-supplied) against the conversation's bound roots.
/// Absolute paths (drive or UNC) must land inside a root; relative paths
/// resolve against every root — one match passes, zero is NotFound, several
/// is Ambiguous (the model is told to pass an absolute path).
pub fn resolve(raw: &str, roots: &[PathBuf]) -> Result<ResolvedPath, GuardError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(GuardError::Empty);
    }
    if raw.chars().any(|c| (c as u32) < 0x20) {
        return Err(GuardError::Invalid(
            "control characters are not allowed".into(),
        ));
    }
    // The verbatim prefix is infrastructure, not a path component ("?" would
    // fail the lexical check); the device form `\\?\UNC\server\share`
    // becomes `UNC\server\share` here, still split-checked per component.
    let lex_raw = strip_verbatim(raw);
    check_components(lex_raw)?;

    // A single leading separator is drive-relative (`\foo` resolves against
    // the current drive) — a classic escape hatch; refuse it outright.
    let drive_relative =
        (raw.starts_with('/') || raw.starts_with('\\')) && !raw.starts_with("\\\\");
    if drive_relative {
        return Err(GuardError::Invalid(
            "device-relative paths (leading backslash) are not supported; \
             use an absolute path inside a workspace"
                .into(),
        ));
    }

    let absolute = raw.len() >= 2 && raw.as_bytes()[1] == b':';
    if absolute || raw.starts_with("\\\\") {
        // UNC or drive-absolute: canonicalize, then find the owning root.
        let canonical = std::fs::canonicalize(raw).map_err(|_| GuardError::NotFound)?;
        let owner = find_root(&canonical, roots)?;
        match owner {
            Some(workspace) => Ok(ResolvedPath {
                path: canonical,
                workspace,
            }),
            None => Err(GuardError::Outside(
                roots.iter().map(display_root).collect(),
            )),
        }
    } else {
        let mut matches: Vec<ResolvedPath> = Vec::new();
        let mut not_found = 0usize;
        let mut outside: Vec<String> = Vec::new();
        for root in roots {
            // Roots are stored plain (dunce form); canonicalize before the
            // prefix check so spellings always match the target's verbatim
            // canonical form.
            let root_canonical = match std::fs::canonicalize(root) {
                Ok(rc) => rc,
                Err(_) => {
                    not_found += 1;
                    continue;
                }
            };
            match std::fs::canonicalize(root.join(raw)) {
                Ok(path) => {
                    if path_prefix_root(&path, &root_canonical).is_some() {
                        // Report the root as stored (plain form).
                        matches.push(ResolvedPath {
                            path,
                            workspace: root.clone(),
                        });
                    } else {
                        // Canonicalized outside the root (reparse escape).
                        outside.push(display_root(root));
                    }
                }
                Err(_) => not_found += 1,
            }
        }
        match matches.len() {
            0 if roots.is_empty() => Err(GuardError::Outside(Vec::new())),
            0 if not_found == roots.len() => Err(GuardError::NotFound),
            0 => Err(GuardError::Outside(outside)),
            1 => Ok(matches.into_iter().next().expect("len == 1")),
            _ => Err(GuardError::Ambiguous(
                roots.iter().map(display_root).collect(),
            )),
        }
    }
}

/// Canonicalize + validate a workspace root at bind time: absolute, exists,
/// is a directory. Returns the canonical path the guard compares against.
pub fn normalize_root(raw: &str) -> Result<PathBuf, GuardError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(GuardError::Empty);
    }
    if raw.chars().any(|c| (c as u32) < 0x20) {
        return Err(GuardError::Invalid(
            "control characters are not allowed".into(),
        ));
    }
    let is_unc = raw.starts_with("\\\\");
    let absolute = raw.len() >= 2 && raw.as_bytes()[1] == b':';
    if !(absolute || is_unc) || (raw.starts_with('/') || raw.starts_with('\\')) {
        return Err(GuardError::Invalid(
            "workspace roots must be absolute paths (e.g. D:\\client-work)".into(),
        ));
    }
    let canonical = std::fs::canonicalize(raw).map_err(|_| GuardError::NotFound)?;
    if !canonical.is_dir() {
        return Err(GuardError::Invalid(
            "workspace root must be an existing folder".into(),
        ));
    }
    // Store the root in plain (dunce) form: verbatim `\\?\` paths skip Win32
    // normalization, so joining relative paths with `..` against a verbatim
    // root would not resolve as expected.
    Ok(dunce(&canonical))
}

/// `\\?\C:\a\b` → `C:\a\b`, `\\?\UNC\srv\share` → `\\srv\share`. The
/// canonical form is still fully resolved (no `..`, no symlink gaps).
fn dunce(path: &std::path::Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        s.to_string().into()
    }
}

/// Drop the `\\?\` / `\\?\UNC\` prefix for lexical (per-component) checks.
fn strip_verbatim(raw: &str) -> &str {
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        rest
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest
    } else {
        raw
    }
}

/// Reserved names, trailing dots/spaces, wildcards, ADS colons — per
/// component, on the model-supplied spelling (canonicalization can't run
/// until the path exists, and Win32 silently rewrites the bad ones).
fn check_components(raw: &str) -> Result<(), GuardError> {
    for comp in raw.split(['/', '\\']) {
        if comp.is_empty() || comp == "." || comp == ".." {
            continue;
        }
        // Drive component, e.g. "C:".
        if comp.len() == 2 && comp.as_bytes()[1] == b':' {
            if !comp.as_bytes()[0].is_ascii_alphabetic() {
                return Err(GuardError::Invalid(format!("bad drive `{comp}`")));
            }
            continue;
        }
        if comp.contains(':') {
            return Err(GuardError::Invalid(format!(
                "`{comp}`: ':' is not allowed in file names (alternate data streams)"
            )));
        }
        if comp.contains(['<', '>', '"', '|', '?', '*']) {
            return Err(GuardError::Invalid(format!(
                "`{comp}` contains a character Windows does not allow in file names"
            )));
        }
        if comp.ends_with('.') || comp.ends_with(' ') {
            return Err(GuardError::Invalid(format!(
                "`{comp}`: Windows strips trailing dots and spaces — name it without them"
            )));
        }
        // "CON.txt" is as reserved as "CON": the device name is the part
        // before the first dot.
        let stem = match comp.split_once('.') {
            Some((stem, _)) => stem,
            None => comp,
        };
        if RESERVED_NAMES
            .iter()
            .any(|name| stem.eq_ignore_ascii_case(name))
        {
            return Err(GuardError::Invalid(format!(
                "`{comp}` is a Windows-reserved device name"
            )));
        }
    }
    Ok(())
}

/// The stored root that contains `path` (returned in stored, plain form).
fn find_root(path: &Path, roots: &[PathBuf]) -> Result<Option<PathBuf>, GuardError> {
    for root in roots {
        let canonical = std::fs::canonicalize(root).map_err(|_| GuardError::NotFound)?;
        if path_prefix_root(path, &canonical).is_some() {
            return Ok(Some(root.clone()));
        }
    }
    Ok(None)
}

/// Case-insensitive prefix containment over canonical text paths. The char
/// right after the root prefix must be a separator (so `C:\foo` does not
/// contain `C:\foobar`) or the paths must be equal (a root itself).
fn path_prefix_root(path: &Path, root: &Path) -> Option<()> {
    let p = path.to_str()?;
    let r = root.to_str()?;
    let p = p.trim_end_matches('\\').to_ascii_lowercase();
    let r = r.trim_end_matches('\\').to_ascii_lowercase();
    if p == r || (p.len() > r.len() && p.as_bytes()[r.len()] == b'\\' && p.starts_with(&r)) {
        Some(())
    } else {
        None
    }
}

fn display_root(root: &PathBuf) -> String {
    root.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(dir: &std::path::Path) -> PathBuf {
        std::fs::canonicalize(dir).unwrap()
    }

    /// Roots as stored on a conversation: plain (dunce) canonical form.
    fn root_of(dir: &std::path::Path) -> PathBuf {
        dunce(&canonical(dir))
    }

    fn touch(dir: &std::path::Path, rel: &str) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "x").unwrap();
        p
    }

    #[test]
    fn absolute_inside_root_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        touch(dir.path(), "notes/todo.txt");
        let r = resolve(
            &dir.path().join("notes").join("todo.txt").display().to_string(),
            &[root.clone()],
        )
        .unwrap();
        assert_eq!(r.workspace, root);
    }

    #[test]
    fn relative_resolves_under_single_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        touch(dir.path(), "src/main.rs");
        let r = resolve("src/main.rs", &[root.clone()]).unwrap();
        assert!(r.path.ends_with("main.rs"));
        assert_eq!(r.workspace, root);
    }

    #[test]
    fn case_insensitive_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        touch(dir.path(), "a.txt");
        // Swap case on every letter of the canonical root.
        let flipped: String = root
            .display()
            .to_string()
            .chars()
            .map(|c| if c.is_ascii_alphabetic() {
                if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }
            } else {
                c
            })
            .collect();
        assert!(resolve(&flipped, &[root]).is_ok(), "NTFS is case-insensitive");
    }

    #[test]
    fn parent_stays_inside_root_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        touch(dir.path(), "sub/deep.txt");
        // `sub/other/../deep.txt` lexically resolves to `sub/deep.txt`,
        // which exists inside the root — must pass.
        assert!(
            resolve("sub/other/../deep.txt", &[root]).is_ok(),
            "resolves back inside"
        );
    }

    #[test]
    fn absolute_outside_root_denied() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        let windir = std::env::var("WINDIR").expect("WINDIR on windows");
        match resolve(&windir, &[root]) {
            Err(GuardError::Outside(_)) => {}
            other => panic!("C:\\Windows must be outside: {other:?}"),
        }
    }

    #[test]
    fn no_roots_means_outside() {
        match resolve("anything.txt", &[]) {
            Err(GuardError::Outside(_)) => {}
            other => panic!("no workspaces ⇒ no access: {other:?}"),
        }
    }

    #[test]
    fn ambiguous_relative_two_roots() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let ra = root_of(a.path());
        let rb = root_of(b.path());
        touch(a.path(), "same.txt");
        touch(b.path(), "same.txt");
        match resolve("same.txt", &[ra, rb]) {
            Err(GuardError::Ambiguous(_)) => {}
            other => panic!("two matches must be ambiguous: {other:?}"),
        }
    }

    #[test]
    fn lexical_defenses() {
        let cases = [
            ("CON", "reserved"),
            ("COM1", "reserved"),
            ("CON.txt", "reserved with extension"),
            ("aux.log", "reserved with extension"),
            ("foo.", "trailing dot"),
            ("foo ", "trailing space"),
            ("a*b", "wildcard"),
            ("file.txt:ads", "alternate data stream"),
            ("a\u{1}b", "control char"),
            (r"\foo", "device-relative"),
            ("/foo", "device-relative"),
            ("sub:dir", "colon mid-path"),
        ];
        for (raw, why) in cases {
            assert!(resolve(raw, &[PathBuf::from("C:\\nowhere")]).is_err(), "{why}: {raw}");
        }
    }

    #[test]
    fn normalize_root_checks_dir() {
        let dir = tempfile::tempdir().unwrap();
        let file = touch(dir.path(), "file.txt");
        assert!(normalize_root(&file.display().to_string()).is_err(), "file is not a root");
        assert!(normalize_root("relative/path").is_err(), "must be absolute");
        assert!(normalize_root("").is_err());
        let ok = normalize_root(&dir.path().display().to_string()).unwrap();
        assert_eq!(ok, root_of(dir.path()), "stored roots are the plain canonical form");
    }

    #[test]
    fn prefix_boundary_does_not_swallow_siblings() {
        // `C:\foo` does not contain `C:\foobar` — the boundary char matters.
        assert!(path_prefix_root(
            &PathBuf::from(r"\\?\C:\foobar\f.txt"),
            &PathBuf::from(r"\\?\C:\foo"),
        )
        .is_none());
        assert!(
            path_prefix_root(&PathBuf::from(r"\\?\C:\foo\f.txt"), &PathBuf::from(r"\\?\C:\foo"))
                .is_some()
        );
        // The root itself counts (fs_list on the bound folder).
        assert!(
            path_prefix_root(&PathBuf::from(r"\\?\C:\foo"), &PathBuf::from(r"\\?\C:\foo"))
                .is_some()
        );
    }

    /// Reparse escape: a junction inside the root pointing elsewhere must
    /// resolve OUTSIDE and be rejected. Skips if junction creation isn't
    /// available in the test environment.
    #[test]
    fn reparse_escape_denied() {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        std::fs::write(other.path().join("leak.txt"), "x").unwrap();

        let ok = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(dir.path().join("sub"))
            .arg(other.path())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return; // junction unavailable in this environment
        }

        match resolve("sub/leak.txt", &[root]) {
            Err(GuardError::Outside(_) | GuardError::NotFound) => {}
            other => panic!("junction escape must be rejected: {other:?}"),
        }
    }
}