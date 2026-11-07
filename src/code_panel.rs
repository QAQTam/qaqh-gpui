use std::io::Read;
use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{Context, Div, PathPromptOptions, SharedString, div, px};
use qaqh_ringing_contract::fact::{ContentValue, TurnId};
use qaqh_ringing_contract::state::ToolCallView;

use crate::{settings::Typography, store::SessionStore, theme::Palette};

#[derive(Clone)]
pub struct Location {
    path: PathBuf,
    line: usize,
    column: Option<usize>,
}

impl Location {
    pub fn parse(value: &str, require_line: bool) -> Option<Self> {
        let (path, line, column) = if let Some((path, line)) = value.rsplit_once("#L") {
            (path, line.parse().ok()?, None)
        } else if let Some((prefix, suffix)) = value.rsplit_once(':') {
            let last = suffix.parse().ok()?;
            match prefix.rsplit_once(':') {
                Some((path, row)) => (path, row.parse().ok()?, Some(last)),
                None => (prefix, last, None),
            }
        } else if require_line {
            return None;
        } else {
            (value, 1, None)
        };
        let path = Path::new(path);
        if line == 0
            || column == Some(0)
            || value.contains("://")
            || path.as_os_str().is_empty()
            || value.contains(['\n', '\r', '\0'])
            || !(path.extension().is_some() || path.components().count() > 1)
        {
            return None;
        }
        Some(Self {
            path: path.into(),
            line,
            column,
        })
    }
}

#[derive(Default)]
pub struct CodePanel {
    pub cwd: Option<PathBuf>,
    local_root: Option<PathBuf>,
    view: Option<View>,
    request: usize,
}

impl CodePanel {
    pub fn with_root(local_root: Option<PathBuf>) -> Self { Self { local_root, ..Self::default() } }
    pub fn local_root(&self) -> Option<PathBuf> { self.local_root.clone() }
}

enum View {
    Source(Location, Result<Excerpt, String>),
    Loading(Location),
    Diff(Review),
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
    rows: Vec<DiffRow>,
}

struct DiffRow {
    text: String,
    old: Option<usize>,
    new: Option<usize>,
}

