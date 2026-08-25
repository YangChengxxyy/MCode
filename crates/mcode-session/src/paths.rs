//! Session file locations (design doc `01-agent-core.md` §4):
//! `~/.mcode/sessions/<cwd-slug>/<timestamp>_<uuid>.jsonl`, with the
//! `$MCODE_HOME` environment variable overriding the root.
//!
//! Every rule exists twice: a pure `*_from` variant taking the home (or
//! sessions) root explicitly — unit-testable without touching process
//! environment — and a convenience wrapper reading the environment.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use mcode_core::McodeError;
use mcode_core::ids::SessionId;

use crate::store::SessionHeader;

/// Resolve the MCode home directory from explicit environment values:
/// `$MCODE_HOME` wins when set and non-empty, otherwise `$HOME/.mcode`.
pub fn home_from(mcode_home: Option<OsString>, home: Option<OsString>) -> PathBuf {
    if let Some(dir) = mcode_home.filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    match home.filter(|value| !value.is_empty()) {
        Some(home) => PathBuf::from(home).join(".mcode"),
        None => PathBuf::from(".mcode"),
    }
}

/// The MCode home directory: `$MCODE_HOME` if set (and non-empty),
/// otherwise `~/.mcode`.
pub fn mcode_home() -> PathBuf {
    home_from(std::env::var_os("MCODE_HOME"), std::env::var_os("HOME"))
}

/// `<home>/sessions`.
pub fn sessions_root_from(home: &Path) -> PathBuf {
    home.join("sessions")
}

/// `<mcode_home>/sessions`.
pub fn sessions_root() -> PathBuf {
    sessions_root_from(&mcode_home())
}

/// Slugify a working directory: `/` becomes `-`, leading/trailing
/// dashes are trimmed, characters hostile to file names become `_`,
/// and the root path degenerates to `"root"`.
///
/// `/Users/cc/projects/MCode` → `Users-cc-projects-MCode`.
pub fn cwd_slug(cwd: &Path) -> String {
    let slugged = cwd
        .to_string_lossy()
        .replace('/', "-")
        .trim_matches('-')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    if slugged.is_empty() {
        "root".into()
    } else {
        slugged
    }
}

/// `<root>/<cwd-slug>` (the per-project session directory).
pub fn session_dir_from(root: &Path, cwd: &Path) -> PathBuf {
    root.join(cwd_slug(cwd))
}

/// `<mcode_home>/sessions/<cwd-slug>`.
pub fn session_dir(cwd: &Path) -> PathBuf {
    session_dir_from(&sessions_root(), cwd)
}

/// A session file name for `now`: `<timestamp>_<uuid>.jsonl`. The
/// timestamp prefix (`%Y%m%dT%H%M%S`) makes lexicographic order match
/// creation order, so plain sorting finds the latest session.
pub fn session_file_name(now: DateTime<Utc>) -> String {
    format!(
        "{}_{}.jsonl",
        now.format("%Y%m%dT%H%M%S"),
        uuid::Uuid::new_v4().simple()
    )
}

/// A fresh session file path for `cwd`:
/// `~/.mcode/sessions/<cwd-slug>/<timestamp>_<uuid>.jsonl` (respecting
/// `$MCODE_HOME`).
pub fn new_session_file(cwd: &Path) -> PathBuf {
    session_dir(cwd).join(session_file_name(Utc::now()))
}

/// List the `.jsonl` session files under a session directory, sorted
/// (chronological by the timestamp-prefixed naming scheme).
fn sorted_session_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    files
}

/// The most recently created session file for `cwd` under `root`
/// (lexicographically last name; `mcode resume latest` support).
pub fn latest_session_file_from(root: &Path, cwd: &Path) -> Option<PathBuf> {
    sorted_session_files(&session_dir_from(root, cwd)).pop()
}

/// Read a session file's header without parsing the whole log.
fn read_header(path: &Path) -> Result<SessionHeader, McodeError> {
    let file = File::open(path)
        .map_err(|err| McodeError::Session(format!("cannot open {}: {err}", path.display())))?;
    let mut line = String::new();
    let mut reader = BufReader::new(file);
    reader
        .read_line(&mut line)
        .map_err(|err| McodeError::Session(format!("cannot read {}: {err}", path.display())))?;
    SessionHeader::from_line(line.trim())
}

/// Find the session file whose header carries `id`, searching
/// `<root>/*/*.jsonl`.
pub fn find_session_by_id(root: &Path, id: &SessionId) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let Ok(slug_dirs) = std::fs::read_dir(root) else {
        return None;
    };
    for slug_dir in slug_dirs.flatten() {
        candidates.extend(sorted_session_files(&slug_dir.path()));
    }
    candidates.sort();
    candidates
        .into_iter()
        .find(|path| read_header(path).is_ok_and(|header| &header.session_id == id))
}

/// Resolve a `path-or-id` session specifier against an explicit root:
/// an existing file path wins, otherwise the string is treated as a
/// session id and searched for under `root`.
pub fn resolve_session_from(root: &Path, spec: &str) -> Option<PathBuf> {
    let as_path = Path::new(spec);
    if as_path.is_file() {
        return Some(as_path.to_path_buf());
    }
    find_session_by_id(root, &SessionId::from(spec))
}

/// Resolve a `path-or-id` session specifier against the real session
/// root (`~/.mcode/sessions`, `$MCODE_HOME` override).
pub fn resolve_session(spec: &str) -> Option<PathBuf> {
    resolve_session_from(&sessions_root(), spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_resolution_prefers_mcode_home_override() {
        assert_eq!(
            home_from(Some("/tmp/x".into()), Some("/u/cc".into())),
            PathBuf::from("/tmp/x")
        );
        // Empty override falls through to $HOME.
        assert_eq!(
            home_from(Some(String::new().into()), Some("/u/cc".into())),
            PathBuf::from("/u/cc/.mcode")
        );
        assert_eq!(
            home_from(None, Some("/u/cc".into())),
            PathBuf::from("/u/cc/.mcode")
        );
        assert_eq!(home_from(None, None), PathBuf::from(".mcode"));
    }

    #[test]
    fn cwd_slug_rules() {
        assert_eq!(
            cwd_slug(Path::new("/Users/cc/projects/MCode")),
            "Users-cc-projects-MCode"
        );
        assert_eq!(cwd_slug(Path::new("/")), "root");
        assert_eq!(cwd_slug(Path::new("/a/b/")), "a-b");
        assert_eq!(cwd_slug(Path::new("/weird name+dir")), "weird_name_dir");
    }

    #[test]
    fn session_file_name_shape_and_order() {
        let earlier = session_file_name(Utc::now());
        let later = session_file_name(Utc::now());
        for name in [&earlier, &later] {
            assert!(name.ends_with(".jsonl"), "{name}");
            assert_eq!(name.matches('_').count(), 1, "{name}");
            assert!(
                name[..15].chars().all(|c| c.is_ascii_digit() || c == 'T'),
                "timestamp prefix must sort chronologically: {name}"
            );
        }
        // Distinct uuids even within the same second.
        assert_ne!(earlier, later);
    }

    #[test]
    fn session_dir_from_nests_under_slug() {
        assert_eq!(
            session_dir_from(
                Path::new("/data/mcode/sessions"),
                Path::new("/Users/cc/app")
            ),
            PathBuf::from("/data/mcode/sessions/Users-cc-app")
        );
    }
}
