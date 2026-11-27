//! Markdown 渲染(M6 · Plan B):基于 `gpui_kit::base::text::TextView` 原生排版引擎。
//! - 原生跨段文本拖选、双击选词、三击选段与复制;
//! - 行内 `` `code` `` 等宽字体 + `0.875x` 缩字号 + `code_fg` 微色差 + 4px 圆角描边胶囊;
//! - H1/H2 标题底部分割线;
//! - 通过 `MarkdownPlugin` 保留文件/网址 Lucide 图标联动(`code_panel` / 浏览器)与 `widget` 卡片,
//!   以及 `highlight.rs` AST 语法高亮 + 右上角复制操作条。

use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::HoverCard;
use gpui_kit::base::text::{
    CodeBlock, InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode,
    MarkdownParseContext, MarkdownPlugin, TextView, TextViewStyle, markdown_ast,
};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, ClipboardItem, Div, FontWeight,
    HighlightStyle, Hsla, Overflow, SharedString, Stateful, StyleRefinement, WeakEntity, Window,
    div, px, relative, rems,
};
use qaqh_gpui_kit::icons::{AppIcon, file_icon, render_app_icon};
use qaqh_gpui_kit::kit::{
    RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, RADIUS_POPOVER, shadow_card, shadow_popover, soft,
};
use qaqh_gpui_kit::motion::effective_motion;
use qaqh_gpui_kit::settings::{MotionLevel, Typography};
use qaqh_gpui_kit::theme::{ActiveTheme, Palette};

use crate::store::SessionStore;

const LINK_ICON_NODE: &str = "qaqh-link-icon";
const WIDGET_NODE: &str = "qaqh-widget";
const MAX_PILL_CHARS: usize = 72;

/// 顶层 Markdown 宽表格相对正文栏宽的对称拓展上限(左右各 +15px,总 +30px)。
pub const TABLE_BREAKOUT_EXTRA_PX: f32 = 30.0;
/// 超过该列数才启用横向滚动兜底;常规 2~4 列表格默认积极折行、一屏完整展示。
const WIDE_TABLE_SCROLL_COLS: usize = 5;

#[derive(Clone, PartialEq, Eq)]
enum LinkTarget {
    Web(String),
    GitHub(crate::github_preview::GitHubRef),
    Source(crate::code_panel::Location),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IconRole {
    Web,
    Dim,
    Alert,
    Code,
    Type,
    Shell,
    Media,
}

impl IconRole {
    fn color(self, t: &Palette) -> Hsla {
        match self {
            Self::Web => t.syn_function,
            Self::Dim => t.dim,
            Self::Alert => t.bad,
            Self::Code => t.syn_keyword,
            Self::Type => t.syn_type,
            Self::Shell => t.syn_string,
            Self::Media => t.syn_number,
        }
    }
}

#[derive(Clone)]
struct LinkIconData {
    label: String,
    target: LinkTarget,
    is_code: bool,
}

fn is_http_url(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
        || url
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
}

fn classify_href(url: &str) -> Option<LinkTarget> {
    if let Some(gh) = crate::github_preview::GitHubRef::parse_url(url) {
        Some(LinkTarget::GitHub(gh))
    } else if is_http_url(url) {
        Some(LinkTarget::Web(url.to_owned()))
    } else {
        crate::code_panel::Location::parse(url, false).map(LinkTarget::Source)
    }
}

fn web_host(url: &str) -> &str {
    let rest = url
        .split_once("://")
        .map(|(_, after)| after)
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.split(':').next().unwrap_or("")
}

fn link_icons(target: &LinkTarget) -> ((AppIcon, IconRole), Option<(IconName, IconRole)>) {
    match target {
        LinkTarget::GitHub(_) => (
            (AppIcon::Lucide(IconName::Github), IconRole::Web),
            Some((IconName::ExternalLink, IconRole::Dim)),
        ),
        LinkTarget::Web(url) => {
            let host = web_host(url).to_ascii_lowercase();
            let lead = if host == "github.com" || host.ends_with(".github.com") {
                (AppIcon::Lucide(IconName::Github), IconRole::Web)
            } else {
                (AppIcon::Lucide(IconName::Globe), IconRole::Web)
            };
            (lead, Some((IconName::ExternalLink, IconRole::Dim)))
        }
        LinkTarget::Source(loc) => {
            let path = loc.path();
            if let Some(brand) = file_icon(path) {
                return ((AppIcon::Brand(brand), IconRole::Dim), None);
            }
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let lead = if file_name.starts_with("license")
                || file_name.starts_with("copying")
                || file_name.starts_with("notice")
            {
                (IconName::Copyright, IconRole::Dim)
            } else if matches!(
                file_name.as_str(),
                ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig"
            ) {
                (IconName::GitBranch, IconRole::Alert)
            } else if matches!(
                file_name.as_str(),
                "cargo.toml"
                    | "cargo.lock"
                    | "package.json"
                    | "tsconfig.json"
                    | "dockerfile"
                    | "makefile"
                    | "justfile"
            ) || matches!(
                ext.as_str(),
                "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env" | "lock"
            ) {
                (IconName::FileCog, IconRole::Dim)
            } else if matches!(ext.as_str(), "md" | "mdx" | "markdown" | "rst" | "txt" | "adoc") {
                (IconName::FileText, IconRole::Web)
            } else if matches!(ext.as_str(), "json" | "jsonc" | "jsonl") {
                (IconName::FileBraces, IconRole::Type)
            } else if matches!(
                ext.as_str(),
                "rs" | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "mjs"
                    | "cjs"
                    | "py"
                    | "go"
                    | "c"
                    | "h"
                    | "cpp"
                    | "hpp"
                    | "java"
                    | "kt"
                    | "swift"
                    | "cs"
                    | "rb"
                    | "php"
                    | "lua"
                    | "zig"
                    | "css"
                    | "scss"
                    | "html"
                    | "xml"
                    | "vue"
                    | "svelte"
            ) {
                (IconName::FileCode, IconRole::Code)
            } else if matches!(
                ext.as_str(),
                "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "bat" | "cmd"
            ) {
                (IconName::FileTerminal, IconRole::Shell)
            } else if matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp"
            ) {
                (IconName::FileImage, IconRole::Media)
            } else if matches!(ext.as_str(), "sql" | "db" | "sqlite" | "sqlite3") {
                (IconName::Database, IconRole::Type)
            } else if matches!(ext.as_str(), "diff" | "patch") {
                (IconName::FileDiff, IconRole::Shell)
            } else if matches!(ext.as_str(), "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar") {
                (IconName::FileArchive, IconRole::Dim)
            } else if matches!(ext.as_str(), "csv" | "tsv" | "xlsx") {
                (IconName::FileSpreadsheet, IconRole::Shell)
            } else {
                (IconName::FileText, IconRole::Dim)
            };
            ((AppIcon::Lucide(lead.0), lead.1), None)
        }
    }
}

