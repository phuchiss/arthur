//! Thin `git` wrappers backing the files panel. No Tauri imports — Tauri-side
//! callers live in `commands.rs`. We shell out to `git` rather than pull in a
//! libgit dependency: it works for every project Arthur already knows about
//! and matches what the user would see in their own terminal.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

const PREVIEW_MAX_BYTES: usize = 1024 * 1024;
const IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;
const DIFF_MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHANGED: usize = 500;
const MAX_ALL: usize = 10_000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    /// Tracked file with no change relative to the baseline. Only produced by
    /// `all_files` (the "All" view); `changed_files` never emits it.
    Unchanged,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFilesResult {
    pub files: Vec<ChangedFile>,
    pub git_available: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilePreview {
    pub content: String,
    pub truncated: bool,
    pub binary: bool,
    /// `data:<mime>;base64,…` URL set only for recognised image files small
    /// enough to inline; `None` for everything else.
    pub image: Option<String>,
}

/// True if `project_dir` is inside a git working tree.
pub fn is_git_repo(project_dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_dir)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Resolve the current HEAD commit sha for the project. Used as the baseline
/// captured the first time the panel is opened in a session.
pub fn snapshot_head(project_dir: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("git rev-parse failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse HEAD: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// All files that have changed in the working tree relative to `baseline`.
/// Combines tracked modifications (`git diff --name-status -z`) with untracked
/// files (`git ls-files --others`).
pub fn changed_files(project_dir: &Path, baseline: &str) -> Result<ChangedFilesResult, String> {
    if !is_git_repo(project_dir) {
        return Ok(ChangedFilesResult {
            files: Vec::new(),
            git_available: false,
            truncated: false,
        });
    }

    let mut files: Vec<ChangedFile> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let tracked = Command::new("git")
        .args(["diff", "--name-status", "-z", baseline])
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("git diff failed: {e}"))?;
    if tracked.status.success() {
        for cf in parse_name_status_z(&tracked.stdout) {
            if seen.insert(cf.path.clone()) {
                files.push(cf);
            }
        }
    }

    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("git ls-files failed: {e}"))?;
    if untracked.status.success() {
        for raw in untracked.stdout.split(|b| *b == 0) {
            if raw.is_empty() {
                continue;
            }
            let path = String::from_utf8_lossy(raw).to_string();
            if seen.insert(path.clone()) {
                files.push(ChangedFile {
                    path,
                    status: FileStatus::Untracked,
                });
            }
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    let truncated = files.len() > MAX_CHANGED;
    if truncated {
        files.truncate(MAX_CHANGED);
    }

    Ok(ChangedFilesResult {
        files,
        git_available: true,
        truncated,
    })
}

/// Every file tracked by git plus untracked-but-not-ignored files, each tagged
/// with its status relative to `baseline`. Powers the panel's "All" view.
/// Files that changed keep their real status (Modified/Added/Untracked/…);
/// everything else is `Unchanged`. Deletions (present in the diff but gone from
/// disk) are included so the list stays consistent with "Changed".
pub fn all_files(project_dir: &Path, baseline: &str) -> Result<ChangedFilesResult, String> {
    if !is_git_repo(project_dir) {
        return Ok(ChangedFilesResult {
            files: Vec::new(),
            git_available: false,
            truncated: false,
        });
    }

    let mut status_by_path: std::collections::HashMap<String, FileStatus> =
        std::collections::HashMap::new();
    for cf in changed_files(project_dir, baseline)?.files {
        status_by_path.insert(cf.path, cf.status);
    }

    let mut paths: std::collections::BTreeSet<String> = status_by_path.keys().cloned().collect();

    let tracked = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("git ls-files failed: {e}"))?;
    if tracked.status.success() {
        for raw in tracked.stdout.split(|b| *b == 0) {
            if !raw.is_empty() {
                paths.insert(String::from_utf8_lossy(raw).to_string());
            }
        }
    }

    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("git ls-files failed: {e}"))?;
    if untracked.status.success() {
        for raw in untracked.stdout.split(|b| *b == 0) {
            if !raw.is_empty() {
                paths.insert(String::from_utf8_lossy(raw).to_string());
            }
        }
    }

    let mut files: Vec<ChangedFile> = paths
        .into_iter()
        .map(|path| {
            let status = status_by_path
                .get(&path)
                .cloned()
                .unwrap_or(FileStatus::Unchanged);
            ChangedFile { path, status }
        })
        .collect();

    let truncated = files.len() > MAX_ALL;
    if truncated {
        files.truncate(MAX_ALL);
    }

    Ok(ChangedFilesResult {
        files,
        git_available: true,
        truncated,
    })
}

/// Parses the NUL-separated output of `git diff --name-status -z`. With `-z`
/// the status letter and the path are *separate* NUL-terminated fields
/// (`A\0added.txt\0M\0kept.txt\0`) — there is no tab. Renames/copies carry two
/// path fields (`R100\0old\0new\0`); we surface the post-rename path.
fn parse_name_status_z(bytes: &[u8]) -> Vec<ChangedFile> {
    let mut out: Vec<ChangedFile> = Vec::new();
    let mut parts = bytes.split(|b| *b == 0);
    while let Some(status_seg) = parts.next() {
        if status_seg.is_empty() {
            continue;
        }
        let letter = String::from_utf8_lossy(status_seg)
            .chars()
            .next()
            .unwrap_or('?');
        let status = match letter {
            'M' | 'T' => FileStatus::Modified,
            'A' => FileStatus::Added,
            'D' => FileStatus::Deleted,
            'R' | 'C' => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };

        let path = if matches!(letter, 'R' | 'C') {
            // R/C: <oldpath>\0<newpath>\0 follow the status field.
            let _old = parts.next();
            parts.next().map(|b| String::from_utf8_lossy(b).to_string())
        } else {
            parts.next().map(|b| String::from_utf8_lossy(b).to_string())
        };

        if let Some(path) = path.filter(|p| !p.is_empty()) {
            out.push(ChangedFile { path, status });
        }
    }
    out
}

/// Read a working-tree file, capped to 1 MB. Recognised image files small
/// enough to inline come back as a base64 `data:` URL in `image`; files with a
/// NUL in the first 8 KB are reported as binary so the UI doesn't try to render
/// them.
pub fn preview(project_dir: &Path, rel_path: &str) -> Result<FilePreview, String> {
    let full = project_dir.join(rel_path);
    let bytes = match std::fs::read(&full) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FilePreview {
                content: String::new(),
                truncated: false,
                binary: false,
                image: None,
            });
        }
        Err(e) => return Err(format!("read {}: {e}", full.display())),
    };

    // Images: inline as a data URL when small enough, otherwise report binary.
    if let Some(mime) = image_mime(rel_path) {
        if bytes.len() <= IMAGE_MAX_BYTES {
            return Ok(FilePreview {
                content: String::new(),
                truncated: false,
                binary: false,
                image: Some(format!("data:{mime};base64,{}", base64_encode(&bytes))),
            });
        }
        return Ok(FilePreview {
            content: String::new(),
            truncated: true,
            binary: true,
            image: None,
        });
    }

    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0) {
        return Ok(FilePreview {
            content: String::new(),
            truncated: false,
            binary: true,
            image: None,
        });
    }

    let truncated = bytes.len() > PREVIEW_MAX_BYTES;
    let slice = if truncated { &bytes[..PREVIEW_MAX_BYTES] } else { &bytes[..] };
    Ok(FilePreview {
        content: String::from_utf8_lossy(slice).into_owned(),
        truncated,
        binary: false,
        image: None,
    })
}

