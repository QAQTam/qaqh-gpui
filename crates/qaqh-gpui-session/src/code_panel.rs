use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::{ClipboardItem, Context, Div, PathPromptOptions, SharedString, Window, div, px};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{
    RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, RADIUS_STAGE, shadow_stage, soft,
};
use qaqh_gpui_kit::motion::{sample_split_drawer, scroll_enter, wrap_split_drawer};
use qaqh_ringing_contract::fact::{ContentValue, ToolCallId, TurnId};
use qaqh_ringing_contract::state::ToolCallView;

use qaqh_gpui_kit::{settings, settings::Typography, theme::Palette};
use crate::store::SessionStore;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    path: PathBuf,
    line: usize,
    column: Option<usize>,
}

fn strip_file_uri(value: &str) -> Option<String> {
    let prefix = value.get(..7)?;
    if !prefix.eq_ignore_ascii_case("file://") {
        return None;
    }
    let rest = value.get(7..)?;
    let rest = if rest
        .get(..10)
        .is_some_and(|head| head.eq_ignore_ascii_case("localhost/"))
    {
        rest.get(9..).unwrap_or(rest)
    } else {
        rest
    };
    let decoded = percent_encoding::percent_decode_str(rest)
        .decode_utf8()
        .ok()?
        .into_owned();
    let bytes = decoded.as_bytes();
    if bytes.first() == Some(&b'/')
        && bytes.get(1).is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(2) == Some(&b':')
        && let Some(stripped) = decoded.strip_prefix('/')
    {
        return Some(stripped.to_owned());
    }
    Some(decoded)
}

fn is_known_root_filename(name: &str) -> bool {
    if name.len() > 1 && name.starts_with('.') {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.starts_with("license")
        || lower.starts_with("copying")
        || lower.starts_with("notice")
        || lower.starts_with("readme")
        || lower.starts_with("changelog")
        || matches!(
            lower.as_str(),
            "makefile"
                | "dockerfile"
                | "justfile"
                | "procfile"
                | "agents"
                | "rakefile"
                | "gemfile"
                | "vagrantfile"
        )
}

fn parse_hash_line_suffix(fragment: &str) -> Option<usize> {
    let rest = fragment.strip_prefix('L')?;
    if let Some((start, end)) = rest.split_once('-') {
        let start_line: usize = start.parse().ok()?;
        let end_line: usize = end.strip_prefix('L').unwrap_or(end).parse().ok()?;
        (start_line > 0 && end_line > 0).then_some(start_line)
    } else {
        rest.parse::<usize>().ok().filter(|&line| line > 0)
    }
}

impl Location {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn parse(value: &str, require_line: bool) -> Option<Self> {
        if value.contains(['\n', '\r', '\0']) {
            return None;
        }
        let stripped_uri = strip_file_uri(value);
        let is_file_uri = stripped_uri.is_some();
        let candidate = stripped_uri.as_deref().unwrap_or(value);
        if !is_file_uri
            && (value.contains("://")
                || value
                    .get(..7)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("mailto:")))
        {
            return None;
        }
        if candidate.starts_with('#') {
            return None;
        }

        let (path_str, line, column) = if let Some((prefix, fragment)) = candidate.rsplit_once('#') {
            if fragment.starts_with('L') {
                (prefix, parse_hash_line_suffix(fragment)?, None)
            } else if require_line {
                return None;
            } else {
                (prefix, 1, None)
            }
        } else if let Some((prefix, suffix)) = candidate.rsplit_once(':')
            && let Ok(last) = suffix.parse::<usize>()
        {
            match prefix.rsplit_once(':') {
                Some((path, row)) if row.parse::<usize>().is_ok() => {
                    (path, row.parse().ok()?, Some(last))
                }
                _ => (prefix, last, None),
            }
        } else if require_line {
            return None;
        } else {
            (candidate, 1, None)
        };

        let path = Path::new(path_str);
        if line == 0 || column == Some(0) || path.as_os_str().is_empty() {
            return None;
        }
        let accepted = is_file_uri
            || path.extension().is_some()
            || path.components().count() > 1
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_known_root_filename);
        if !accepted {
            return None;
        }
        Some(Self {
            path: path.into(),
            line,
            column,
        })
    }
}

pub struct CodePanel {
    pub cwd: Option<PathBuf>,
    local_root: Option<PathBuf>,
    view: Option<View>,
    open: bool,
    wrap: bool,
    request: usize,
    live_call_id: Option<ToolCallId>,
    live_baselines: HashMap<String, Option<Arc<Vec<String>>>>,
}

impl Default for CodePanel {
    fn default() -> Self {
        Self {
            cwd: None,
            local_root: None,
            view: None,
            open: false,
            wrap: true,
            request: 0,
            live_call_id: None,
            live_baselines: HashMap::new(),
        }
    }
}

impl CodePanel {
    pub fn with_root(local_root: Option<PathBuf>) -> Self { Self { local_root, ..Self::default() } }
    pub fn local_root(&self) -> Option<PathBuf> { self.local_root.clone() }
    pub fn is_open(&self) -> bool { self.open }
}

fn set_open(store: &mut SessionStore, open: bool, cx: &mut Context<SessionStore>) {
    if store.code_panel.open != open {
        store.code_panel.open = open;
        cx.emit(crate::store::SessionNavEvent::SidePanelOpenChanged { open });
    }
}

pub fn close(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    store.code_panel.request += 1;
    set_open(store, false, cx);
    cx.notify();
}

enum View {
    Source(Location, Result<Excerpt, String>),
    Loading(Location),
    Diff(Review),
    LivePatch(LivePatchView),
}

pub enum LivePatchEvent {
    Delta(ToolCallId),
    Finished { call_id: ToolCallId, success: bool },
}