fn collect_plain_text(nodes: &[markdown_ast::Node], out: &mut String) {
    for node in nodes {
        match node {
            markdown_ast::Node::Text(text) => out.push_str(&text.value),
            markdown_ast::Node::InlineCode(code) => out.push_str(&code.value),
            markdown_ast::Node::Break(_) => out.push(' '),
            other => {
                if let Some(children) = other.children() {
                    collect_plain_text(children, out);
                }
            }
        }
    }
}

/// widget marker 判定:`U+E200 visualize U+E202 {json} U+E201` 独占行。
fn widget_line(line: &str) -> Option<Result<(String, Option<String>), ()>> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('\u{E200}')?;
    let rest = rest.strip_prefix(" visualize ")?;
    let rest = rest.strip_prefix('\u{E202}')?;
    let payload = rest.strip_suffix('\u{E201}')?;
    match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => Some(Ok((
            value
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            value
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        ))),
        Err(_) => Some(Err(())),
    }
}

struct LinkIconPlugin {
    session: WeakEntity<SessionStore>,
}

impl MarkdownPlugin for LinkIconPlugin {
    fn name(&self) -> &str {
        LINK_ICON_NODE
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        match node {
            markdown_ast::Node::Link(link) => {
                let target = classify_href(&link.url)?;
                let mut raw_label = String::new();
                collect_plain_text(&link.children, &mut raw_label);
                let trimmed = raw_label.trim();
                let label = match &target {
                    LinkTarget::GitHub(gh)
                        if trimmed.is_empty()
                            || is_http_url(trimmed)
                            || trimmed.chars().count() > MAX_PILL_CHARS =>
                    {
                        gh.short_label()
                    }
                    _ => {
                        if trimmed.is_empty() || trimmed.chars().count() > MAX_PILL_CHARS {
                            return None;
                        }
                        trimmed.to_owned()
                    }
                };
                let is_code =
                    matches!(link.children.as_slice(), [markdown_ast::Node::InlineCode(_)]);
                let data = LinkIconData {
                    label: label.clone(),
                    target,
                    is_code,
                };
                Some(MarkdownNode::new(LINK_ICON_NODE, data).text(label))
            }
            markdown_ast::Node::InlineCode(code) => {
                if code.value.chars().count() > MAX_PILL_CHARS {
                    return None;
                }
                let location = crate::code_panel::Location::parse(&code.value, true)?;
                let data = LinkIconData {
                    label: code.value.clone(),
                    target: LinkTarget::Source(location),
                    is_code: true,
                };
                Some(MarkdownNode::new(LINK_ICON_NODE, data).text(code.value.clone()))
            }
            _ => None,
        }
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let data = node.data::<LinkIconData>()?.clone();
        let t = *cx.colors();
        let id_offset = node.source_range().map_or(0, |r| r.start);
        if let LinkTarget::GitHub(gh_ref) = data.target {
            return Some(render_github_pill(
                gh_ref,
                data.label,
                id_offset,
                context,
                self.session.clone(),
                &t,
                cx,
            ));
        }

        let ((lead_icon, lead_role), trail) = link_icons(&data.target);
        let is_web = matches!(data.target, LinkTarget::Web(_));

        let label_view = if data.is_code {
            div()
                .px(px(4.))
                .py(px(1.))
                .rounded(px(4.))
                .bg(t.code_bg)
                .border_1()
                .border_color(t.border_subtle)
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_color(t.code_fg)
                .child(SharedString::from(data.label))
                .into_any_element()
        } else {
            div()
                .text_color(if is_web { t.syn_function } else { t.text })
                .underline()
                .child(SharedString::from(data.label))
                .into_any_element()
        };

        let target = data.target;
        let session = self.session.clone();
        let mut row = div()
            .id(("md-link-icon", id_offset))
            .px(px(2.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .gap(px(3.))
            .text_size(context.font_size())
            .line_height(context.line_height())
            .cursor_pointer()
            .hover(|style| style.bg(t.hover))
            .on_click(move |_, _, cx| match &target {
                LinkTarget::Web(url) => cx.open_url(url),
                LinkTarget::GitHub(gh) => cx.open_url(&gh.url),
                LinkTarget::Source(location) => {
                    let location = location.clone();
                    let _ = session.update(cx, |store, cx| {
                        crate::code_panel::open_source(store, location, cx);
                    });
                }
            })
            .child(render_app_icon(lead_icon, 13.0, lead_role.color(&t)))
            .child(label_view);

        if let Some((trail_icon, trail_role)) = trail {
            row = row.child(
                Icon::new(trail_icon)
                    .with_size(Size::XSmall)
                    .text_color(trail_role.color(&t)),
            );
        }

        Some(InlineElement::new(row))
    }
}

fn github_state_visual(
    kind: crate::github_preview::GitHubKind,
    state: crate::github_preview::GitHubItemState,
    t: &Palette,
) -> (IconName, Hsla) {
    use crate::github_preview::{GitHubItemState, GitHubKind};
    match (kind, state) {
        (GitHubKind::Pull, GitHubItemState::Open) => (IconName::GitPullRequest, t.ok),
        (GitHubKind::Pull, GitHubItemState::Merged) => (IconName::GitMerge, t.syn_keyword),
        (GitHubKind::Pull, GitHubItemState::Closed) => (IconName::GitPullRequestClosed, t.bad),
        (GitHubKind::Pull, GitHubItemState::Draft) => (IconName::GitPullRequestDraft, t.dim),
        (GitHubKind::Issue, GitHubItemState::Open) => (IconName::CircleDot, t.ok),
        (GitHubKind::Issue, GitHubItemState::Merged | GitHubItemState::Closed) => {
            (IconName::CircleCheck, t.syn_keyword)
        }
        (GitHubKind::Issue, GitHubItemState::Draft) => (IconName::CircleDot, t.dim),
    }
}

fn render_github_pill(
    gh_ref: crate::github_preview::GitHubRef,
    label: String,
    id_offset: usize,
    context: &InlineRenderContext,
    session: WeakEntity<SessionStore>,
    t: &Palette,
    cx: &App,
) -> InlineElement {
    let (ready_meta, is_loading) = crate::github_preview::snapshot(cx, &gh_ref);
    let animate = effective_motion(cx) != MotionLevel::Off;

    let lead_icon: AnyElement = if is_loading {
        let spinner = Icon::new(IconName::LoaderCircle)
            .with_size(Size::XSmall)
            .text_color(t.dim);
        if animate {
            spinner
                .with_animation(
                    SharedString::from(format!("gh-spin-{}-{}", id_offset, gh_ref.number)),
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |icon, delta| icon.rotate(gpui_kit::Radians(delta * std::f32::consts::TAU)),
                )
                .into_any_element()
        } else {
            spinner.into_any_element()
        }
    } else if let Some(meta) = &ready_meta {
        let (icon_name, color) = github_state_visual(meta.reference.kind, meta.state, t);
        Icon::new(icon_name)
            .with_size(Size::XSmall)
            .text_color(color)
            .into_any_element()
    } else {
        Icon::new(IconName::Github)
            .with_size(Size::XSmall)
            .text_color(t.dim)
            .into_any_element()
    };

    let gh_for_hover = gh_ref.clone();
    let url_for_click = gh_ref.url.clone();
    let pill = div()
        .id(("md-gh-pill", id_offset ^ (gh_ref.number as usize)))
        .px(px(5.))
        .py(px(1.))
        .rounded(px(RADIUS_CHIP))
        .bg(t.surface_subtle)
        .border_1()
        .border_color(t.border_subtle)
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(context.font_size())
        .line_height(context.line_height())
        .cursor_pointer()
        .hover(|style| style.bg(t.hover))
        .on_hover(move |hovered, _window, cx| {
            if *hovered {
                crate::github_preview::request_on_hover(
                    gh_for_hover.clone(),
                    session.clone(),
                    cx,
                );
            }
        })
        .on_click(move |_, _, cx| cx.open_url(&url_for_click))
        .child(lead_icon)
        .child(
            div()
                .text_color(t.syn_function)
                .child(SharedString::from(label)),
        );

    let card_id = SharedString::from(format!(
        "gh-hover-{}-{}-{}-{}",
        gh_ref.owner, gh_ref.repo, gh_ref.number, id_offset
    ));
    let hover_card = HoverCard::new(card_id)
        .anchor(Anchor::TopLeft)
        .open_delay(Duration::from_millis(220))
        .close_delay(Duration::from_millis(260))
        .trigger(pill)
        .when_some(ready_meta, |card, meta| {
            card.content(move |_, _, cx| render_github_hover_card(&meta, cx))
        });

    InlineElement::new(hover_card)
}

fn render_github_hover_card(
    meta: &crate::github_preview::GitHubItemMeta,
    cx: &App,
) -> Stateful<Div> {
    let t = *cx.colors();
    let (state_icon, state_color) = github_state_visual(meta.reference.kind, meta.state, &t);
    let url = meta.reference.url.clone();

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .id("gh-card-repo-link")
                .flex()
                .items_center()
                .gap_1p5()
                .min_w_0()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(
                    Icon::new(IconName::Github)
                        .with_size(Size::XSmall)
                        .text_color(t.text),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.syn_function)
                        .child(SharedString::from(meta.reference.full_label())),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(1.))
                .rounded_full()
                .bg(soft(state_color, 0.14))
                .border_1()
                .border_color(soft(state_color, 0.28))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(state_color)
                .child(
                    Icon::new(state_icon)
                        .with_size(Size::XSmall)
                        .text_color(state_color),
                )
                .child(meta.state.label()),
        );

