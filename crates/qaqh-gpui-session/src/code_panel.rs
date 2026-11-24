use std::io::Read;
use std::path::{Path, PathBuf};

use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, PathPromptOptions, SharedString, div, px};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{
    RADIUS_CHIP, RADIUS_CONTROL, RADIUS_STAGE, shadow_stage, soft,
};
use qaqh_ringing_contract::fact::{ContentValue, TurnId};
use qaqh_ringing_contract::state::ToolCallView;

use qaqh_gpui_kit::{settings::Typography, theme::Palette};
use crate::store::SessionStore;

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
                deleted: deleted_header || !saw_new_lines,
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
        if let Some(diff_val) = &tool.diff {
            match diff_val {
                ContentValue::Inline { text } => patches.extend(parse_diff(text)),
                ContentValue::Ref { content_ref } => references.push(content_ref.clone()),
                ContentValue::Unavailable(_) => {}
            }
        } else {
            match &tool.output {
                Some(ContentValue::Inline { text }) => patches.extend(patches_from_output(text)),
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
                            .font_family("monospace")
                            .text_color(t.ok)
                            .child(SharedString::from(format!("+{added}"))),
                    )
                    .child(
                        div()
                            .font_family("monospace")
                            .text_color(t.bad)
                            .child(SharedString::from(format!("−{removed}"))),
                    )
                    .on_click(cx.listener(move |store, _, _, cx| {
                        open_review(store, turn.clone(), true);
                        cx.notify();
                    })),
            ),
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
                            .child(qaqh_gpui_render::highlight::code_text(line, lang, t))
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
                .gap_1p5()
                .pb_2p5()
                .children(patches.iter().enumerate().map(|(index, patch)| {
                    let active = index == *selected;
                    div()
                        .id(("diff-file", index))
                        .px_2()
                        .py_1()
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
                            el.bg(t.surface_subtle)
                                .text_color(t.dim)
                                .hover(|style| style.bg(t.hover).text_color(t.text))
                        })
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
                        let (color, row_bg) = match row.text.as_bytes().first() {
                            Some(b'+') => (t.ok, Some(soft(t.ok, 0.08))),
                            Some(b'-') => (t.bad, Some(soft(t.bad, 0.08))),
                            Some(b'@') => (t.accent, Some(soft(t.accent, 0.07))),
                            _ => (t.text, None),
                        };
                        let path = PathBuf::from(&patch.path);
                        let line = row.new;
                        div()
                            .id(("diff-line", index))
                            .flex()
                            .gap_2()
                            .px_1p5()
                            .rounded(px(4.))
                            .whitespace_nowrap()
                            .text_color(color)
                            .when_some(row_bg, |el, bg| el.bg(bg))
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
            .ml_2()
            .w(px(440.))
            .max_w(gpui_kit::relative(0.50))
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
                                    View::Source(location, _) => Some(location.clone()),
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
                                    .id("close-code-panel")
                                    .size(px(24.))
                                    .rounded(px(RADIUS_CHIP))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(t.hover))
                                    .child(
                                        Icon::new(IconName::X)
                                            .with_size(Size::XSmall)
                                            .text_color(t.dim),
                                    )
                                    .on_click(cx.listener(|store, _, _, cx| {
                                        store.code_panel.request += 1;
                                        store.code_panel.view = None;
                                        cx.notify();
                                    })),
                            ),
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
                    .p_3p5()
                    .font_family("monospace")
                    .text_size(px(ty.code_font_size))
                    .child(body),
            ),
    )
}