#[derive(Clone, PartialEq, Eq)]
enum LivePatchStatus {
    Streaming,
    Applied,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LivePatchKind {
    Add,
    Update,
    Delete,
}

#[derive(Clone, PartialEq, Eq)]
enum LiveHunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Clone, PartialEq, Eq)]
struct LiveHunk {
    header: String,
    lines: Vec<LiveHunkLine>,
    has_trailing_partial: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct LiveFilePatch {
    path: String,
    kind: LivePatchKind,
    hunks: Vec<LiveHunk>,
}

struct LivePatchView {
    call_id: ToolCallId,
    files: Vec<LiveFilePatch>,
    selected: usize,
    user_selected: bool,
    args_complete: bool,
    status: LivePatchStatus,
}

struct Review {
    turn: TurnId,
    patches: Vec<Patch>,
    selected: usize,
    loading: bool,
    failed: bool,
}

struct Excerpt {
    first_line: usize,
    lines: Vec<String>,
}

struct Patch {
    path: String,
    deleted: bool,
    edits: usize,
    rows: Vec<DiffRow>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffRowKind {
    Code,
    HunkHeader,
    Meta,
    StepDivider,
}

struct DiffRow {
    text: String,
    old: Option<usize>,
    new: Option<usize>,
    kind: DiffRowKind,
}

fn resolve_safe_text_file(root: Option<&Path>, raw_path: &Path) -> std::io::Result<String> {
    let path = if raw_path.is_absolute() {
        let path = raw_path.canonicalize()?;
        if !path.is_file() {
            return Err(std::io::Error::other("目标必须是存在的普通文件"));
        }
        path
    } else {
        let root = root.ok_or_else(|| std::io::Error::other("未选择源码目录"))?;
        let root = root.canonicalize()?;
        let path = root.join(raw_path).canonicalize()?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(std::io::Error::other("目标必须是所选目录内的普通文件"));
        }
        path
    };
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_BYTES || text.contains('\0') {
        return Err(std::io::Error::other("只支持 4 MiB 以内的 UTF-8 文本文件"));
    }
    Ok(text)
}

fn load_baseline_lines(root: Option<&Path>, path: &str) -> Option<Arc<Vec<String>>> {
    let text = resolve_safe_text_file(root, Path::new(path)).ok()?;
    Some(Arc::new(
        text.split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
            .collect(),
    ))
}

fn load_source(root: Option<&Path>, location: &Location) -> std::io::Result<Excerpt> {
    let text = resolve_safe_text_file(root, &location.path)?;
    let first_line = location.line.saturating_sub(20).max(1);
    let lines: Vec<_> = text
        .split('\n')
        .skip(first_line - 1)
        .take(400)
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect();
    if lines.get(location.line - first_line).is_none() {
        return Err(std::io::Error::other("目标行超出当前文件范围"));
    }
    Ok(Excerpt { first_line, lines })
}

pub fn open_source(store: &mut SessionStore, location: Location, cx: &mut Context<SessionStore>) {
    load_location(store, location, false, cx);
}

fn load_location(
    store: &mut SessionStore,
    location: Location,
    choose_root: bool,
    cx: &mut Context<SessionStore>,
) {
    store.code_panel.request += 1;
    let request = store.code_panel.request;
    store.code_panel.view = Some(View::Loading(location.clone()));
    set_open(store, true, cx);
    cx.notify();

    let needs_root = !location.path.is_absolute();
    let root = (needs_root && !choose_root)
        .then(|| {
            store
                .code_panel
                .local_root
                .clone()
                .or_else(|| store.code_panel.cwd.clone())
        })
        .flatten();
    let picker = (choose_root || (needs_root && root.is_none())).then(|| {
        cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择源码所在的本地目录".into()),
        })
    });
    let selecting = picker.is_some();
    cx.spawn(async move |session, cx| {
        let root: Result<Option<PathBuf>, String> = match picker {
            Some(picker) => match picker.await {
                Ok(Ok(Some(paths))) => paths
                    .into_iter()
                    .next()
                    .map(Some)
                    .ok_or_else(|| "未选择目录".into()),
                Ok(Err(error)) => Err(error.to_string()),
                _ => Err("未选择目录".into()),
            },
            None => Ok(root),
        };
        let result = match &root {
            Ok(root) => {
                let root = root.clone();
                let location = location.clone();
                cx.background_executor()
                    .spawn(async move {
                        load_source(root.as_deref(), &location).map_err(|error| error.to_string())
                    })
                    .await
            }
            Err(error) => Err(error.clone()),
        };
        let _ = session.update(cx, |store, cx| {
            if store.code_panel.request == request {
                if selecting && let Ok(Some(root)) = root {
                    store.code_panel.local_root = Some(root);
                }
                store.code_panel.view = Some(View::Source(location, result));
                cx.notify();
            }
        });
    })
    .detach();
}

fn is_edit(tool: &ToolCallView) -> bool {
    tool.diff.is_some()
        || tool.code_delta.is_some()
        || matches!(
            tool.tool_name.as_deref(),
            Some("write" | "edit" | "apply_patch" | "delete" | "copy_range")
        )
}

fn patches_from_output(text: &str) -> Vec<Patch> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return value
            .get("diff")
            .and_then(|value| value.as_str())
            .map_or_else(Vec::new, parse_diff);
    }
    parse_diff(text)
}

fn range(value: &str, prefix: char) -> Option<(usize, usize)> {
    let value = value.strip_prefix(prefix)?;
    match value.split_once(',') {
        Some((line, count)) => Some((line.parse().ok()?, count.parse().ok()?)),
        None => Some((value.parse().ok()?, 1)),
    }
}

fn parse_diff(text: &str) -> Vec<Patch> {
    let mut lines = text.lines().peekable();
    let mut files = Vec::new();
    while let Some(line) = lines.next() {
        let Some(old) = line.strip_prefix("--- ") else {
            continue;
        };
        let Some(new) = lines
            .next_if(|line| line.starts_with("+++ "))
            .and_then(|line| line.strip_prefix("+++ "))
        else {
            continue;
        };
        let deleted_header = new == "/dev/null";
        let path = if deleted_header {
            old.strip_prefix("a/").unwrap_or(old)
        } else {
            new.strip_prefix("b/").unwrap_or(new)
        };
        let mut rows = Vec::new();
        let mut saw_new_lines = false;
        while let Some(header) = lines.next_if(|line| line.starts_with("@@ ")) {
            let mut parts = header.split_whitespace().skip(1);
            let Some((mut old, mut old_count)) = parts.next().and_then(|part| range(part, '-'))
            else {
                break;
            };
            let Some((mut new, mut new_count)) = parts.next().and_then(|part| range(part, '+'))
            else {
                break;
            };
            if new_count > 0 {
                saw_new_lines = true;
            }
            rows.push(DiffRow {
                text: header.into(),
                old: None,
                new: None,
                kind: DiffRowKind::HunkHeader,
            });
            while old_count > 0 || new_count > 0 {
                let Some(line) = lines.next_if(|line| match line.as_bytes().first() {
                    Some(b' ') => old_count > 0 && new_count > 0,
                    Some(b'-') => old_count > 0,
                    Some(b'+') => new_count > 0,
                    Some(b'\\') => true,
                    _ => false,
                }) else {
                    break;
                };
                let has_old = line.starts_with([' ', '-']);
                let has_new = line.starts_with([' ', '+']);
                rows.push(DiffRow {
                    text: line.into(),
                    old: has_old.then_some(old),
                    new: has_new.then_some(new),
                    kind: match line.as_bytes().first() {
                        Some(b'+' | b'-' | b' ') => DiffRowKind::Code,
                        _ => DiffRowKind::Meta,
                    },
                });
                old_count -= usize::from(has_old);
                new_count -= usize::from(has_new);
                old = old.saturating_add(usize::from(has_old));
                new = new.saturating_add(usize::from(has_new));
            }
            lines.next_if(|line| line.starts_with('\\'));
        }
        if !rows.is_empty() {
            files.push(Patch {
                path: path.into(),
                deleted: deleted_header || !saw_new_lines,
                edits: 1,
                rows,
            });
        }
    }
    files
}

fn normalize_slashes(path: &str) -> String {
    path.trim().replace('\\', "/")
}

fn strip_root_prefix(path: &str, root: Option<&Path>) -> String {
    let norm = normalize_slashes(path);
    let Some(root) = root else {
        return norm;
    };
    let root_norm = normalize_slashes(&root.to_string_lossy());
    let root_trimmed = root_norm.trim_end_matches('/');
    if root_trimmed.is_empty() {
        return norm;
    }
    if let Some(rest) = norm.strip_prefix(root_trimmed) {
        let rel = rest.trim_start_matches('/');
        if !rel.is_empty() {
            return rel.to_owned();
        }
    }
    norm
}

fn patch_merge_key(path: &str, root: Option<&Path>) -> String {
    let stripped = strip_root_prefix(path, root);
    let trimmed = stripped.trim_start_matches("./");
    if cfg!(windows) {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_owned()
    }
}