    let mut card = div()
        .id("gh-pr-hover-card")
        .w(px(360.))
        .max_w(px(400.))
        .p_3()
        .rounded(px(RADIUS_POPOVER))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .shadow(shadow_popover(&t))
        .flex()
        .flex_col()
        .gap_2()
        .child(header)
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(SharedString::from(meta.title.clone())),
        );

    if meta.branch.is_some() || meta.author.is_some() {
        let mut rows = div().flex().flex_col().gap_1();
        if let Some((head, base)) = &meta.branch {
            rows = rows.child(github_meta_row(
                "Branch",
                div()
                    .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                    .text_xs()
                    .text_color(t.text)
                    .child(SharedString::from(format!("{head} → {base}"))),
                &t,
            ));
        }
        if let Some(author) = &meta.author {
            rows = rows.child(github_meta_row(
                "Author",
                div()
                    .text_xs()
                    .text_color(t.text)
                    .child(SharedString::from(format!("@{author}"))),
                &t,
            ));
        }
        card = card.child(rows);
    }

    if let Some(summary) = &meta.summary {
        card = card
            .child(div().h(px(1.)).w_full().bg(t.border_subtle))
            .child(
                div()
                    .text_xs()
                    .text_color(t.dim)
                    .child(SharedString::from(summary.clone())),
            );
    }

    card
}