fn load_source(root: &Path, location: &Location) -> std::io::Result<Excerpt> {
    let root = root.canonicalize()?;
    let path = root.join(&location.path).canonicalize()?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(std::io::Error::other("目标必须是所选目录内的普通文件"));
    }
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_BYTES || text.contains('\0') {
        return Err(std::io::Error::other("只支持 4 MiB 以内的 UTF-8 文本文件"));
    }
    let first_line = location.line.saturating_sub(20).max(1);
    let lines: Vec<_> = text
        .split('\n')
        .skip(first_line - 1)
        .take(101)
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
    cx.notify();

    let root = (!choose_root)
        .then(|| {
            store
                .code_panel
                .local_root
                .clone()
                .or_else(|| store.code_panel.cwd.clone())
        })
        .flatten();
    let picker = root.is_none().then(|| {
        cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择源码所在的本地目录".into()),
        })
    });
    let selecting = picker.is_some();
    cx.spawn(async move |session, cx| {
        let root = match picker {
            Some(picker) => match picker.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next().ok_or_else(|| "未选择目录".into()),
                Ok(Err(error)) => Err(error.to_string()),
                _ => Err("未选择目录".into()),
            },
            None => Ok(root.expect("已有本地目录")),
        };
        let result = match &root {
            Ok(root) => {
                let root = root.clone();
                let location = location.clone();
                cx.background_executor()
                    .spawn(async move {
                        load_source(&root, &location).map_err(|error| error.to_string())
                    })
                    .await
            }
            Err(error) => Err(error.clone()),
        };
        let _ = session.update(cx, |store, cx| {
            if store.code_panel.request == request {
                if selecting && let Ok(root) = root {
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
    tool.code_delta.is_some()
        || matches!(
            tool.tool_name.as_deref(),
            Some("write" | "edit" | "apply_patch")
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
        let deleted = new == "/dev/null";
        let path = if deleted {
            old.strip_prefix("a/").unwrap_or(old)
        } else {
            new.strip_prefix("b/").unwrap_or(new)
        };
        let mut rows = Vec::new();
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
            rows.push(DiffRow {
                text: header.into(),
                old: None,
                new: None,
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
                deleted,
                rows,
            });
        }
    }
    files
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
}

pub fn auto_review(store: &mut SessionStore, turn: TurnId) {
    open_review(store, turn, false);
}

fn open_review(store: &mut SessionStore, turn: TurnId, explicit: bool) {
    let mut patches = Vec::new();
    let mut references = Vec::new();
    for tool in store
        .tools
        .iter()
        .filter(|tool| tool.turn_id.as_ref() == Some(&turn) && is_edit(tool))
    {
        match &tool.output {
            Some(ContentValue::Inline { text }) => patches.extend(patches_from_output(text)),
            Some(ContentValue::Ref { content_ref }) => references.push(content_ref.clone()),
            _ => {}
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
}

pub fn finish_review(
    store: &mut SessionStore,
    turn: TurnId,
    request: usize,
    results: Vec<Result<String, String>>,
) {
    if store.code_panel.request == request
        && let Some(View::Diff(review)) = &mut store.code_panel.view
        && review.turn == turn
    {
        review.loading = false;
        for result in results {
            match result {
                Ok(text) => review.patches.extend(patches_from_output(&text)),
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
    Some(
        div()
            .id("review-latest-turn")
            .text_xs()
            .text_color(t.accent)
            .cursor_pointer()
            .child(format!("最近一轮改动 · +{added} −{removed}"))
            .on_click(cx.listener(move |store, _, _, cx| {
                open_review(store, turn.clone(), true);
                cx.notify();
            })),
    )
}

pub fn render(
    store: &SessionStore,
    t: &Palette,
    ty: &Typography,
    cx: &mut Context<SessionStore>,
) -> Option<Div> {
    let view = store.code_panel.view.as_ref()?;
    let (title, body) = match view {
        View::Loading(location) => (
            location.path.display().to_string(),
            div().child("正在读取源码…"),
        ),
        View::Source(location, result) => {
            let mut title = format!("{}:{}", location.path.display(), location.line);
            if let Some(column) = location.column {
                title.push_str(&format!(":{column}"));
            }
            let body = match result {
                Ok(excerpt) => {
                    let lang = location
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str());
                    div().children(excerpt.lines.iter().enumerate().map(|(index, line)| {
                        let number = excerpt.first_line + index;
                        div()
                            .flex()
                            .gap_2()
                            .whitespace_nowrap()
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
                            .child(crate::highlight::code_text(line, lang, t))
                    }))
                }
                Err(error) => div().text_color(t.warn).child(error.clone()),
            };
            (title, body)
        }
        View::Diff(review) => {
            let Review {
                patches, selected, ..
            } = review;
            let files = div()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(patches.iter().enumerate().map(|(index, patch)| {
                    div()
                        .id(("diff-file", index))
                        .cursor_pointer()
                        .text_color(if index == *selected { t.accent } else { t.dim })
                        .child(patch.path.clone())
                        .on_click(cx.listener(move |store, _, _, cx| {
                            if let Some(View::Diff(review)) = &mut store.code_panel.view {
                                review.selected = index;
                                cx.notify();
                            }
                        }))
                }));
            let mut body = div().child(files);
            if let Some(patch) = patches.get(*selected) {
                body =
                    body.children(patch.rows.iter().enumerate().map(|(index, row)| {
                        let color = match row.text.as_bytes().first() {
                            Some(b'+') => t.ok,
                            Some(b'-') => t.bad,
                            Some(b'@') => t.accent,
                            _ => t.text,
                        };
                        let path = PathBuf::from(&patch.path);
                        let line = row.new;
                        div()
                            .id(("diff-line", index))
                            .flex()
                            .gap_2()
                            .whitespace_nowrap()
                            .text_color(color)
                            .child(div().w(px(82.)).flex_shrink_0().text_color(t.dim).child(
                                format!(
                                    "{:>4} {:>4}",
                                    row.old.map(|line| line.to_string()).unwrap_or_default(),
                                    line.map(|line| line.to_string()).unwrap_or_default()
                                ),
                            ))
                            .child(row.text.clone())
                            .when_some(line.filter(|_| !patch.deleted), |element, line| {
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
                    }));
            } else if !review.loading && !review.failed {
                body = body.child("当前工具结果没有可显示的补丁正文");
            }
            if review.loading {
                body = body.child("正在读取工具结果…");
            }
            if review.failed {
                body = body.child(div().text_color(t.warn).child("部分工具结果读取失败"));
            }
            ("本轮改动".into(), body)
        }
    };
    Some(
        div()
            .w(px(420.))
            .max_w(gpui::relative(0.55))
            .min_h_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(t.border)
            .bg(t.panel)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .child(div().min_w_0().child(title))
                    .children(
                        match view {
                            View::Source(location, _) => Some(location.clone()),
                            _ => None,
                        }
                        .map(|location| {
                            div()
                                .id("choose-source-root")
                                .cursor_pointer()
                                .text_color(t.accent)
                                .child("更换目录")
                                .on_click(cx.listener(move |store, _, _, cx| {
                                    load_location(store, location.clone(), true, cx);
                                }))
                        }),
                    )
                    .child(
                        div()
                            .id("close-code-panel")
                            .cursor_pointer()
                            .child("关闭")
                            .on_click(cx.listener(|store, _, _, cx| {
                                store.code_panel.request += 1;
                                store.code_panel.view = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(match view {
                        View::Diff(review) => {
                            format!("diff-body-{}-{}", store.code_panel.request, review.selected)
                        }
                        _ => format!("source-body-{}", store.code_panel.request),
                    }))
                    .min_h_0()
                    .flex_1()
                    .overflow_scroll()
                    .p_3()
                    .font_family("monospace")
                    .text_size(px(ty.code_font_size))
                    .child(body),
            ),
    )
}