fn merge_patches(existing: &mut Vec<Patch>, incoming: Vec<Patch>, root: Option<&Path>) {
    for mut patch in incoming {
        let key = patch_merge_key(&patch.path, root);
        if let Some(target) = existing
            .iter_mut()
            .find(|item| patch_merge_key(&item.path, root) == key)
        {
            target.edits = target.edits.saturating_add(patch.edits.max(1));
            target.deleted = patch.deleted;
            // 优先保留带工作区相对语义或更规范的路径，便于点击跳源码。
            if Path::new(&target.path).is_relative() && Path::new(&patch.path).is_absolute() {
                target.path = patch.path;
            }
            target.rows.push(DiffRow {
                text: format!("第 {} 次修改", target.edits),
                old: None,
                new: None,
                kind: DiffRowKind::StepDivider,
            });
            target.rows.append(&mut patch.rows);
        } else {
            existing.push(patch);
        }
    }
}

fn disambiguated_labels(patches: &[Patch], root: Option<&Path>) -> Vec<String> {
    let rels: Vec<String> = patches
        .iter()
        .map(|p| strip_root_prefix(&p.path, root))
        .collect();
    let names: Vec<String> = rels
        .iter()
        .map(|rel| {
            rel.rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(rel.as_str())
                .to_owned()
        })
        .collect();
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for name in &names {
        *counts.entry(name.clone()).or_default() += 1;
    }
    rels.into_iter()
        .zip(names)
        .map(|(rel, name)| {
            if counts.get(name.as_str()).copied().unwrap_or(0) <= 1 {
                name
            } else {
                let mut parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() >= 2 {
                    let tail = parts.split_off(parts.len() - 2);
                    tail.join("/")
                } else {
                    rel
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_unified_diff_keeps_header_like_content_in_its_hunk() {
        let patches = parse_diff(concat!(
            "--- a/first.rs\n+++ b/first.rs\n@@ -5 +5 @@\n",
            "--- old text\n+++ new text\n",
            "--- a/second.rs\n+++ b/second.rs\n@@ -0,0 +1 @@\n+new file\n",
        ));
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].rows[1].old, Some(5));
        assert_eq!(patches[0].rows[2].new, Some(5));
        assert_eq!(patches[1].path, "second.rs");
        assert_eq!(patches[1].rows[1].new, Some(1));
    }

    #[test]
    fn parses_windows_absolute_and_similar_added_deleted_diffs() {
        let diff = concat!(
            "--- D:\\Projects\\foo\\a.rs\n",
            "+++ D:\\Projects\\foo\\a.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n+new\n",
            "--- a/added.rs\n+++ b/added.rs\n@@ -0,0 +1,2 @@\n+one\n+two\n",
            "--- a/gone.rs\n+++ b/gone.rs\n@@ -1,2 +0,0 @@\n-x\n-y\n",
        );
        let patches = parse_diff(diff);
        assert_eq!(patches.len(), 3);
        assert_eq!(patches[0].path, "D:\\Projects\\foo\\a.rs");
        assert!(!patches[0].deleted);
        assert_eq!(patches[1].path, "added.rs");
        assert!(!patches[1].deleted);
        assert_eq!(patches[2].path, "gone.rs");
        assert!(patches[2].deleted);
    }

    #[test]
    fn location_parse_handles_file_uris_line_ranges_windows_and_root_files() {
        let loc = Location::parse("file:///d:/Projects/qaqh-pets/Cargo.toml#L5", false).unwrap();
        assert_eq!(loc.path, PathBuf::from("d:/Projects/qaqh-pets/Cargo.toml"));
        assert_eq!(loc.line, 5);
        assert_eq!(loc.column, None);

        let loc = Location::parse("file:///d:/Projects/qaqh-pets/ASSETS.md#L5-L8", false).unwrap();
        assert_eq!(loc.path, PathBuf::from("d:/Projects/qaqh-pets/ASSETS.md"));
        assert_eq!(loc.line, 5);

        let loc = Location::parse("file:///home/user/repo/src/main.rs#L12", false).unwrap();
        assert_eq!(loc.path, PathBuf::from("/home/user/repo/src/main.rs"));
        assert_eq!(loc.line, 12);

        let loc = Location::parse("file:///D:/My%20Docs/notes.md", false).unwrap();
        assert_eq!(loc.path, PathBuf::from("D:/My Docs/notes.md"));
        assert_eq!(loc.line, 1);

        let loc = Location::parse("D:/Projects/qaqh-pets/Cargo.toml", false).unwrap();
        assert_eq!(loc.path, PathBuf::from("D:/Projects/qaqh-pets/Cargo.toml"));
        assert_eq!(loc.line, 1);

        let loc = Location::parse("D:/Projects/qaqh-pets/src/main.rs:42", true).unwrap();
        assert_eq!(loc.path, PathBuf::from("D:/Projects/qaqh-pets/src/main.rs"));
        assert_eq!(loc.line, 42);
        assert_eq!(loc.column, None);

        let loc = Location::parse("D:/Projects/qaqh-pets/src/main.rs:42:7", true).unwrap();
        assert_eq!(loc.path, PathBuf::from("D:/Projects/qaqh-pets/src/main.rs"));
        assert_eq!(loc.line, 42);
        assert_eq!(loc.column, Some(7));

        let loc = Location::parse("crates/qaqh-gpui-shell/src/sidebar.rs#L10-L20", false).unwrap();
        assert_eq!(
            loc.path,
            PathBuf::from("crates/qaqh-gpui-shell/src/sidebar.rs")
        );
        assert_eq!(loc.line, 10);

        assert!(Location::parse("LICENSE-MIT", false).is_some());
        assert!(Location::parse(".gitignore", false).is_some());
        assert!(Location::parse("Cargo.toml", false).is_some());

        assert!(Location::parse("https://github.com/QAQTam/qaqh-pets", false).is_none());
        assert!(Location::parse("#section", false).is_none());
        assert!(Location::parse("hello", false).is_none());
        assert!(Location::parse("file.rs#L0", false).is_none());
    }

    #[test]
    fn merge_patches_deduplicates_same_file_across_relative_and_absolute() {
        let root = Path::new("/home/qaqtam/项目/qaq-bx");
        let first = parse_diff("--- a/justfile\n+++ b/justfile\n@@ -1 +1 @@\n-old\n+mid\n");
        let second = parse_diff(
            "--- /home/qaqtam/项目/qaq-bx/justfile\n+++ /home/qaqtam/项目/qaq-bx/justfile\n@@ -1 +1 @@\n-mid\n+new\n",
        );
        let mut merged = Vec::new();
        merge_patches(&mut merged, first, Some(root));
        merge_patches(&mut merged, second, Some(root));

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].edits, 2);
        assert!(
            merged[0]
                .rows
                .iter()
                .any(|row| row.kind == DiffRowKind::StepDivider && row.text == "第 2 次修改")
        );
    }

    #[test]
    fn disambiguated_labels_shorten_unique_and_qualify_collisions() {
        let root = Path::new("/repo");
        let patches = vec![
            Patch {
                path: "/repo/justfile".into(),
                deleted: false,
                edits: 1,
                rows: Vec::new(),
            },
            Patch {
                path: "/repo/crates/a/lib.rs".into(),
                deleted: false,
                edits: 1,
                rows: Vec::new(),
            },
            Patch {
                path: "/repo/crates/b/lib.rs".into(),
                deleted: false,
                edits: 1,
                rows: Vec::new(),
            },
        ];
        assert_eq!(
            disambiguated_labels(&patches, Some(root)),
            vec!["justfile", "a/lib.rs", "b/lib.rs"]
        );
    }
}