fn github_meta_row(label: &'static str, value: impl IntoElement, t: &Palette) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .w(px(52.))
                .flex_shrink_0()
                .text_xs()
                .text_color(t.faint)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(value))
}

struct WidgetBlockPlugin;

impl MarkdownPlugin for WidgetBlockPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        WIDGET_NODE
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        if !matches!(node, markdown_ast::Node::Paragraph(_)) {
            return None;
        }
        let source = cx.node_source(node)?;
        let lines: Vec<&str> = source.lines().collect();
        if lines.is_empty() || !lines.iter().all(|line| widget_line(line).is_some()) {
            return None;
        }
        let widgets: Vec<Result<(String, Option<String>), ()>> =
            lines.into_iter().filter_map(widget_line).collect();
        Some(MarkdownNode::new(WIDGET_NODE, widgets).text(source.to_owned()))
    }

    fn render(&self, node: &MarkdownNode, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(widgets) = node.data::<Vec<Result<(String, Option<String>), ()>>>() else {
            return div().into_any_element();
        };
        let t = *cx.colors();
        let base_offset = node.source_range().map_or(0, |r| r.start);
        let mut col = div().flex().flex_col().gap_2();
        for (idx, item) in widgets.iter().enumerate() {
            let card = match item {
                Ok((path, title)) => {
                    let label = title.clone().unwrap_or_else(|| {
                        std::path::Path::new(path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "widget".into())
                    });
                    let mut card = div()
                        .id(("widget-card", base_offset + idx))
                        .my_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_3p5()
                        .py_3()
                        .rounded(px(RADIUS_CARD))
                        .border_1()
                        .border_color(t.border_subtle)
                        .bg(t.card)
                        .shadow(shadow_card(&t))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(24.))
                                        .rounded(px(RADIUS_CHIP))
                                        .bg(t.accent_soft)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            Icon::new(IconName::Sparkles)
                                                .with_size(Size::XSmall)
                                                .text_color(t.accent),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text)
                                        .truncate()
                                        .child(SharedString::from(label)),
                                ),
                        );
                    if path.is_empty() {
                        card = card.child(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .child("(widget 指令解析失败)"),
                        );
                    } else {
                        let url = format!(
                            "file:///{}",
                            path.replace('\\', "/").trim_start_matches('/')
                        );
                        card = card.child(
                            div()
                                .id(("widget-open", base_offset + idx))
                                .self_start()
                                .px_2p5()
                                .py_1()
                                .rounded(px(RADIUS_CHIP))
                                .bg(t.accent_soft)
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.accent)
                                .cursor_pointer()
                                .hover(|style| style.bg(t.accent_bg))
                                .on_click(move |_, _, cx: &mut App| {
                                    cx.open_url(&url);
                                })
                                .child("在浏览器中打开 →"),
                        );
                    }
                    card
                }
                Err(()) => div()
                    .id(("widget-broken", base_offset + idx))
                    .my_1()
                    .px_3p5()
                    .py_2()
                    .rounded(px(RADIUS_CARD))
                    .border_1()
                    .border_color(t.border_subtle)
                    .bg(t.card)
                    .text_xs()
                    .text_color(t.dim)
                    .child("(widget 指令解析失败)"),
            };
            col = col.child(card);
        }
        col.into_any_element()
    }
}