/// Map a file extension to an image MIME type, or `None` if it isn't an image
/// we know how to inline. SVG counts — it renders fine from a data URL.
fn image_mime(rel_path: &str) -> Option<&'static str> {
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// Standard base64 (RFC 4648) encoder. Hand-rolled to avoid pulling in a crate
/// for the one place we need it — inlining small images as data URLs.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = *chunk.get(1).unwrap_or(&0) as usize;
        let b2 = *chunk.get(2).unwrap_or(&0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// Produce a unified diff between `baseline` and the working tree for one
/// file. Untracked files are diffed against `/dev/null` so additions still
/// render.
pub fn diff(project_dir: &Path, baseline: &str, rel_path: &str) -> Result<String, String> {
    if !is_git_repo(project_dir) {
        return Ok(String::new());
    }

    let untracked = is_untracked(project_dir, rel_path);
    let out = if untracked {
        // `git diff --no-index` exits 1 when files differ — treat that as success.
        Command::new("git")
            .args([
                "diff",
                "--no-color",
                "--unified=3",
                "--no-index",
                "--",
                "/dev/null",
                rel_path,
            ])
            .current_dir(project_dir)
            .output()
            .map_err(|e| format!("git diff --no-index failed: {e}"))?
    } else {
        Command::new("git")
            .args([
                "diff",
                "--no-color",
                "--unified=3",
                baseline,
                "--",
                rel_path,
            ])
            .current_dir(project_dir)
            .output()
            .map_err(|e| format!("git diff failed: {e}"))?
    };

    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.len() > DIFF_MAX_BYTES {
        text.truncate(DIFF_MAX_BYTES);
        text.push_str("\n…diff truncated…\n");
    }
    Ok(text)
}

fn is_untracked(project_dir: &Path, rel_path: &str) -> bool {
    Command::new("git")
        .args(["ls-files", "--error-unmatch", "--", rel_path])
        .current_dir(project_dir)
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git invocation");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn temp_repo() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "arthur-files-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        run(&root, &["init", "-q", "-b", "main"]);
        run(&root, &["config", "user.email", "t@example.com"]);
        run(&root, &["config", "user.name", "t"]);
        run(&root, &["commit", "--allow-empty", "-q", "-m", "init"]);
        root
    }

    #[test]
    fn changed_files_detects_modified_added_untracked() {
        if !git_available() {
            return;
        }
        let dir = temp_repo();
        std::fs::write(dir.join("kept.txt"), b"hello\n").unwrap();
        run(&dir, &["add", "kept.txt"]);
        run(&dir, &["commit", "-q", "-m", "kept"]);
        let baseline = snapshot_head(&dir).unwrap();

        // Modify the tracked file, add a new tracked file, and leave an untracked one.
        std::fs::write(dir.join("kept.txt"), b"hello world\n").unwrap();
        std::fs::write(dir.join("added.txt"), b"new\n").unwrap();
        run(&dir, &["add", "added.txt"]);
        std::fs::write(dir.join("loose.txt"), b"loose\n").unwrap();

        let result = changed_files(&dir, &baseline).expect("changed_files");
        assert!(result.git_available);
        let paths: Vec<&str> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"kept.txt"));
        assert!(paths.contains(&"added.txt"));
        assert!(paths.contains(&"loose.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_files_lists_everything_with_status() {
        if !git_available() {
            return;
        }
        let dir = temp_repo();
        std::fs::write(dir.join("kept.txt"), b"hello\n").unwrap();
        std::fs::write(dir.join("stable.txt"), b"unchanged\n").unwrap();
        run(&dir, &["add", "kept.txt", "stable.txt"]);
        run(&dir, &["commit", "-q", "-m", "files"]);
        let baseline = snapshot_head(&dir).unwrap();

        std::fs::write(dir.join("kept.txt"), b"hello world\n").unwrap();
        std::fs::write(dir.join("loose.txt"), b"loose\n").unwrap();

        let result = all_files(&dir, &baseline).expect("all_files");
        let by_path: std::collections::HashMap<&str, &FileStatus> = result
            .files
            .iter()
            .map(|f| (f.path.as_str(), &f.status))
            .collect();

        assert_eq!(by_path.get("kept.txt"), Some(&&FileStatus::Modified));
        assert_eq!(by_path.get("stable.txt"), Some(&&FileStatus::Unchanged));
        assert_eq!(by_path.get("loose.txt"), Some(&&FileStatus::Untracked));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_shape_for_tracked_and_untracked() {
        if !git_available() {
            return;
        }
        let dir = temp_repo();
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-q", "-m", "a"]);
        let baseline = snapshot_head(&dir).unwrap();

        std::fs::write(dir.join("a.txt"), b"two\n").unwrap();
        let d = diff(&dir, &baseline, "a.txt").unwrap();
        assert!(d.contains("-one"));
        assert!(d.contains("+two"));

        std::fs::write(dir.join("b.txt"), b"fresh\n").unwrap();
        let d = diff(&dir, &baseline, "b.txt").unwrap();
        assert!(d.contains("+fresh"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_marks_binary_and_truncates() {
        if !git_available() {
            return;
        }
        let dir = temp_repo();
        let bin = [b'a', 0u8, b'b'];
        std::fs::write(dir.join("bin"), bin).unwrap();
        let p = preview(&dir, "bin").unwrap();
        assert!(p.binary);

        let big = vec![b'x'; PREVIEW_MAX_BYTES + 256];
        std::fs::write(dir.join("big.txt"), &big).unwrap();
        let p = preview(&dir, "big.txt").unwrap();
        assert!(p.truncated);
        assert!(!p.binary);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn preview_inlines_image_as_data_url() {
        let dir = std::env::temp_dir().join(format!(
            "arthur-files-img-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // A 1x1 transparent GIF's first bytes contain a NUL, so this also proves
        // images bypass the binary probe.
        std::fs::write(dir.join("pixel.gif"), [b'G', b'I', b'F', 0u8, b'8']).unwrap();
        let p = preview(&dir, "pixel.gif").unwrap();
        assert!(!p.binary);
        assert!(p.image.as_deref().unwrap().starts_with("data:image/gif;base64,"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn not_a_repo_returns_git_unavailable() {
        let dir = std::env::temp_dir().join(format!(
            "arthur-files-not-a-repo-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let r = changed_files(&dir, "HEAD").unwrap();
        assert!(!r.git_available);
        assert!(r.files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