/// 增量解析 Codex `apply_patch` 参数前缀,提取目标文件、分块、以及末尾未换行的流式半行标志。
fn parse_live_apply_patch(text: &str) -> Vec<LiveFilePatch> {
    let has_trailing_newline = text.ends_with('\n');
    let raw_lines: Vec<&str> = text.split('\n').collect();
    let total_lines = raw_lines.len();
    let mut files: Vec<LiveFilePatch> = Vec::new();

    for (ix, raw) in raw_lines.into_iter().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let is_last = ix + 1 == total_lines;
        let is_partial = is_last && !has_trailing_newline;

        if line.starts_with("*** Begin Patch") || line.starts_with("*** End Patch") {
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File:") {
            let trimmed = path.trim();
            if !trimmed.is_empty() && !is_partial {
                files.push(LiveFilePatch {
                    path: trimmed.to_owned(),
                    kind: LivePatchKind::Update,
                    hunks: Vec::new(),
                });
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Add File:") {
            let trimmed = path.trim();
            if !trimmed.is_empty() && !is_partial {
                files.push(LiveFilePatch {
                    path: trimmed.to_owned(),
                    kind: LivePatchKind::Add,
                    hunks: Vec::new(),
                });
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File:") {
            let trimmed = path.trim();
            if !trimmed.is_empty() && !is_partial {
                files.push(LiveFilePatch {
                    path: trimmed.to_owned(),
                    kind: LivePatchKind::Delete,
                    hunks: Vec::new(),
                });
            }
            continue;
        }
        if line.starts_with("*** Move to:") || line.starts_with("*** End of File") {
            continue;
        }
        let Some(current_file) = files.last_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("@@") {
            let header = rest.trim().to_owned();
            current_file.hunks.push(LiveHunk {
                header,
                lines: Vec::new(),
                has_trailing_partial: is_partial,
            });
            continue;
        }
        if current_file.kind == LivePatchKind::Add {
            if let Some(added) = line.strip_prefix('+') {
                if current_file.hunks.is_empty() {
                    current_file.hunks.push(LiveHunk {
                        header: String::new(),
                        lines: Vec::new(),
                        has_trailing_partial: false,
                    });
                }
                let hunk = current_file.hunks.last_mut().expect("just pushed");
                hunk.lines.push(LiveHunkLine::Add(added.to_owned()));
                hunk.has_trailing_partial = is_partial;
            }
            continue;
        }
        let Some(current_hunk) = current_file.hunks.last_mut() else {
            continue;
        };
        if let Some(body) = line.strip_prefix('+') {
            current_hunk.lines.push(LiveHunkLine::Add(body.to_owned()));
            current_hunk.has_trailing_partial = is_partial;
        } else if let Some(body) = line.strip_prefix('-') {
            current_hunk.lines.push(LiveHunkLine::Remove(body.to_owned()));
            current_hunk.has_trailing_partial = is_partial;
        } else if let Some(body) = line.strip_prefix(' ') {
            current_hunk.lines.push(LiveHunkLine::Context(body.to_owned()));
            current_hunk.has_trailing_partial = is_partial;
        }
    }

    files
}

/// 在本地基线源码中定位补丁 hunk 的起始行号(1-based)。
/// 仅当锚点或原文件上下文序列唯一命中时返回行号,避免误判导致视图跳动。
fn locate_hunk_start(file_lines: &[String], hunk: &LiveHunk) -> Option<usize> {
    let mut seq: Vec<&str> = hunk
        .lines
        .iter()
        .filter_map(|line| match line {
            LiveHunkLine::Context(s) | LiveHunkLine::Remove(s) => Some(s.as_str()),
            LiveHunkLine::Add(_) => None,
        })
        .collect();
    if hunk.has_trailing_partial
        && matches!(
            hunk.lines.last(),
            Some(LiveHunkLine::Context(_) | LiveHunkLine::Remove(_))
        )
    {
        seq.pop();
    }

    let anchor = hunk.header.trim();
    if !seq.is_empty() && (seq.len() >= 2 || seq[0].trim().len() >= 8) {
        let len = seq.len();
        let matches: Vec<usize> = (0..=file_lines.len().saturating_sub(len))
            .filter(|&start| {
                seq.iter()
                    .enumerate()
                    .all(|(off, expected)| file_lines[start + off].trim_end() == expected.trim_end())
            })
            .collect();
        if matches.len() == 1 {
            return Some(matches[0] + 1);
        }
        if matches.len() > 1 && !anchor.is_empty() {
            let anchor_hits: Vec<usize> = file_lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.contains(anchor))
                .map(|(ix, _)| ix)
                .collect();
            if anchor_hits.len() == 1 {
                let a = anchor_hits[0];
                if let Some(&best) = matches.iter().filter(|&&m| m >= a).min() {
                    return Some(best + 1);
                }
            }
        }
    }

    if anchor.len() >= 4 {
        let hits: Vec<usize> = file_lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains(anchor))
            .map(|(ix, _)| ix)
            .collect();
        if hits.len() == 1 {
            return Some(hits[0] + 1);
        }
    }
    None
}