/// 每个会话构建一次的稳定 Markdown 扩展配置(避免每帧触发 AST 重解析)。
pub fn extensions(session: WeakEntity<SessionStore>) -> Arc<MarkdownExtensions> {
    Arc::new(
        MarkdownExtensions::default()
            .raw_html(false)
            .plugin(LinkIconPlugin { session })
            .plugin(WidgetBlockPlugin),
    )
}

#[derive(PartialEq)]
struct StyleCacheKey {
    stage: Hsla,
    text: Hsla,
    code_bg: Hsla,
    code_fg: Hsla,
    border_subtle: Hsla,
    selection_wash: Hsla,
}

type CodeBlockHighlighter =
    Arc<dyn Fn(&CodeBlock) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync>;

struct CachedViewTheme {
    key: StyleCacheKey,
    doc_style: TextViewStyle,
    wide_table_style: TextViewStyle,
    compact_style: TextViewStyle,
    highlighter: CodeBlockHighlighter,
}

thread_local! {
    static THEME_CACHE: RefCell<Option<CachedViewTheme>> = const { RefCell::new(None) };
}

fn build_text_view_style(t: &Palette, compact: bool, scroll_tables: bool) -> TextViewStyle {
    let border_subtle = t.border_subtle;
    let inline_code_style = StyleRefinement::default()
        .bg(t.code_bg)
        .text_color(t.code_fg)
        .border_1()
        .border_color(border_subtle)
        .rounded(px(4.))
        .px(px(4.))
        .py(px(1.));
    let blockquote_style = StyleRefinement::default()
        .bg(t.surface_subtle)
        .rounded(px(RADIUS_CONTROL))
        .px_3()
        .py_2()
        .border_l_2()
        .border_color(soft(t.accent, 0.55));
    let mut code_block = StyleRefinement::default()
        .rounded(px(RADIUS_CARD))
        .border_color(border_subtle);
    code_block.padding.top = Some(px(34.).into());

    let mut table = StyleRefinement::default().rounded(px(RADIUS_CONTROL));
    if scroll_tables {
        table.overflow.x = Some(Overflow::Scroll);
    }
    let table_head = StyleRefinement::default()
        .bg(t.surface_subtle)
        .text_color(t.text);
    let table_cell = StyleRefinement::default().px(px(10.)).py(px(6.));

    TextViewStyle::default()
        .with_foreground(t.text)
        .with_muted_foreground(t.dim)
        .with_link(t.syn_function)
        .with_selection(t.selection_wash)
        .with_code_background(t.code_bg)
        .with_border(border_subtle)
        .with_paragraph_gap(rems(if compact { 0.25 } else { 0.6 }))
        .with_heading(move |level| match level {
            1 | 2 if !compact => StyleRefinement::default()
                .pb(px(6.))
                .mb(px(6.))
                .border_b_1()
                .border_color(border_subtle),
            _ => StyleRefinement::default(),
        })
        .with_inline_code(HighlightStyle {
            color: Some(t.code_fg),
            background_color: Some(t.code_bg),
            ..Default::default()
        })
        .with_inline_code_style(inline_code_style)
        .with_blockquote_style(blockquote_style)
        .with_code_block(code_block)
        .with_table(table)
        .with_table_head(table_head)
        .with_table_cell(table_cell)
        .with_table_row_hover(t.hover)
        .with_dark(t.bg.l <= 0.5)
}

fn cached_theme_assets(
    t: &Palette,
) -> (TextViewStyle, TextViewStyle, TextViewStyle, CodeBlockHighlighter) {
    let key = StyleCacheKey {
        stage: t.stage,
        text: t.text,
        code_bg: t.code_bg,
        code_fg: t.code_fg,
        border_subtle: t.border_subtle,
        selection_wash: t.selection_wash,
    };
    THEME_CACHE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(cached) = slot.as_ref()
            && cached.key == key
        {
            return (
                cached.doc_style.clone(),
                cached.wide_table_style.clone(),
                cached.compact_style.clone(),
                cached.highlighter.clone(),
            );
        }
        let palette = *t;
        let doc_style = build_text_view_style(&palette, false, false);
        let wide_table_style = build_text_view_style(&palette, false, true);
        let compact_style = build_text_view_style(&palette, true, false);
        let highlighter: CodeBlockHighlighter = Arc::new(move |block| {
            let lang = block.lang();
            qaqh_gpui_render::highlight::highlight_spans(
                block.code().as_ref(),
                lang.as_deref(),
                &palette,
            )
        });
        *slot = Some(CachedViewTheme {
            key,
            doc_style: doc_style.clone(),
            wide_table_style: wide_table_style.clone(),
            compact_style: compact_style.clone(),
            highlighter: highlighter.clone(),
        });
        (doc_style, wide_table_style, compact_style, highlighter)
    })
}

