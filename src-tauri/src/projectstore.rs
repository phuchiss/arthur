use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One entry in the user's recent-projects list. `path` is canonicalized
/// (trailing separator stripped) so two opens of the same dir don't dupe.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecentProject {
    pub path: String,
    pub last_opened_at: u64,
}

/// Top-level wrapper mirrors `chatstore`'s layout so we can add fields later
/// (e.g. pinned projects) without a JSON migration.
#[derive(Serialize, Deserialize, Default)]
struct ProjectsFile {
    #[serde(default)]
    recents: Vec<RecentProject>,
}

const MAX_RECENTS: usize = 20;

fn projects_file(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("projects.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Strip a trailing path separator so `/foo` and `/foo/` dedupe.
fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        path.to_string()
    } else {
        trimmed.to_string()
    }
}

fn read_all(app_data_dir: &Path) -> ProjectsFile {
    std::fs::read_to_string(projects_file(app_data_dir))
        .ok()
        .and_then(|s| serde_json::from_str::<ProjectsFile>(&s).ok())
        .unwrap_or_default()
}

fn write_all(app_data_dir: &Path, all: &ProjectsFile) {
    if std::fs::create_dir_all(app_data_dir).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_string_pretty(all) {
        let _ = std::fs::write(projects_file(app_data_dir), json);
    }
}

/// Recent projects, newest first, with missing directories filtered out.
/// Missing entries are kept on disk (the user may have an unmounted drive); we
/// just hide them from the UI.
pub fn list(app_data_dir: &Path) -> Vec<RecentProject> {
    let mut all = read_all(app_data_dir);
    all.recents
        .sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
    all.recents
        .into_iter()
        .filter(|e| Path::new(&e.path).is_dir())
        .collect()
}

/// Upsert by normalized path, bump `last_opened_at`, cap to `MAX_RECENTS`.
pub fn add(app_data_dir: &Path, path: &str) {
    let path = normalize(path);
    if path.is_empty() {
        return;
    }
    let mut all = read_all(app_data_dir);
    let now = now_secs();
    if let Some(existing) = all.recents.iter_mut().find(|e| e.path == path) {
        existing.last_opened_at = now;
    } else {
        all.recents.push(RecentProject {
            path,
            last_opened_at: now,
        });
    }
    all.recents
        .sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
    all.recents.truncate(MAX_RECENTS);
    write_all(app_data_dir, &all);
}

pub fn remove(app_data_dir: &Path, path: &str) {
    let path = normalize(path);
    let mut all = read_all(app_data_dir);
    all.recents.retain(|e| e.path != path);
    write_all(app_data_dir, &all);
}

pub fn clear(app_data_dir: &Path) {
    write_all(app_data_dir, &ProjectsFile::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arthur-projectstore-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn add_then_list_returns_entry() {
        let data = temp_data_dir();
        let project = data.join("proj-a");
        std::fs::create_dir_all(&project).unwrap();

        add(&data, project.to_str().unwrap());
        let entries = list(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, project.to_string_lossy());
        assert!(entries[0].last_opened_at > 0);

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn add_dedupes_by_path_and_normalizes_trailing_slash() {
        let data = temp_data_dir();
        let project = data.join("proj-b");
        std::fs::create_dir_all(&project).unwrap();
        let p = project.to_string_lossy().to_string();
        let with_slash = format!("{p}/");

        add(&data, &p);
        add(&data, &with_slash);
        let entries = list(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, p);

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn list_filters_out_missing_directories() {
        let data = temp_data_dir();
        let kept = data.join("kept");
        let gone = data.join("gone");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::create_dir_all(&gone).unwrap();

        add(&data, kept.to_str().unwrap());
        add(&data, gone.to_str().unwrap());
        std::fs::remove_dir_all(&gone).unwrap();

        let entries = list(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, kept.to_string_lossy());

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn add_caps_at_max_recents() {
        let data = temp_data_dir();
        let mut paths = Vec::new();
        for i in 0..(MAX_RECENTS + 5) {
            let p = data.join(format!("proj-{i}"));
            std::fs::create_dir_all(&p).unwrap();
            paths.push(p.to_string_lossy().to_string());
            add(&data, paths.last().unwrap());
        }

        let all = read_all(&data);
        assert_eq!(all.recents.len(), MAX_RECENTS);
        // The most recently added entries survive — newest paths come first.
        assert_eq!(all.recents[0].path, *paths.last().unwrap());

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn remove_and_clear() {
        let data = temp_data_dir();
        let a = data.join("a");
        let b = data.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        add(&data, a.to_str().unwrap());
        add(&data, b.to_str().unwrap());
        assert_eq!(list(&data).len(), 2);

        remove(&data, a.to_str().unwrap());
        let entries = list(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, b.to_string_lossy());

        clear(&data);
        assert!(list(&data).is_empty());

        let _ = std::fs::remove_dir_all(&data);
    }
}