/// 将流式 `LiveFilePatch` 与本地基线行合并成带真实行号和上下文窗口的 [`Patch`] 预览。
fn build_live_preview_patch(
    file: &LiveFilePatch,
    baseline: Option<&Vec<String>>,
) -> (Patch, Option<usize>) {
    let mut rows = Vec::new();
    let mut locked_line = None;

    if file.kind == LivePatchKind::Delete {
        rows.push(DiffRow {
            text: "将删除此文件".into(),
            old: None,
            new: None,
            kind: DiffRowKind::Meta,
        });
        if let Some(lines) = baseline {
            for (ix, line) in lines.iter().take(24).enumerate() {
                rows.push(DiffRow {
                    text: format!("-{line}"),
                    old: Some(ix + 1),
                    new: None,
                    kind: DiffRowKind::Code,
                });
            }
        }
        return (
            Patch {
                path: file.path.clone(),
                deleted: true,
                edits: 1,
                rows,
            },
            baseline.map(|_| 1),
        );
    }

    if file.hunks.is_empty() {
        if let Some(lines) = baseline {
            rows.push(DiffRow {
                text: "@@ 等待补丁定位行号…".into(),
                old: None,
                new: None,
                kind: DiffRowKind::HunkHeader,
            });
            for (ix, line) in lines.iter().take(20).enumerate() {
                rows.push(DiffRow {
                    text: format!(" {line}"),
                    old: Some(ix + 1),
                    new: Some(ix + 1),
                    kind: DiffRowKind::Code,
                });
            }
        }
        return (
            Patch {
                path: file.path.clone(),
                deleted: false,
                edits: 1,
                rows,
            },
            None,
        );
    }

    let last_ix = file.hunks.len().saturating_sub(1);
    for (hunk_ix, hunk) in file.hunks.iter().enumerate() {
        let is_focus = hunk_ix == last_ix;
        let anchor = baseline.and_then(|lines| locate_hunk_start(lines, hunk));
        if is_focus {
            locked_line = anchor;
        }
        let header_label = match (anchor, hunk.header.is_empty()) {
            (Some(line), false) => format!("@@ L{line} · {}", hunk.header),
            (Some(line), true) => format!("@@ L{line}"),
            (None, false) => format!("@@ {}", hunk.header),
            (None, true) => "@@".to_owned(),
        };
        rows.push(DiffRow {
            text: header_label,
            old: None,
            new: None,
            kind: DiffRowKind::HunkHeader,
        });

        let (mut old_line, mut new_line) = match (anchor, file.kind) {
            (Some(start), _) => (Some(start), Some(start)),
            (None, LivePatchKind::Add) => (None, Some(1)),
            (None, _) => (None, None),
        };

        if let (Some(lines), Some(start)) = (baseline, anchor)
            && is_focus
        {
            let ctx_start = start.saturating_sub(7).max(1);
            for line_no in ctx_start..start {
                if let Some(src) = lines.get(line_no - 1) {
                    rows.push(DiffRow {
                        text: format!(" {src}"),
                        old: Some(line_no),
                        new: Some(line_no),
                        kind: DiffRowKind::Code,
                    });
                }
            }
        }

        let mut body_only_anchor = hunk.lines.is_empty();
        for item in &hunk.lines {
            body_only_anchor = false;
            match item {
                LiveHunkLine::Context(s) => {
                    rows.push(DiffRow {
                        text: format!(" {s}"),
                        old: old_line,
                        new: new_line,
                        kind: DiffRowKind::Code,
                    });
                    if let Some(n) = old_line.as_mut() {
                        *n += 1;
                    }
                    if let Some(n) = new_line.as_mut() {
                        *n += 1;
                    }
                }
                LiveHunkLine::Remove(s) => {
                    rows.push(DiffRow {
                        text: format!("-{s}"),
                        old: old_line,
                        new: None,
                        kind: DiffRowKind::Code,
                    });
                    if let Some(n) = old_line.as_mut() {
                        *n += 1;
                    }
                }
                LiveHunkLine::Add(s) => {
                    rows.push(DiffRow {
                        text: format!("+{s}"),
                        old: None,
                        new: new_line,
                        kind: DiffRowKind::Code,
                    });
                    if let Some(n) = new_line.as_mut() {
                        *n += 1;
                    }
                }
            }
        }

        if let (Some(lines), Some(start)) = (baseline, anchor)
            && is_focus
        {
            if body_only_anchor {
                let end = (start + 14).min(lines.len());
                for line_no in start..=end {
                    if let Some(src) = lines.get(line_no - 1) {
                        rows.push(DiffRow {
                            text: format!(" {src}"),
                            old: Some(line_no),
                            new: Some(line_no),
                            kind: DiffRowKind::Code,
                        });
                    }
                }
            } else if let Some(next_old) = old_line {
                let end = (next_old + 8).min(lines.len());
                let mut cur_new = new_line;
                for line_no in next_old..=end {
                    if let Some(src) = lines.get(line_no - 1) {
                        rows.push(DiffRow {
                            text: format!(" {src}"),
                            old: Some(line_no),
                            new: cur_new,
                            kind: DiffRowKind::Code,
                        });
                        if let Some(n) = cur_new.as_mut() {
                            *n += 1;
                        }
                    }
                }
            }
        }
    }

    (
        Patch {
            path: file.path.clone(),
            deleted: false,
            edits: 1,
            rows,
        },
        locked_line,
    )
}