fn attach_link_handler(
    view: TextView,
    session: WeakEntity<SessionStore>,
) -> TextView {
    view.on_link_click(move |url, _, _, cx| {
        if is_http_url(url.as_ref()) {
            cx.open_url(url.as_ref());
        } else if let Some(location) = crate::code_panel::Location::parse(url.as_ref(), false) {
            let _ = session.update(cx, |store, cx| {
                crate::code_panel::open_source(store, location, cx);
            });
        }
    })
}

#[derive(Debug, PartialEq, Eq)]
enum MarkdownSegment {
    Prose(String),
    Table { markdown: String, cols: usize },
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let leading = line.bytes().take_while(|&b| b == b' ').count();
    if leading > 3 {
        return None;
    }
    let rest = line.trim_start_matches(' ');
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = rest.chars().take_while(|&c| c == first).count();
    (count >= 3).then_some((first, count))
}

fn is_fence_close(line: &str, ch: char, min_len: usize) -> bool {
    let leading = line.bytes().take_while(|&b| b == b' ').count();
    if leading > 3 {
        return false;
    }
    let rest = line.trim_start_matches(' ');
    let count = rest.chars().take_while(|&c| c == ch).count();
    count >= min_len && rest.chars().skip(count).all(|c| c == ' ' || c == '\t')
}

fn split_table_cells(line: &str) -> Vec<&str> {
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    trimmed.split('|').collect()
}

fn parse_table_delimiter_cols(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    let cells = split_table_cells(trimmed);
    if cells.len() < 2 {
        return None;
    }
    for cell in &cells {
        let cell = cell.trim();
        if cell.is_empty() {
            return None;
        }
        let inner = cell.strip_prefix(':').unwrap_or(cell);
        let inner = inner.strip_suffix(':').unwrap_or(inner);
        if inner.is_empty() || !inner.bytes().all(|b| b == b'-') {
            return None;
        }
    }
    Some(cells.len())
}

fn is_table_body_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.contains('|')
        && !trimmed.starts_with('#')
        && fence_marker(line).is_none()
}

/// 将顶层 GFM 表格从 Markdown 文档中拆出,便于主对话舞台对表格做对称破栏宽出。
/// 代码块(``` / ~~~)内的管道符会被完整跳过,不会被误识别为表格。
fn split_markdown_tables(source: &str) -> Vec<MarkdownSegment> {
    let lines: Vec<&str> = source.lines().collect();
    let mut segments = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;

    let flush_prose = |prose: &mut Vec<&str>, segments: &mut Vec<MarkdownSegment>| {
        if prose.iter().any(|l| !l.trim().is_empty()) {
            segments.push(MarkdownSegment::Prose(prose.join("\n")));
        }
        prose.clear();
    };

    while i < lines.len() {
        let line = lines[i];
        if let Some((ch, len)) = fence {
            prose.push(line);
            if is_fence_close(line, ch, len) {
                fence = None;
            }
            i += 1;
            continue;
        }
        if let Some(open) = fence_marker(line) {
            fence = Some(open);
            prose.push(line);
            i += 1;
            continue;
        }
        if line.contains('|')
            && let Some(next) = lines.get(i + 1)
            && let Some(cols) = parse_table_delimiter_cols(next)
            && split_table_cells(line).len() == cols
        {
            flush_prose(&mut prose, &mut segments);
            let mut table_lines = vec![line, *next];
            i += 2;
            while i < lines.len() && is_table_body_line(lines[i]) {
                table_lines.push(lines[i]);
                i += 1;
            }
            segments.push(MarkdownSegment::Table {
                markdown: table_lines.join("\n"),
                cols,
            });
            continue;
        }
        prose.push(line);
        i += 1;
    }
    flush_prose(&mut prose, &mut segments);
    segments
}

fn build_document_view(
    id: SharedString,
    source: String,
    style: TextViewStyle,
    highlighter: CodeBlockHighlighter,
    extensions: &Arc<MarkdownExtensions>,
    session: WeakEntity<SessionStore>,
    ty: &Typography,
) -> TextView {
    let view = TextView::markdown(id, source)
        .style(style)
        .markdown_extensions((**extensions).clone())
        .shared_code_block_highlighter(highlighter)
        .code_block_actions(|block, _, cx| {
            let t = *cx.colors();
            let lang = block
                .lang()
                .filter(|lang| !lang.is_empty())
                .unwrap_or_else(|| "text".into());
            let copy_code = block.code();
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(RADIUS_CHIP))
                        .bg(t.btn)
                        .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                        .text_xs()
                        .text_color(t.dim)
                        .child(lang),
                )
                .child(
                    div()
                        .id("copy-code-block")
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_0p5()
                        .rounded(px(RADIUS_CHIP))
                        .text_xs()
                        .cursor_pointer()
                        .bg(t.card)
                        .border_1()
                        .border_color(t.border_subtle)
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.hover).text_color(t.text))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_code.to_string()));
                        })
                        .child(Icon::new(IconName::Copy).with_size(Size::XSmall).text_color(t.dim))
                        .child("复制"),
                )
        })
        .selectable(true)
        .text_sm()
        .line_height(relative(ty.prose_line_height));

    attach_link_handler(view, session)
}

