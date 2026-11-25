//! Markdown 渲染(M6 · Plan B):基于 `gpui_kit::base::text::TextView` 原生排版引擎。
//! - 原生跨段文本拖选、双击选词、三击选段与 `Ctrl+C` 复制;
//! - 行内 `` `code` `` 等宽字体 + `0.875x` 缩字号 + `code_fg` 微色差 + 4px 圆角描边胶囊;
//! - H1/H2 标题底部分割线;
//! - 通过 `MarkdownPlugin` 保留文件/网址 Lucide 图标联动(`code_panel` / 浏览器)与 `widget` 卡片,
//!   以及 `highlight.rs` AST 语法高亮 + 右上角复制操作条。

use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::text::{
    CodeBlock, InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode,
    MarkdownParseContext, MarkdownPlugin, TextView, TextViewStyle, markdown_ast,
};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, FontWeight, HighlightStyle, Hsla, Overflow, SharedString,
    StyleRefinement, WeakEntity, Window, div, px, relative, rems,
};
use qaqh_gpui_kit::kit::{
    RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, shadow_card, soft,
};
use qaqh_gpui_kit::settings::Typography;
use qaqh_gpui_kit::theme::{ActiveTheme, Palette};

use crate::store::SessionStore;

const LINK_ICON_NODE: &str = "qaqh-link-icon";
const WIDGET_NODE: &str = "qaqh-widget";
const MAX_PILL_CHARS: usize = 72;

/// 正文阅读轨外框附加宽度(含左右 `px_4` 内边距),与原时间线内容宽保持像素一致。
pub const PROSE_RAIL_PAD_PX: f32 = 48.0;
/// 顶层 Markdown 表格对称破栏宽出量(左右各 +120px,4px 网格),充分利用宽屏舞台留白。
pub const TABLE_BREAKOUT_EXTRA_PX: f32 = 240.0;
/// 超过该列数才启用横向滚动兜底;常规 2~6 列表格默认积极折行、一屏完整展示。
const WIDE_TABLE_SCROLL_COLS: usize = 7;

#[derive(Clone, PartialEq, Eq)]
enum LinkTarget {
    Web(String),
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
    if is_http_url(url) {
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

fn link_icons(target: &LinkTarget) -> ((IconName, IconRole), Option<(IconName, IconRole)>) {
    match target {
        LinkTarget::Web(url) => {
            let host = web_host(url).to_ascii_lowercase();
            let lead = if host == "github.com" || host.ends_with(".github.com") {
                (IconName::Github, IconRole::Web)
            } else {
                (IconName::Globe, IconRole::Web)
            };
            (lead, Some((IconName::ExternalLink, IconRole::Dim)))
        }
        LinkTarget::Source(loc) => {
            let path = loc.path();
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
            (lead, None)
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
                let mut label = String::new();
                collect_plain_text(&link.children, &mut label);
                let label = label.trim().to_owned();
                if label.is_empty() || label.chars().count() > MAX_PILL_CHARS {
                    return None;
                }
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
        let ((lead_icon, lead_role), trail) = link_icons(&data.target);
        let id_offset = node.source_range().map_or(0, |r| r.start);
        let is_web = matches!(data.target, LinkTarget::Web(_));

        let label_view = if data.is_code {
            div()
                .px(px(4.))
                .py(px(1.))
                .rounded(px(4.))
                .bg(t.code_bg)
                .border_1()
                .border_color(t.border_subtle)
                .font_family("monospace")
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
                LinkTarget::Source(location) => {
                    let location = location.clone();
                    let _ = session.update(cx, |store, cx| {
                        crate::code_panel::open_source(store, location, cx);
                    });
                }
            })
            .child(
                Icon::new(lead_icon)
                    .with_size(Size::XSmall)
                    .text_color(lead_role.color(&t)),
            )
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
                        .font_family("monospace")
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

/// 主对话正文 Markdown 渲染(支持顶层表格对称破栏宽出):
/// - 普通正文守住 `content_width + PROSE_RAIL_PAD_PX` 最佳阅读行宽;
/// - 顶层表格对称向外借用 `TABLE_BREAKOUT_EXTRA_PX` 舞台留白,窗口窄时自动贴边收缩;
/// - 2~6 列表格默认积极折行一屏完整可见,7+ 列超宽表启用横向滚动兜底与图标操作条。
pub fn render_document_breakout(
    id: impl Into<SharedString>,
    source: &str,
    extensions: &Arc<MarkdownExtensions>,
    session: WeakEntity<SessionStore>,
    t: &Palette,
    ty: &Typography,
) -> AnyElement {
    let base_id = id.into();
    let prose_max = px(ty.content_width + PROSE_RAIL_PAD_PX);
    let table_max = px(ty.content_width + PROSE_RAIL_PAD_PX + TABLE_BREAKOUT_EXTRA_PX);
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
                                .child("Shift+滚轮或触控板横滑")
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
                stack = stack.child(div().w_full().max_w(table_max).mx_auto().px_4().child(view));
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