pub fn on_live_patch_event(
    store: &mut SessionStore,
    event: LivePatchEvent,
    cx: &mut Context<SessionStore>,
) {
    match event {
        LivePatchEvent::Delta(call_id) => {
            if !settings::get(cx).behavior.exp_live_patch_preview {
                return;
            }
            let Some(stream) = store.arg_stream.iter().find(|s| s.call_id == call_id) else {
                return;
            };
            let stream_text = stream.text.clone();
            let target_files = stream.target_files.clone();
            let args_complete = stream.args_complete;

            let mut files = parse_live_apply_patch(&stream_text);
            let root = store
                .code_panel
                .local_root
                .clone()
                .or_else(|| store.code_panel.cwd.clone());
            for tf in &target_files {
                let key = patch_merge_key(tf, root.as_deref());
                if !files.iter().any(|f| patch_merge_key(&f.path, root.as_deref()) == key) {
                    files.push(LiveFilePatch {
                        path: tf.clone(),
                        kind: LivePatchKind::Update,
                        hunks: Vec::new(),
                    });
                }
            }
            if files.is_empty() {
                return;
            }

            if store.code_panel.live_call_id.as_ref() != Some(&call_id) {
                store.code_panel.live_call_id = Some(call_id.clone());
                store.code_panel.live_baselines.clear();
                store.code_panel.request += 1;
            }
            for file in &files {
                if !store.code_panel.live_baselines.contains_key(&file.path) {
                    let loaded = match file.kind {
                        LivePatchKind::Add => None,
                        LivePatchKind::Update | LivePatchKind::Delete => {
                            load_baseline_lines(root.as_deref(), &file.path)
                        }
                    };
                    store.code_panel.live_baselines.insert(file.path.clone(), loaded);
                }
            }

            let (selected, user_selected) = match &store.code_panel.view {
                Some(View::LivePatch(prev)) if prev.call_id == call_id && prev.user_selected => {
                    (prev.selected.min(files.len().saturating_sub(1)), true)
                }
                _ => (files.len().saturating_sub(1), false),
            };
            store.code_panel.view = Some(View::LivePatch(LivePatchView {
                call_id,
                files,
                selected,
                user_selected,
                args_complete,
                status: LivePatchStatus::Streaming,
            }));
            set_open(store, true, cx);
        }
        LivePatchEvent::Finished { call_id, success } => {
            let is_active_live = matches!(
                &store.code_panel.view,
                Some(View::LivePatch(live)) if live.call_id == call_id
            );
            if !is_active_live {
                return;
            }
            if success {
                let turn_id = store
                    .tools
                    .iter()
                    .find(|tool| tool.call_id == call_id)
                    .and_then(|tool| tool.turn_id.clone());
                if let Some(turn) = turn_id {
                    open_review(store, turn, false, cx);
                }
                if let Some(View::LivePatch(live)) = &mut store.code_panel.view
                    && live.call_id == call_id
                {
                    live.status = LivePatchStatus::Applied;
                }
            } else {
                let reason = store
                    .tools
                    .iter()
                    .find(|tool| tool.call_id == call_id)
                    .and_then(|tool| tool.output.as_ref())
                    .and_then(|out| match out {
                        ContentValue::Inline { text } => {
                            let line = text.lines().find(|l| !l.trim().is_empty())?.trim();
                            Some(line.chars().take(120).collect::<String>())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| "补丁未能应用（上下文不匹配或参数错误）".to_owned());
                if let Some(View::LivePatch(live)) = &mut store.code_panel.view {
                    live.status = LivePatchStatus::Failed(reason);
                }
            }
        }
    }
}

pub fn auto_review(store: &mut SessionStore, turn: TurnId, cx: &mut Context<SessionStore>) {
    open_review(store, turn, false, cx);
}

fn open_review(
    store: &mut SessionStore,
    turn: TurnId,
    explicit: bool,
    cx: &mut Context<SessionStore>,
) {
    let root = store
        .code_panel
        .local_root
        .clone()
        .or_else(|| store.code_panel.cwd.clone());
    let mut patches = Vec::new();
    let mut references = Vec::new();
    for tool in store
        .tools
        .iter()
        .filter(|tool| tool.turn_id.as_ref() == Some(&turn) && is_edit(tool))
    {
        if let Some(diff_val) = &tool.diff {
            match diff_val {
                ContentValue::Inline { text } => {
                    merge_patches(&mut patches, parse_diff(text), root.as_deref());
                }
                ContentValue::Ref { content_ref } => references.push(content_ref.clone()),
                ContentValue::Unavailable(_) => {}
            }
        } else {
            match &tool.output {
                Some(ContentValue::Inline { text }) => {
                    merge_patches(&mut patches, patches_from_output(text), root.as_deref());
                }
                Some(ContentValue::Ref { content_ref }) => references.push(content_ref.clone()),
                _ => {}
            }
        }
    }
    if !explicit && patches.is_empty() && references.is_empty() {
        return;
    }
    store.code_panel.request += 1;
    let loading = !references.is_empty();
    if loading {
        store.request_review_content(turn.clone(), store.code_panel.request, references);
    }
    store.code_panel.view = Some(View::Diff(Review {
        turn,
        patches,
        selected: 0,
        loading,
        failed: false,
    }));
    set_open(store, true, cx);
}

pub fn finish_review(
    store: &mut SessionStore,
    turn: TurnId,
    request: usize,
    results: Vec<Result<String, String>>,
) {
    let root = store
        .code_panel
        .local_root
        .clone()
        .or_else(|| store.code_panel.cwd.clone());
    if store.code_panel.request == request
        && let Some(View::Diff(review)) = &mut store.code_panel.view
        && review.turn == turn
    {
        review.loading = false;
        for result in results {
            match result {
                Ok(text) => {
                    merge_patches(&mut review.patches, patches_from_output(&text), root.as_deref());
                }
                Err(_) => review.failed = true,
            }
        }
    }
}

pub fn review_button(
    store: &SessionStore,
    t: &Palette,
    cx: &mut Context<SessionStore>,
) -> Option<impl IntoElement> {
    let turn = store
        .turns
        .iter()
        .rev()
        .find(|turn| {
            turn.outcome.is_some()
                && store
                    .tools
                    .iter()
                    .any(|tool| tool.turn_id.as_ref() == Some(&turn.turn_id) && is_edit(tool))
        })?
        .turn_id
        .clone();
    let (added, removed) = store
        .tools
        .iter()
        .filter(|tool| tool.turn_id.as_ref() == Some(&turn))
        .filter_map(|tool| tool.code_delta.as_ref())
        .fold((0usize, 0usize), |(added, removed), delta| {
            (
                added.saturating_add(delta.lines_added),
                removed.saturating_add(delta.lines_removed),
            )
        });
    // git-compare icon 前缀 + 居中悬浮胶囊,置于 Composer 上方。
    Some(
        div()
            .flex()
            .justify_center()
            .pb_1p5()
            .child(
                div()
                    .id("review-latest-turn")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(t.surface_subtle)
                    .border_1()
                    .border_color(t.border_subtle)
                    .text_xs()
                    .cursor_pointer()
                    .hover(|style| style.bg(t.hover))
                    .child(
                        Icon::new(IconName::GitCompare)
                            .with_size(Size::XSmall)
                            .text_color(t.accent),
                    )
                    .child(div().text_color(t.text).child("查看本轮代码改动"))
                    .child(
                        div()
                            .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                            .text_color(t.ok)
                            .child(SharedString::from(format!("+{added}"))),
                    )
                    .child(
                        div()
                            .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                            .text_color(t.bad)
                            .child(SharedString::from(format!("−{removed}"))),
                    )
                    .on_click(cx.listener(move |store, _, _, cx| {
                        open_review(store, turn.clone(), true, cx);
                        cx.notify();
                    })),
            ),
    )
}

fn line_text_cell(text: SelectableText, wrap: bool) -> gpui_kit::AnyElement {
    if wrap {
        div().flex_1().min_w_0().child(text).into_any_element()
    } else {
        text.into_any_element()
    }
}

fn render_files_strip(
    request: usize,
    paths: &[String],
    edits: &[usize],
    selected_idx: usize,
    root: Option<&Path>,
    is_live: bool,
    t: &Palette,
    cx: &mut Context<SessionStore>,
) -> Option<gpui_kit::AnyElement> {
    if paths.is_empty() {
        return None;
    }
    let label_stubs: Vec<Patch> = paths
        .iter()
        .map(|path| Patch {
            path: path.clone(),
            deleted: false,
            edits: 1,
            rows: Vec::new(),
        })
        .collect();
    let labels = disambiguated_labels(&label_stubs, root);
    let strip = div()
        .id("diff-files-strip")
        .h(px(36.))
        .w_full()
        .px_3()
        .flex()
        .items_center()
        .gap_1p5()
        .children(paths.iter().zip(labels).enumerate().map(|(index, (path, label))| {
            let active = index == selected_idx;
            let full_tip = strip_root_prefix(path, root);
            let edit_count = edits.get(index).copied().unwrap_or(1);
            div()
                .id(("diff-file", index))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(3.))
                .rounded(px(RADIUS_CHIP))
                .text_xs()
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(t.accent_soft)
                        .text_color(t.accent)
                        .border_1()
                        .border_color(soft(t.accent, 0.28))
                })
                .when(!active, |el| {
                    el.bg(t.card)
                        .border_1()
                        .border_color(t.border_subtle)
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.hover).text_color(t.text))
                })
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(full_tip.clone()).build(window, cx)
                })
                .child(SharedString::from(label))
                .when(edit_count > 1, |el| {
                    el.child(
                        div()
                            .text_size(px(10.))
                            .text_color(if active { t.accent } else { t.faint })
                            .child(SharedString::from(format!("×{edit_count}"))),
                    )
                })
                .on_click(cx.listener(move |store, _, _, cx| {
                    match &mut store.code_panel.view {
                        Some(View::Diff(review)) if !is_live => review.selected = index,
                        Some(View::LivePatch(live)) if is_live => {
                            live.selected = index;
                            live.user_selected = true;
                        }
                        _ => {}
                    }
                    cx.notify();
                }))
        }));
    Some(
        div()
            .w_full()
            .flex_shrink_0()
            .bg(t.surface_subtle)
            .border_b_1()
            .border_color(t.border_subtle)
            .child(
                strip
                    .overflow_x_scrollbar()
                    .id(SharedString::from(format!("diff-files-scroll-{request}"))),
            )
            .into_any_element(),
    )
}