/// 渲染正文 Markdown 文档(原生跨段选字、行内胶囊代码、标题分割线、图标联动与 AST 代码块)。
pub fn render_document(
    id: impl Into<SharedString>,
    source: &str,
    extensions: &Arc<MarkdownExtensions>,
    session: WeakEntity<SessionStore>,
    t: &Palette,
    ty: &Typography,
) -> AnyElement {
    let (doc_style, _, _, highlighter) = cached_theme_assets(t);
    build_document_view(
        id.into(),
        source.to_owned(),
        doc_style,
        highlighter,
        extensions,
        session,
        ty,
    )
    .into_any_element()
}

/// 估算 GFM 表格的自然总宽(px),用于让 2~3 列短表紧贴内容自适应收束,不拉满整行大空白;
/// 长内容表格自动放大至栏宽上限并在单元格内自动换行。
fn estimate_table_natural_width(markdown: &str, cols: usize) -> f32 {
    let mut col_max = vec![48.0_f32; cols.max(1)];
    for (row_ix, line) in markdown.lines().enumerate() {
        if row_ix == 1 && parse_table_delimiter_cols(line).is_some() {
            continue;
        }
        for (col_ix, cell) in split_table_cells(line).into_iter().take(cols).enumerate() {
            let trimmed = cell.trim();
            let mut text_w = 0.0_f32;
            let mut has_inline_code = false;
            for ch in trimmed.chars() {
                if ch == '`' {
                    has_inline_code = true;
                    continue;
                }
                text_w += if ch.is_ascii() { 8.0 } else { 15.0 };
            }
            if has_inline_code {
                text_w += 16.0;
            }
            let cell_w = (text_w * 1.15 + 24.0).max(56.0);
            if cell_w > col_max[col_ix] {
                col_max[col_ix] = cell_w;
            }
        }
    }
    col_max.into_iter().sum::<f32>() + 16.0
}

/// 主对话正文 Markdown 渲染(支持顶层表格自适应排版):
/// - 普通正文守住 `MESSAGE_MAX_WIDTH_PX`(880px)最佳阅读行宽,窄窗口自动压缩;
/// - 2~3 列表格贴合正文左缘按内容估算宽度自适应收束(不超过 880px),长单元格自动换行;
/// - 4+ 列表格最多对称向正文左右各拓展 15px(`TABLE_BREAKOUT_EXTRA_PX = 30px`),窄屏自动收缩;
/// - 5+ 列超宽表先按行宽折行,超出最小列宽后启用横向滚动兜底与图标操作条。
pub fn render_document_breakout(
    id: impl Into<SharedString>,
    source: &str,
    extensions: &Arc<MarkdownExtensions>,
    session: WeakEntity<SessionStore>,
    t: &Palette,
    ty: &Typography,
) -> AnyElement {
    let base_id = id.into();
    let prose_max = px(qaqh_gpui_kit::settings::MESSAGE_MAX_WIDTH_PX);
    let table_max = px(qaqh_gpui_kit::settings::MESSAGE_MAX_WIDTH_PX + TABLE_BREAKOUT_EXTRA_PX);
    let segments = split_markdown_tables(source);

    if !segments
        .iter()
        .any(|seg| matches!(seg, MarkdownSegment::Table { .. }))
    {
        return div()
            .w_full()
            .max_w(prose_max)
            .mx_auto()
            .px_4()
            .child(render_document(base_id, source, extensions, session, t, ty))
            .into_any_element();
    }

    let (doc_style, wide_table_style, _, highlighter) = cached_theme_assets(t);
    let mut stack = div().w_full().flex().flex_col().gap_1p5();

    for (seg_ix, segment) in segments.into_iter().enumerate() {
        let seg_id = SharedString::from(format!("{base_id}-seg-{seg_ix}"));
        match segment {
            MarkdownSegment::Prose(chunk) => {
                let view = build_document_view(
                    seg_id,
                    chunk,
                    doc_style.clone(),
                    highlighter.clone(),
                    extensions,
                    session.clone(),
                    ty,
                );
                stack = stack.child(div().w_full().max_w(prose_max).mx_auto().px_4().child(view));
            }
            MarkdownSegment::Table { markdown, cols } => {
                let wide_scroll = cols >= WIDE_TABLE_SCROLL_COLS;
                let style = if wide_scroll {
                    wide_table_style.clone()
                } else {
                    doc_style.clone()
                };
                let fit_inner = if cols <= 3 {
                    let inner_limit = qaqh_gpui_kit::settings::MESSAGE_MAX_WIDTH_PX - 32.0;
                    Some(estimate_table_natural_width(&markdown, cols).clamp(320.0, inner_limit))
                } else {
                    None
                };
                let copy_md: SharedString = markdown.clone().into();
                let view = build_document_view(
                    seg_id,
                    markdown,
                    style,
                    highlighter.clone(),
                    extensions,
                    session.clone(),
                    ty,
                )
                .table_actions(move |_, _, cx| {
                    let t = *cx.colors();
                    let copy_md = copy_md.clone();
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .children(wide_scroll.then(|| {
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(t.faint)
                                .child(
                                    Icon::new(IconName::MoveHorizontal)
                                        .with_size(Size::XSmall)
                                        .text_color(t.faint),
                                )
                                .child("可横向滑动查看")
                        }))
                        .child(
                            div()
                                .id("copy-md-table")
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(24.))
                                .rounded(px(RADIUS_CHIP))
                                .cursor_pointer()
                                .text_color(t.dim)
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("复制 Markdown 表格")
                                        .build(window, cx)
                                })
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_md.to_string(),
                                    ));
                                })
                                .child(
                                    Icon::new(IconName::Copy)
                                        .with_size(Size::XSmall)
                                        .text_color(t.dim),
                                ),
                        )
                });
                let table_block = if let Some(fit_inner) = fit_inner {
                    // `prose_max` 容器左右各有 `px_4` (合计 32px),内层按内容自适应并在超长时封顶贴齐正文右缘。
                    div()
                        .w_full()
                        .max_w(prose_max)
                        .mx_auto()
                        .px_4()
                        .child(div().w_full().max_w(px(fit_inner)).child(view))
                } else {
                    div().w_full().max_w(table_max).mx_auto().px_4().child(view)
                };
                stack = stack.child(table_block);
            }
        }
    }

    stack.into_any_element()
}