fn render_patch_body(
    request: usize,
    selected_idx: usize,
    patch: &Patch,
    wrap: bool,
    greyed_out: bool,
    trailing_pulse: bool,
    t: &Palette,
    cx: &mut Context<SessionStore>,
) -> (Div, String) {
    let mut body = if wrap {
        div().w_full().min_w_0().flex().flex_col()
    } else {
        div().min_w_full().flex().flex_col()
    };
    let copy_all = patch
        .rows
        .iter()
        .filter(|row| row.kind != DiffRowKind::StepDivider)
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let lang = Path::new(&patch.path)
        .extension()
        .and_then(|extension| extension.to_str());
    let code_lines: Vec<&str> = patch
        .rows
        .iter()
        .map(|row| {
            if row.kind == DiffRowKind::Code {
                row.text.get(1..).unwrap_or("")
            } else {
                ""
            }
        })
        .collect();
    let per_line = qaqh_gpui_render::highlight::highlight_lines(&code_lines, lang, t);
    let total_rows = patch.rows.len();
    body = body.children(patch.rows.iter().zip(per_line).enumerate().map(
        |(index, (row, spans))| {
            if row.kind == DiffRowKind::StepDivider {
                return div()
                    .id(("diff-step-sep", index))
                    .my_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().h(px(1.)).bg(t.border_subtle))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.faint)
                            .child(SharedString::from(row.text.clone())),
                    )
                    .child(div().flex_1().h(px(1.)).bg(t.border_subtle))
                    .into_any_element();
            }
            let first = row.text.as_bytes().first().copied();
            let (sign_str, color, row_bg, is_code) = if greyed_out {
                match (row.kind, first) {
                    (DiffRowKind::Code, Some(b'+')) => ("+", t.faint, Some(soft(t.faint, 0.07)), true),
                    (DiffRowKind::Code, Some(b'-')) => ("-", t.faint, Some(soft(t.faint, 0.05)), true),
                    (DiffRowKind::Code, Some(b' ')) => (" ", t.faint, None, true),
                    (DiffRowKind::HunkHeader, _) => ("", t.dim, Some(soft(t.faint, 0.06)), false),
                    _ => ("", t.dim, None, false),
                }
            } else {
                match (row.kind, first) {
                    (DiffRowKind::Code, Some(b'+')) => ("+", t.ok, Some(soft(t.ok, 0.08)), true),
                    (DiffRowKind::Code, Some(b'-')) => ("-", t.bad, Some(soft(t.bad, 0.08)), true),
                    (DiffRowKind::Code, Some(b' ')) => (" ", t.dim, None, true),
                    (DiffRowKind::HunkHeader, _) => {
                        ("", t.accent, Some(soft(t.accent, 0.07)), false)
                    }
                    _ => ("", t.text, None, false),
                }
            };
            let is_streaming_tail = trailing_pulse && index + 1 == total_rows && is_code;
            let code_body = if is_code {
                row.text.get(1..).unwrap_or("").to_owned()
            } else {
                row.text.clone()
            };
            let path = PathBuf::from(&patch.path);
            let line = row.new;
            let text_el = SelectableText::new(
                SharedString::from(format!("code-diff-{request}-{selected_idx}-{index}")),
                code_body,
            )
            .highlights(if greyed_out { Vec::new() } else { spans })
            .selection_color(t.selection_wash);
            div()
                .id(("diff-line", index))
                .flex()
                .items_start()
                .gap_2()
                .px_1p5()
                .rounded(px(4.))
                .when(wrap, |row| row.w_full().min_w_0())
                .when(!wrap, |row| row.whitespace_nowrap())
                .text_color(if greyed_out {
                    t.dim
                } else if is_code {
                    t.text
                } else {
                    color
                })
                .when(is_streaming_tail, |el| {
                    el.bg(soft(t.accent, 0.14)).border_l_2().border_color(t.accent)
                })
                .when(!is_streaming_tail, |el| el.when_some(row_bg, |el, bg| el.bg(bg)))
                .child(div().w(px(82.)).flex_shrink_0().text_color(t.dim).child(
                    format!(
                        "{:>4} {:>4}",
                        row.old.map(|line| line.to_string()).unwrap_or_default(),
                        line.map(|line| line.to_string()).unwrap_or_default()
                    ),
                ))
                .when(is_code, |el| {
                    el.child(div().w(px(10.)).flex_shrink_0().text_color(color).child(sign_str))
                })
                .child(line_text_cell(text_el, wrap))
                .when(!greyed_out, |el| {
                    el.when_some(line.filter(|_| !patch.deleted), |element, line| {
                        element.cursor_pointer().on_click(cx.listener(
                            move |store, _, _, cx| {
                                open_source(
                                    store,
                                    Location {
                                        path: path.clone(),
                                        line,
                                        column: None,
                                    },
                                    cx,
                                );
                            },
                        ))
                    })
                })
                .into_any_element()
        },
    ));
    (body, copy_all)
}