/// 渲染紧凑行内 Markdown(带链接图标与点击跳转,可选中),用于 Ask 题干。
pub fn render_inline(
    id: impl Into<SharedString>,
    source: &str,
    extensions: &Arc<MarkdownExtensions>,
    session: WeakEntity<SessionStore>,
    t: &Palette,
) -> AnyElement {
    let (_, _, compact_style, highlighter) = cached_theme_assets(t);
    let view = TextView::markdown(id.into(), source.to_owned())
        .style(compact_style)
        .markdown_extensions((**extensions).clone())
        .shared_code_block_highlighter(highlighter)
        .selectable(true);

    attach_link_handler(view, session).into_any_element()
}

/// 渲染紧凑纯交互友好行内 Markdown(禁用选中/外链拦截,避免抢占选项行整行点击)。
pub fn render_inline_plain(
    id: impl Into<SharedString>,
    source: &str,
    t: &Palette,
) -> AnyElement {
    let (_, _, compact_style, highlighter) = cached_theme_assets(t);
    TextView::markdown(id.into(), source.to_owned())
        .style(compact_style)
        .shared_code_block_highlighter(highlighter)
        .selectable(false)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_icons_classify_web_and_source_targets() {
        let ((gh_icon, _), gh_trail) =
            link_icons(&classify_href("https://github.com/QAQTam/qaqh-pets").expect("github"));
        assert_eq!(gh_icon, IconName::Github);
        assert_eq!(gh_trail.map(|(i, _)| i), Some(IconName::ExternalLink));

        let ((web_icon, _), _) =
            link_icons(&classify_href("https://example.com/docs").expect("web"));
        assert_eq!(web_icon, IconName::Globe);

        let ((lic_icon, _), _) =
            link_icons(&classify_href("LICENSE-MIT").expect("license"));
        assert_eq!(lic_icon, IconName::Copyright);

        let ((git_icon, _), _) =
            link_icons(&classify_href(".gitignore").expect("gitignore"));
        assert_eq!(git_icon, IconName::GitBranch);

        let ((cfg_icon, _), _) =
            link_icons(&classify_href("Cargo.toml").expect("cargo"));
        assert_eq!(cfg_icon, IconName::FileCog);

        let ((rs_icon, _), _) =
            link_icons(&classify_href("crates/qaqh-gpui-shell/src/sidebar.rs#L10-L20").expect("rs"));
        assert_eq!(rs_icon, IconName::FileCode);
    }

    #[test]
    fn widget_line_parses_valid_and_rejects_partial() {
        let marker =
            "\u{E200} visualize \u{E202}{\"path\":\"D:/p/score.html\",\"title\":\"Score\"}\u{E201}";
        assert_eq!(
            widget_line(marker),
            Some(Ok((
                "D:/p/score.html".to_owned(),
                Some("Score".to_owned())
            )))
        );
        assert_eq!(widget_line("\u{E200} visualize \u{E202}{oops}\u{E201}"), Some(Err(())));
        assert_eq!(widget_line("\u{E200} visualize \u{E202}{\"path\":"), None);
    }

    #[test]
    fn split_markdown_tables_extracts_top_level_tables_and_skips_code_fences() {
        let doc = "\
## 标题

前文说明

| # | 材料 | 作用 | 从哪里来 | 备注 |
|---|---|---|---|---|
| 1 | 镜像 | 货 | dump | ok |

```markdown
| fake | table |
|---|---|
| 1 | 2 |
```

后文总结";
        let segs = split_markdown_tables(doc);
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[0], MarkdownSegment::Prose(p) if p.contains("前文说明")));
        assert!(matches!(
            &segs[1],
            MarkdownSegment::Table { markdown, cols: 5 } if markdown.contains("镜像")
        ));
        assert!(matches!(
            &segs[2],
            MarkdownSegment::Prose(p) if p.contains("fake") && p.contains("后文总结")
        ));
    }
}