pub fn render(
    store: &mut SessionStore,
    t: &Palette,
    ty: &Typography,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
) -> Option<Div> {
    let sibling_open = store.terminal_panel.open && !store.terminal_panel.modal_visible();
    let Some(sample) =
        sample_split_drawer("code-panel-width", store.code_panel.open, sibling_open, window, cx)
    else {
        store.code_panel.view = None;
        return None;
    };
    let wrap = store.code_panel.wrap;
    let root = store
        .code_panel
        .local_root
        .clone()
        .or_else(|| store.code_panel.cwd.clone());
    let view = store.code_panel.view.as_ref()?;
    let (title, sub_header, body, full_copy_text) = match view {
        View::Loading(location) => (
            location.path.display().to_string(),
            None,
            div().child("正在读取源码…"),
            String::new(),
        ),
        View::Source(location, result) => {
            let mut title = format!("{}:{}", location.path.display(), location.line);
            if let Some(column) = location.column {
                title.push_str(&format!(":{column}"));
            }
            let (body, copy_all) = match result {
                Ok(excerpt) => {
                    let lang = location
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str());
                    let line_refs: Vec<&str> = excerpt.lines.iter().map(String::as_str).collect();
                    let per_line = qaqh_gpui_render::highlight::highlight_lines(&line_refs, lang, t);
                    let copy_all = excerpt.lines.join("\n");
                    let container = if wrap {
                        div().w_full().min_w_0().flex().flex_col()
                    } else {
                        div().min_w_full().flex().flex_col()
                    };
                    let body = container.children(excerpt.lines.iter().zip(per_line).enumerate().map(
                        |(index, (line, spans))| {
                            let number = excerpt.first_line + index;
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .when(wrap, |row| row.w_full().min_w_0())
                                .when(!wrap, |row| row.whitespace_nowrap())
                                .when(number == location.line, |row| {
                                    row.bg(t.code_bg).border_l_2().border_color(t.accent)
                                })
                                .child(
                                    div()
                                        .w(px(48.))
                                        .flex_shrink_0()
                                        .text_color(t.dim)
                                        .child(number.to_string()),
                                )
                                .child(line_text_cell(
                                    SelectableText::new(
                                        SharedString::from(format!(
                                            "code-src-{}-{number}",
                                            store.code_panel.request
                                        )),
                                        line.clone(),
                                    )
                                    .highlights(spans)
                                    .selection_color(t.selection_wash),
                                    wrap,
                                ))
                        },
                    ));
                    (body, copy_all)
                }
                Err(error) => (div().text_color(t.warn).child(error.clone()), String::new()),
            };
            (title, None, body, copy_all)
        }
        View::Diff(review) => {
            let Review {
                patches, selected, ..
            } = review;
            let selected_idx = (*selected).min(patches.len().saturating_sub(1));
            let paths: Vec<String> = patches.iter().map(|p| p.path.clone()).collect();
            let edits: Vec<usize> = patches.iter().map(|p| p.edits).collect();
            let files_bar = render_files_strip(
                store.code_panel.request,
                &paths,
                &edits,
                selected_idx,
                root.as_deref(),
                false,
                t,
                cx,
            );
            let (mut body, copy_all) = if let Some(patch) = patches.get(selected_idx) {
                render_patch_body(
                    store.code_panel.request,
                    selected_idx,
                    patch,
                    wrap,
                    false,
                    false,
                    t,
                    cx,
                )
            } else {
                (
                    if wrap {
                        div().w_full().min_w_0().flex().flex_col()
                    } else {
                        div().min_w_full().flex().flex_col()
                    },
                    String::new(),
                )
            };
            if patches.get(selected_idx).is_none() && !review.loading && !review.failed {
                body = body.child("当前工具结果没有可显示的补丁正文");
            }
            if review.loading {
                body = body.child("正在读取工具结果…");
            }
            if review.failed {
                body = body.child(div().text_color(t.warn).child("部分工具结果读取失败"));
            }
            let diff_title = if patches.is_empty() {
                "本轮改动".to_string()
            } else {
                format!("本轮改动 · {} 个文件", patches.len())
            };
            (diff_title, files_bar, body, copy_all)
        }
        View::LivePatch(live) => {
            let selected_idx = live.selected.min(live.files.len().saturating_sub(1));
            let paths: Vec<String> = live.files.iter().map(|f| f.path.clone()).collect();
            let edits: Vec<usize> = vec![1; paths.len()];
            let files_bar = render_files_strip(
                store.code_panel.request,
                &paths,
                &edits,
                selected_idx,
                root.as_deref(),
                true,
                t,
                cx,
            );
            let greyed_out = matches!(live.status, LivePatchStatus::Failed(_));
            let trailing_pulse =
                matches!(live.status, LivePatchStatus::Streaming) && !live.args_complete;
            let (body, copy_all, live_title) = if let Some(file) = live.files.get(selected_idx) {
                let baseline = store
                    .code_panel
                    .live_baselines
                    .get(&file.path)
                    .and_then(|opt| opt.as_deref());
                let (patch, locked_line) = build_live_preview_patch(file, baseline);
                let (mut body, copy_all) = render_patch_body(
                    store.code_panel.request,
                    selected_idx,
                    &patch,
                    wrap,
                    greyed_out,
                    trailing_pulse,
                    t,
                    cx,
                );
                if let LivePatchStatus::Failed(reason) = &live.status {
                    body = div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .rounded(px(RADIUS_CARD))
                                .bg(soft(t.faint, 0.10))
                                .border_1()
                                .border_color(t.border_subtle)
                                .text_xs()
                                .text_color(t.dim)
                                .child(SharedString::from(format!(
                                    "补丁未应用（已置灰预览） · {reason}"
                                ))),
                        )
                        .child(body);
                }
                let short_name = strip_root_prefix(&file.path, root.as_deref());
                let title = match (&live.status, locked_line) {
                    (LivePatchStatus::Failed(_), Some(line)) => {
                        format!("未应用（已置灰）· {short_name}:L{line}")
                    }
                    (LivePatchStatus::Failed(_), None) => format!("未应用（已置灰）· {short_name}"),
                    (LivePatchStatus::Applied, Some(line)) => format!("已应用 · {short_name}:L{line}"),
                    (LivePatchStatus::Applied, None) => format!("已应用 · {short_name}"),
                    (LivePatchStatus::Streaming, Some(line)) => {
                        format!("实时修改 · {short_name}:L{line}")
                    }
                    (LivePatchStatus::Streaming, None) => format!("实时修改 · {short_name}"),
                };
                (body, copy_all, title)
            } else {
                (
                    div().child("等待补丁内容…"),
                    String::new(),
                    "实时修改预览".to_owned(),
                )
            };
            (live_title, files_bar, body, copy_all)
        }
    };
    let content_key = match view {
        View::Loading(_) => format!("cp-load-{}", store.code_panel.request),
        View::Source(_, _) => format!("cp-src-{}", store.code_panel.request),
        View::Diff(review) => format!("cp-diff-{}-{}", store.code_panel.request, review.selected),
        View::LivePatch(live) => format!("cp-live-{}-{}", store.code_panel.request, live.selected),
    };
    let animated_body = scroll_enter(content_key, wrap, body.into_any_element(), window, cx);
    let scroll_id = SharedString::from(match view {
        View::Diff(review) => {
            format!("diff-body-{}-{}", store.code_panel.request, review.selected)
        }
        View::LivePatch(live) => {
            format!("live-body-{}-{}", store.code_panel.request, live.selected)
        }
        _ => format!("source-body-{}", store.code_panel.request),
    });
    let scroll_box = div()
        .id(scroll_id.clone())
        .min_h_0()
        .flex_1()
        .p_3p5()
        .font_family(qaqh_gpui_kit::fonts::mono_font_family())
        .text_size(px(ty.code_font_size))
        .child(animated_body);
    let scrollable = if wrap {
        scroll_box
            .overflow_y_scrollbar()
            .id(SharedString::from(format!("{scroll_id}-y")))
            .into_any_element()
    } else {
        scroll_box
            .overflow_scrollbar()
            .id(SharedString::from(format!("{scroll_id}-xy")))
            .into_any_element()
    };
    let body_viewport = div()
        .min_h_0()
        .flex_1()
        .flex()
        .flex_col()
        .child(scrollable)
        .context_menu(move |menu, window, cx| {
            let selected = TextSelection::selected_text(window, cx);
            let selected_trimmed = selected.trim().to_string();
            let has_selection = !selected_trimmed.is_empty();
            let copy_all = full_copy_text.clone();
            menu.item(
                PopupMenuItem::new("复制选中内容")
                    .icon(IconName::Copy)
                    .disabled(!has_selection)
                    .on_click(move |_, _, cx| {
                        if !selected_trimmed.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                selected_trimmed.clone(),
                            ));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("复制全部内容")
                    .icon(IconName::Copy)
                    .disabled(copy_all.is_empty())
                    .on_click(move |_, _, cx| {
                        if !copy_all.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                copy_all.clone(),
                            ));
                        }
                    }),
            )
        });

    let card = div()
        .w(px(sample.card_width))
        .h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .rounded(px(RADIUS_STAGE))
        .bg(t.stage)
        .border_1()
        .border_color(t.border_subtle)
        .shadow(shadow_stage(t))
        .overflow_hidden()
        .child(
            div()
                .h(px(42.))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_3p5()
                .bg(t.surface_subtle)
                .border_b_1()
                .border_color(t.border_subtle)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            Icon::new(IconName::FileCode)
                                .with_size(Size::XSmall)
                                .text_color(t.accent),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(title),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .flex_shrink_0()
                        .children(
                            match view {
                                View::Source(location, _) if !location.path.is_absolute() => {
                                    Some(location.clone())
                                }
                                _ => None,
                            }
                            .map(|location| {
                                div()
                                    .id("choose-source-root")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(t.btn)
                                    .text_xs()
                                    .cursor_pointer()
                                    .text_color(t.accent)
                                    .hover(|style| style.bg(t.btn_hover))
                                    .child("更换目录")
                                    .on_click(cx.listener(move |store, _, _, cx| {
                                        load_location(store, location.clone(), true, cx);
                                    }))
                            }),
                        )
                        .child(
                            div()
                                .id("toggle-code-wrap")
                                .size(px(24.))
                                .rounded(px(RADIUS_CHIP))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .when(wrap, |el| el.bg(t.accent_soft))
                                .hover(|style| style.bg(t.hover))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(if wrap {
                                        "自动换行：已开启（点击切换为横向滚动）"
                                    } else {
                                        "自动换行：已关闭（点击开启自动换行）"
                                    })
                                    .build(window, cx)
                                })
                                .child(
                                    Icon::new(IconName::TextWrap)
                                        .with_size(Size::XSmall)
                                        .text_color(if wrap { t.accent } else { t.dim }),
                                )
                                .on_click(cx.listener(|store, _, _, cx| {
                                    store.code_panel.wrap = !store.code_panel.wrap;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("close-code-panel")
                                .size(px(24.))
                                .rounded(px(RADIUS_CHIP))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(t.hover))
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("关闭文档面板")
                                        .build(window, cx)
                                })
                                .child(
                                    Icon::new(IconName::X)
                                        .with_size(Size::XSmall)
                                        .text_color(t.dim),
                                )
                                .on_click(cx.listener(|store, _, _, cx| {
                                    close(store, cx);
                                })),
                        ),
                ),
        )
        .children(sub_header)
        .child(body_viewport);

    Some(wrap_split_drawer(sample, card.into_any_element(), cx))
}
