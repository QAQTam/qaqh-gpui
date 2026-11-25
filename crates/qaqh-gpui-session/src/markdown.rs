//! Markdown 渲染(M4):pulldown-cmark 事件流 → 顶层块 → gpui 元素。
//!
//! 设计借鉴 zed markdown crate 的管线(仅借鉴,其 crate 为 GPL 不搬代码):
//! 事件流 + 顶层块(root-block)切分、块按 source offset 命名 ElementId、
//! 流式时旧结果保持可见。自写轻量实现;每帧重渲染的成本由两级缓存吸收:
//! - 本模块的解析缓存(按 section key + 源文本哈希,未变不重解析);
//! - gpui 窗口级 LineLayoutCache(未变文本行免 re-shape)。

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::AnyElement;
use gpui_kit::App;
use gpui_kit::Bounds;
use gpui_kit::ClipboardItem;
use gpui_kit::Element;
use gpui_kit::ElementId;
use gpui_kit::FontStyle;
use gpui_kit::FontWeight;
use gpui_kit::GlobalElementId;
use gpui_kit::HighlightStyle;
use gpui_kit::Hsla;
use gpui_kit::InspectorElementId;
use gpui_kit::InteractiveElement;
use gpui_kit::LayoutId;
use gpui_kit::Pixels;
use gpui_kit::SharedString;
use gpui_kit::StrikethroughStyle;
use gpui_kit::Styled;
use gpui_kit::StyledText;
use gpui_kit::TextLayout;
use gpui_kit::TransformationMatrix;
use gpui_kit::UnderlineStyle;
use gpui_kit::Window;
use gpui_kit::div;
use gpui_kit::point;
use gpui_kit::px;
use gpui_kit::relative;
use gpui_kit::size;
use pulldown_cmark::Alignment;
use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;

use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, shadow_card, soft};
use qaqh_gpui_kit::settings::Typography;
use qaqh_gpui_kit::theme::Palette;

type TableCell = (String, Vec<MdRun>);
type CachedBlocks = (u64, Arc<Vec<MdBlock>>);

/// inline 样式标注:块文本内 byte 范围 + 类别。
#[derive(Clone, Debug)]
pub struct MdRun {
    pub range: Range<usize>,
    pub kind: MdRunKind,
}

#[derive(Clone, Debug)]
pub enum MdRunKind {
    Bold,
    Italic,
    Strike,
    Code,
    Link(String),
}

#[derive(Debug)]
pub enum MdBlockKind {
    Paragraph { text: String, runs: Vec<MdRun> },
    Heading { level: u32, text: String, runs: Vec<MdRun> },
    Quote { text: String, runs: Vec<MdRun> },
    /// 列表项:(编号 None = 无序, 文本, runs)。
    List { items: Vec<(Option<u64>, String, Vec<MdRun>)> },
    Table { columns: Vec<Alignment>, rows: Vec<Vec<TableCell>> },
    Code { lang: Option<String>, code: String },
    Rule,
    /// widget-directive marker 卡:模型输出的 block 行,桌面端以系统浏览器打开
    /// fragment(同机部署 path 即本地文件;空 path = JSON 解析失败的一行占位)。
    Widget { path: String, title: Option<String> },
}

#[derive(Debug)]
pub struct MdBlock {
    /// 块在源文本中的起始偏移,用作流式稳定的 ElementId。
    pub offset: usize,
    pub kind: MdBlockKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_keeps_header_columns_and_inline_styles() {
        let blocks = parse_blocks(
            "| 类别\t | 命令 |\n| :--- | ---: |\n| **查找** | `rg` |\n| 编辑 |\n",
        );
        let MdBlockKind::Table { columns, rows } = &blocks[0].kind else {
            panic!("expected a table");
        };
        assert_eq!(columns, &[Alignment::Left, Alignment::Right]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][0].0, "类别");
        assert_eq!(rows[0][1].0, "命令");
        assert_eq!(rows[1][0].0, "查找");
        assert!(matches!(rows[1][0].1[0].kind, MdRunKind::Bold));
        assert!(matches!(rows[1][1].1[0].kind, MdRunKind::Code));
        assert_eq!(rows[2][1].0, "");
    }

    #[test]
    fn fenced_tsv_keeps_empty_columns() {
        let blocks = parse_blocks("```tsv\n类别\t命令\t备注\n查找\t\trg\n编辑\twrite\n```");
        let MdBlockKind::Table { columns, rows } = &blocks[0].kind else {
            panic!("expected a table");
        };
        assert_eq!(columns.len(), 3);
        assert_eq!(rows[1][1].0, "");
        assert_eq!(rows[1][2].0, "rg");
    }

    #[test]
    fn widget_marker_lines_become_widget_blocks() {
        let marker = "\u{E200} visualize \u{E202}{\"path\":\"D:/p/.qaqh/widgets/s1/score.html\",\"title\":\"Score\"}\u{E201}";
        let blocks = parse_blocks(&format!("正文一句\n{marker}\n{marker}"));
        // 段落与 marker 混排(inline 语义)不抽,整段独占行才抽。
        assert!(matches!(blocks[0].kind, MdBlockKind::Paragraph { .. }));
        let blocks = parse_blocks(&format!("{marker}\n{marker}"));
        assert_eq!(blocks.len(), 2);
        assert!(blocks.iter().all(|block| matches!(
            block.kind,
            MdBlockKind::Widget { ref path, title: Some(_) } if path == "D:/p/.qaqh/widgets/s1/score.html"
        )));
        // JSON 失败降级占位(空 path);流式半截保持原样。
        let broken = "\u{E200} visualize \u{E202}{oops}\u{E201}";
        let blocks = parse_blocks(broken);
        assert!(matches!(
            blocks[0].kind,
            MdBlockKind::Widget { path: ref p, .. } if p.is_empty()
        ));
        let partial = "\u{E200} visualize \u{E202}{\"path\":";
        let blocks = parse_blocks(partial);
        assert!(matches!(blocks[0].kind, MdBlockKind::Paragraph { .. }));
    }

    #[test]
    fn autolinks_bare_urls_and_decorates_links_with_icons() {
        let blocks = parse_blocks(
            "仓库：https://github.com/QAQTam/qaqh-pets（PUBLIC），官网 https://example.com/docs。许可：[MIT](LICENSE-MIT)，忽略：[忽略](.gitignore)，清单：[Cargo.toml](Cargo.toml)，源码：[sidebar](crates/qaqh-gpui-shell/src/sidebar.rs#L10-L20)，以及 [**加粗**链接](README.md)。",
        );
        let MdBlockKind::Paragraph { text, runs } = &blocks[0].kind else {
            panic!("expected paragraph");
        };
        let urls: Vec<&str> = runs
            .iter()
            .filter_map(|run| match &run.kind {
                MdRunKind::Link(url) => Some(url.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://github.com/QAQTam/qaqh-pets",
                "https://example.com/docs",
                "LICENSE-MIT",
                ".gitignore",
                "Cargo.toml",
                "crates/qaqh-gpui-shell/src/sidebar.rs#L10-L20",
                "README.md",
                "README.md",
            ]
        );
        let deco = plan_decorations(runs, text).expect("expected decorated plan");
        // Github (lead + trail), Globe (lead + trail), Copyright, GitBranch, FileCog, FileCode, merged FileText
        let icon_names: Vec<IconName> = deco.icons.iter().map(|icon| icon.icon).collect();
        assert_eq!(
            icon_names,
            vec![
                IconName::Github,
                IconName::ExternalLink,
                IconName::Globe,
                IconName::ExternalLink,
                IconName::Copyright,
                IconName::GitBranch,
                IconName::FileCog,
                IconName::FileCode,
                IconName::FileText,
            ]
        );
        for icon in &deco.icons {
            assert!(deco.out.is_char_boundary(icon.byte_index));
        }
        for range in &deco.click_ranges {
            assert!(deco.out.is_char_boundary(range.start));
            assert!(deco.out.is_char_boundary(range.end));
            assert!(range.start < range.end);
        }
        assert!(deco.out.contains("**加粗**") || deco.out.contains("加粗链接"));
    }
}

/// 块 id:section key 区分不同消息,offset 区分块内位置(状态跨帧稳定)。
fn block_id(section: usize, offset: usize) -> ElementId {
    ("mdb", section * 1_000_003 + offset).into()
}

/// 解析缓存:section key → (源哈希, 解析结果)。
/// 流式追加只改尾部 section 的哈希,其余块跨帧复用解析结果。
#[derive(Default)]
pub struct ParseCache {
    entries: RefCell<HashMap<usize, CachedBlocks>>,
}

impl ParseCache {
    pub fn get(&self, section: usize, source: &str) -> Arc<Vec<MdBlock>> {
        let hash = qaqh_gpui_render::highlight::fnv1a(source);
        let mut entries = self.entries.borrow_mut();
        if let Some((previous, blocks)) = entries.get(&section)
            && *previous == hash
        {
            return blocks.clone();
        }
        let parsed = Arc::new(parse_blocks(source));
        if entries.len() >= 512 { entries.clear(); }
        entries.insert(section, (hash, parsed.clone()));
        parsed
    }

    /// 会话重置(快照重灌)时清理。
    pub fn clear(&mut self) {
        self.entries.get_mut().clear();
    }
}

/// 当前顶层块的收集状态。
enum Blk {
    Paragraph,
    Heading(u32),
    Quote,
    List { ordered: bool },
    Table(Vec<Alignment>),
    Code(Option<String>),
}

fn lang_of(kind: &CodeBlockKind) -> Option<String> {
    match kind {
        CodeBlockKind::Fenced(info) => info.trim().split_whitespace().next().map(str::to_string),
        CodeBlockKind::Indented => None,
    }
}

fn tsv_table(source: &str) -> MdBlockKind {
    let mut rows: Vec<Vec<TableCell>> = source
        .lines()
        .map(|line| line.split('\t').map(|cell| (cell.to_owned(), Vec::new())).collect())
        .collect();
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        row.resize_with(width, TableCell::default);
    }
    MdBlockKind::Table { columns: vec![Alignment::None; width], rows }
}

fn append_styled_chunk(
    sink: &mut String,
    runs: &mut Vec<MdRun>,
    style: Option<&MdRunKind>,
    chunk: &str,
) {
    if chunk.is_empty() {
        return;
    }
    let start = sink.len();
    sink.push_str(chunk);
    if let Some(kind) = style {
        runs.push(MdRun {
            range: start..sink.len(),
            kind: kind.clone(),
        });
    }
}

fn find_url_start(haystack: &str) -> Option<usize> {
    let mut offset = 0usize;
    while let Some(tail) = haystack.get(offset..) {
        let idx_https = tail.find("https://");
        let idx_http = tail.find("http://");
        let rel = match (idx_https, idx_http) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return None,
        };
        let abs = offset + rel;
        let boundary_ok = abs == 0
            || haystack
                .get(..abs)
                .and_then(|before| before.chars().next_back())
                .is_some_and(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
        if boundary_ok {
            return Some(abs);
        }
        offset = abs + 4;
    }
    None
}

fn trim_trailing_url_punct(mut url: &str) -> &str {
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if trimmed.len() != url.len() {
            url = trimmed;
            continue;
        }
        match url.chars().next_back() {
            Some(')') if url.matches(')').count() > url.matches('(').count() => {
                url = url.strip_suffix(')').unwrap_or(url);
            }
            Some(']') if url.matches(']').count() > url.matches('[').count() => {
                url = url.strip_suffix(']').unwrap_or(url);
            }
            Some('}') if url.matches('}').count() > url.matches('{').count() => {
                url = url.strip_suffix('}').unwrap_or(url);
            }
            _ => break,
        }
    }
    url
}

fn is_valid_http_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host_port.split(':').next().unwrap_or("");
    !host_port.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && (host.contains('.')
            || host_port.starts_with('[')
            || host.eq_ignore_ascii_case("localhost"))
}

/// 向文本槽追加 chunk,栈顶 inline 样式(若有)记 run;非链接内文本自动识别裸 http(s) URL。
fn push_text(sink: &mut String, runs: &mut Vec<MdRun>, styles: &[MdRunKind], chunk: &str) {
    if styles.iter().any(|kind| matches!(kind, MdRunKind::Link(_))) {
        append_styled_chunk(sink, runs, inline_style(styles), chunk);
        return;
    }
    let normal_style = inline_style(styles);
    let mut cursor = 0usize;
    while let Some(tail) = chunk.get(cursor..) {
        let Some(rel_start) = find_url_start(tail) else {
            append_styled_chunk(sink, runs, normal_style, tail);
            return;
        };
        let abs_start = cursor + rel_start;
        if let Some(prefix) = chunk.get(cursor..abs_start) {
            append_styled_chunk(sink, runs, normal_style, prefix);
        }
        let Some(url_tail) = chunk.get(abs_start..) else {
            return;
        };
        let raw_len = url_tail
            .char_indices()
            .find(|&(_, ch)| {
                !ch.is_ascii()
                    || ch.is_ascii_whitespace()
                    || matches!(ch, '<' | '>' | '"' | '\'' | '`')
            })
            .map(|(idx, _)| idx)
            .unwrap_or(url_tail.len());
        let candidate = url_tail.get(..raw_len).unwrap_or("");
        let trimmed = trim_trailing_url_punct(candidate);
        if is_valid_http_url(trimmed) {
            append_styled_chunk(sink, runs, Some(&MdRunKind::Link(trimmed.to_owned())), trimmed);
            cursor = abs_start + trimmed.len();
        } else {
            let advance = (abs_start + 4).min(chunk.len());
            if let Some(plain) = chunk.get(abs_start..advance) {
                append_styled_chunk(sink, runs, normal_style, plain);
            }
            cursor = advance;
        }
    }
}

fn inline_style(styles: &[MdRunKind]) -> Option<&MdRunKind> {
    styles.iter().rev().find(|kind| matches!(kind, MdRunKind::Link(_)))
        .or_else(|| styles.last())
}

/// 解析 markdown 源为顶层块列表。
pub fn parse_blocks(source: &str) -> Vec<MdBlock> {
    let mut opts = Options::empty();
    opts.insert(
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_GFM,
    );

    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut depth: usize = 0;
    let mut block_offset = 0usize;
    let mut blk: Option<Blk> = None;
    let mut text = String::new();
    let mut runs: Vec<MdRun> = Vec::new();
    let mut styles: Vec<MdRunKind> = Vec::new();
    let mut items: Vec<(Option<u64>, String, Vec<MdRun>)> = Vec::new();
    let mut list_number = 0u64;
    let mut rows = Vec::new();
    let mut row = Vec::new();

    for (event, range) in Parser::new_ext(source, opts).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                depth += 1;
                if depth == 1 {
                    block_offset = range.start;
                    text.clear();
                    runs.clear();
                    styles.clear();
                    items.clear();
                    rows.clear();
                    row.clear();
                    blk = Some(match tag {
                        Tag::Heading { level, .. } => Blk::Heading(level as u32),
                        Tag::BlockQuote(_) => Blk::Quote,
                        Tag::List(marker) => {
                            list_number = marker.unwrap_or(0);
                            Blk::List {
                                ordered: marker.is_some(),
                            }
                        }
                        Tag::Table(columns) => Blk::Table(columns),
                        Tag::CodeBlock(code_kind) => Blk::Code(lang_of(&code_kind)),
                        _ => Blk::Paragraph,
                    });
                    continue;
                }
                match (&blk, tag) {
                    (Some(Blk::List { ordered }), Tag::Item) => {
                        let number = if *ordered {
                            let n = list_number;
                            list_number += 1;
                            Some(n)
                        } else {
                            None
                        };
                        items.push((number, String::new(), Vec::new()));
                    }
                    (Some(_), Tag::Strong) => styles.push(MdRunKind::Bold),
                    (Some(_), Tag::Emphasis) => styles.push(MdRunKind::Italic),
                    (Some(_), Tag::Strikethrough) => styles.push(MdRunKind::Strike),
                    (Some(_), Tag::Link { dest_url, .. }) => {
                        styles.push(MdRunKind::Link(dest_url.into_string()));
                    }
                    _ => {}
                }
            }
            Event::End(tag_end) => {
                depth = depth.saturating_sub(1);
                if depth > 0 {
                    // 块内结束:quote/list 内段落分隔 + inline 样式出栈。
                    match (&blk, tag_end) {
                        (Some(Blk::Table(_)), TagEnd::TableCell) => {
                            row.push((std::mem::take(&mut text), std::mem::take(&mut runs)));
                        }
                        (Some(Blk::Table(_)), TagEnd::TableHead | TagEnd::TableRow) => {
                            rows.push(std::mem::take(&mut row));
                        }
                        (Some(Blk::Quote), TagEnd::Paragraph) => {
                            if !text.is_empty() {
                                text.push_str("\n\n");
                            }
                        }
                        (Some(Blk::List { .. }), TagEnd::Paragraph) => {
                            if let Some((_, item, _)) = items.last_mut()
                                && !item.is_empty()
                            {
                                item.push('\n');
                            }
                        }
                        (Some(_), TagEnd::Strong) => {
                            styles.pop();
                        }
                        (Some(_), TagEnd::Emphasis) => {
                            styles.pop();
                        }
                        (Some(_), TagEnd::Strikethrough) => {
                            styles.pop();
                        }
                        (Some(_), TagEnd::Link) => {
                            styles.pop();
                        }
                        _ => {}
                    }
                    continue;
                }
                let Some(current) = blk.take() else {
                    continue;
                };
                let kind = match current {
                    Blk::Heading(level) => MdBlockKind::Heading {
                        level,
                        text: std::mem::take(&mut text),
                        runs: std::mem::take(&mut runs),
                    },
                    Blk::Quote => MdBlockKind::Quote {
                        text: std::mem::take(&mut text),
                        runs: std::mem::take(&mut runs),
                    },
                    Blk::List { .. } => MdBlockKind::List {
                        items: std::mem::take(&mut items),
                    },
                    Blk::Table(columns) => MdBlockKind::Table {
                        columns,
                        rows: std::mem::take(&mut rows),
                    },
                    Blk::Code(Some(lang)) if lang.eq_ignore_ascii_case("tsv") && text.contains('\t') => {
                        tsv_table(&text)
                    }
                    Blk::Code(lang) => MdBlockKind::Code {
                        lang,
                        code: std::mem::take(&mut text),
                    },
                    Blk::Paragraph => MdBlockKind::Paragraph {
                        text: std::mem::take(&mut text),
                        runs: std::mem::take(&mut runs),
                    },
                };
                blocks.push(MdBlock {
                    offset: block_offset,
                    kind,
                });
            }
            Event::Text(chunk) => {
                if depth == 0 {
                    continue;
                }
                match &blk {
                    Some(Blk::Code(_)) => text.push_str(&chunk),
                    Some(Blk::List { .. }) => {
                        if let Some((_, item, item_runs)) = items.last_mut() {
                            push_text(item, item_runs, &styles, &chunk);
                        }
                    }
                    _ => push_text(&mut text, &mut runs, &styles, &chunk),
                }
            }
            Event::Code(chunk) => {
                if depth == 0 {
                    continue;
                }
                let kind = inline_style(&styles)
                    .filter(|kind| matches!(kind, MdRunKind::Link(_)))
                    .cloned().unwrap_or(MdRunKind::Code);
                match &blk {
                    Some(Blk::List { .. }) => {
                        if let Some((_, item, item_runs)) = items.last_mut() {
                            let start = item.len();
                            item.push_str(&chunk);
                            item_runs.push(MdRun {
                                range: start..item.len(),
                                kind,
                            });
                        }
                    }
                    _ => {
                        let start = text.len();
                        text.push_str(&chunk);
                        runs.push(MdRun {
                            range: start..text.len(),
                            kind,
                        });
                    }
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                match &blk {
                    Some(Blk::Code(_)) => {}
                    Some(Blk::List { .. }) => {
                        if let Some((_, item, _)) = items.last_mut() {
                            item.push('\n');
                        }
                    }
                    _ => text.push('\n'),
                }
            }
            Event::TaskListMarker(checked) => {
                if let Some((_, item, _)) = items.last_mut() {
                    item.push_str(if checked { "☑ " } else { "☐ " });
                }
            }
            Event::Rule => {
                if depth == 0 {
                    blocks.push(MdBlock {
                        offset: range.start,
                        kind: MdBlockKind::Rule,
                    });
                }
            }
            Event::InlineHtml(chunk) | Event::Html(chunk) => {
                if depth > 0 && !matches!(blk, Some(Blk::Code(_))) {
                    push_text(&mut text, &mut runs, &styles, &chunk);
                }
            }
            // 数学/脚注未启用(Options 未开 ENABLE_MATH/ENABLE_FOOTNOTES),事件不可达。
            _ => {}
        }
    }

    extract_widget_markers(blocks)
}

/// widget marker 判定:`U+E200 visualize U+E202 {json} U+E201` 独占行。
/// Some(Ok((path,title))) = 有效;Some(Err(())) = 完整但 JSON 失败(占位);
/// None = 非 marker 行(含流式半截)。
fn widget_line(line: &str) -> Option<Result<(String, Option<String>), ()>> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('\u{E200}')?;
    let rest = rest.strip_prefix(" visualize ")?;
    let rest = rest.strip_prefix('\u{E202}')?;
    let payload = rest.strip_suffix('\u{E201}')?;
    match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => Some(Ok((
            value.get("path").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            value.get("title").and_then(|v| v.as_str()).map(str::to_owned),
        ))),
        Err(_) => Some(Err(())),
    }
}

/// 段落级 marker 抽取:整段(含软换行连接的多行)全部为 marker 行时,
/// 逐行替换成 Widget 块;否则原样保留(inline marker 按原文显示,不猜)。
fn extract_widget_markers(blocks: Vec<MdBlock>) -> Vec<MdBlock> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        let MdBlock { offset, kind: MdBlockKind::Paragraph { text, runs } } = block else {
            out.push(block);
            continue;
        };
        let lines: Vec<&str> = text.split('\n').collect();
        if lines.is_empty() || !lines.iter().all(|line| widget_line(line).is_some()) {
            out.push(MdBlock {
                offset,
                kind: MdBlockKind::Paragraph { text, runs },
            });
            continue;
        }
        let mut line_offset = offset;
        for line in lines {
            match widget_line(line) {
                Some(Ok((path, title))) => out.push(MdBlock {
                    offset: line_offset,
                    kind: MdBlockKind::Widget { path, title },
                }),
                Some(Err(())) => out.push(MdBlock {
                    offset: line_offset,
                    kind: MdBlockKind::Widget { path: String::new(), title: None },
                }),
                None => {}
            }
            line_offset += line.len() + 1;
        }
    }
    out
}

fn run_style(kind: &MdRunKind, t: &Palette) -> HighlightStyle {
    match kind {
        MdRunKind::Bold => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        },
        MdRunKind::Italic => HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..Default::default()
        },
        MdRunKind::Strike => HighlightStyle {
            strikethrough: Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            ..Default::default()
        },
        MdRunKind::Code => HighlightStyle {
            color: Some(t.text),
            background_color: Some(t.code_bg),
            ..Default::default()
        },
        MdRunKind::Link(_) => HighlightStyle {
            color: Some(t.accent),
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            }),
            ..Default::default()
        },
    }
}

/// 块文本渲染:延迟高亮(以 paint 时环境样式为基,runs 覆盖)。
fn styled(runs: &[MdRun], text: &str, t: &Palette) -> StyledText {
    StyledText::new(text).with_highlights(
        runs.iter()
            .map(|run| (run.range.clone(), run_style(&run.kind, t))),
    )
}

#[derive(Clone)]
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct IconPlan {
    byte_index: usize,
    icon: IconName,
    role: IconRole,
    scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InlineIcon {
    byte_index: usize,
    icon: IconName,
    color: Hsla,
    scale: f32,
}

enum HighlightSpec {
    WebLink,
    SourceLink { is_code: bool },
    Run(MdRunKind),
}

struct DecoratedPlan {
    out: String,
    specs: Vec<(Range<usize>, HighlightSpec)>,
    click_ranges: Vec<Range<usize>>,
    targets: Vec<LinkTarget>,
    icons: Vec<IconPlan>,
}

fn is_http_url(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
        || url
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
}

fn classify_link(run: &MdRun, text: &str) -> Option<LinkTarget> {
    match &run.kind {
        MdRunKind::Link(url) => {
            if is_http_url(url) {
                Some(LinkTarget::Web(url.clone()))
            } else {
                crate::code_panel::Location::parse(url, false).map(LinkTarget::Source)
            }
        }
        MdRunKind::Code => text
            .get(run.range.clone())
            .and_then(|snippet| crate::code_panel::Location::parse(snippet, true))
            .map(LinkTarget::Source),
        _ => None,
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

fn merge_adjacent_link_runs(runs: &[MdRun]) -> Vec<MdRun> {
    let mut merged: Vec<MdRun> = Vec::with_capacity(runs.len());
    for run in runs {
        if let (Some(prev), MdRunKind::Link(url)) = (merged.last_mut(), &run.kind)
            && prev.range.end == run.range.start
            && matches!(&prev.kind, MdRunKind::Link(prev_url) if prev_url == url)
        {
            prev.range.end = run.range.end;
            continue;
        }
        merged.push(run.clone());
    }
    merged
}

fn plan_decorations(runs: &[MdRun], text: &str) -> Option<DecoratedPlan> {
    let merged = merge_adjacent_link_runs(runs);
    if !merged.iter().any(|run| classify_link(run, text).is_some()) {
        return None;
    }

    let mut out = String::with_capacity(text.len() + merged.len() * 12);
    let mut specs = Vec::with_capacity(merged.len());
    let mut click_ranges = Vec::new();
    let mut targets = Vec::new();
    let mut icons = Vec::new();
    let mut cursor = 0usize;

    for run in merged {
        if run.range.start > cursor {
            if let Some(gap) = text.get(cursor..run.range.start) {
                out.push_str(gap);
            }
            cursor = run.range.start;
        }
        let Some(slice) = text.get(run.range.clone()) else {
            continue;
        };
        cursor = run.range.end;

        if let Some(target) = classify_link(&run, text) {
            let ((lead_icon, lead_role), trail) = link_icons(&target);
            let click_start = out.len();
            let lead_idx = out.len();
            // Em Space + Thin Space + Word Joiner keeps the leading icon glued to the link text.
            out.push_str("\u{2003}\u{2009}\u{2060}");
            icons.push(IconPlan {
                byte_index: lead_idx,
                icon: lead_icon,
                role: lead_role,
                scale: 0.90,
            });

            let text_start = out.len();
            out.push_str(slice);
            let text_end = out.len();
            let spec = match &target {
                LinkTarget::Web(_) => HighlightSpec::WebLink,
                LinkTarget::Source(_) => HighlightSpec::SourceLink {
                    is_code: matches!(run.kind, MdRunKind::Code),
                },
            };
            specs.push((text_start..text_end, spec));

            if let Some((trail_icon, trail_role)) = trail {
                out.push_str("\u{2060}\u{2009}");
                let trail_idx = out.len();
                out.push('\u{2003}');
                icons.push(IconPlan {
                    byte_index: trail_idx,
                    icon: trail_icon,
                    role: trail_role,
                    scale: 0.78,
                });
            }

            let click_end = out.len();
            click_ranges.push(click_start..click_end);
            targets.push(target);
        } else {
            let start = out.len();
            out.push_str(slice);
            specs.push((start..out.len(), HighlightSpec::Run(run.kind)));
        }
    }

    if let Some(tail) = text.get(cursor..) {
        out.push_str(tail);
    }

    Some(DecoratedPlan {
        out,
        specs,
        click_ranges,
        targets,
        icons,
    })
}

struct DecoratedText {
    inner: AnyElement,
    layout: TextLayout,
    icons: Vec<InlineIcon>,
}

impl Element for DecoratedText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.inner.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(window, cx);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = self.layout.line_height();
        for icon in &self.icons {
            let Some(pos) = self.layout.position_for_index(icon.byte_index) else {
                continue;
            };
            let icon_px = font_size * icon.scale;
            let x_pad = ((font_size - icon_px) * 0.5).max(px(0.));
            let y_pad = ((line_height - icon_px) * 0.5).max(px(0.));
            let icon_bounds = Bounds::new(
                point(pos.x + x_pad, pos.y + y_pad),
                size(icon_px, icon_px),
            );
            let _ = window.paint_svg(
                icon_bounds,
                icon.icon.path(),
                None,
                TransformationMatrix::unit(),
                icon.color,
                cx,
            );
        }
    }
}

/// 渲染一组块(每个块的 div id 按 source offset 命名,状态跨帧稳定)。
pub fn render_blocks(
    blocks: &[MdBlock],
    section: usize,
    t: &Palette,
    ty: &Typography,
    session: &gpui_kit::Entity<crate::store::SessionStore>,
) -> Vec<AnyElement> {
    blocks
        .iter()
        .map(|block| render_block(block, section, t, ty, session))
        .collect()
}

fn render_block(
    block: &MdBlock,
    section: usize,
    t: &Palette,
    ty: &Typography,
    session: &gpui_kit::Entity<crate::store::SessionStore>,
) -> AnyElement {
    let mut inline_index = 0usize;
    let mut styled = |runs: &[MdRun], text: &str, t: &Palette| {
        inline_index += 1;
        let Some(plan) = plan_decorations(runs, text) else {
            return styled(runs, text, t).into_any_element();
        };
        let highlights = plan.specs.into_iter().map(|(range, spec)| {
            let style = match spec {
                HighlightSpec::WebLink => HighlightStyle {
                    color: Some(t.syn_function),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(soft(t.syn_function, 0.45)),
                        wavy: false,
                    }),
                    ..Default::default()
                },
                HighlightSpec::SourceLink { is_code } => HighlightStyle {
                    color: Some(t.text),
                    background_color: is_code.then_some(t.code_bg),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(soft(t.dim, 0.45)),
                        wavy: false,
                    }),
                    ..Default::default()
                },
                HighlightSpec::Run(kind) => run_style(&kind, t),
            };
            (range, style)
        });
        let styled_text = StyledText::new(plan.out).with_highlights(highlights);
        let layout = styled_text.layout().clone();
        let icons: Vec<InlineIcon> = plan
            .icons
            .into_iter()
            .map(|icon| InlineIcon {
                byte_index: icon.byte_index,
                icon: icon.icon,
                color: icon.role.color(t),
                scale: icon.scale,
            })
            .collect();
        let targets = plan.targets;
        let session = session.downgrade();
        let link_id = (section.wrapping_mul(1_000_003))
            .wrapping_add(block.offset.wrapping_mul(31))
            .wrapping_add(inline_index);
        let interactive = gpui_kit::InteractiveText::new(("md-link", link_id), styled_text)
            .on_click(plan.click_ranges, move |index, _, cx| {
                let Some(target) = targets.get(index) else {
                    return;
                };
                match target {
                    LinkTarget::Web(url) => cx.open_url(url),
                    LinkTarget::Source(location) => {
                        let location = location.clone();
                        let _ = session.update(cx, |store, cx| {
                            crate::code_panel::open_source(store, location, cx);
                        });
                    }
                }
            })
            .into_any_element();
        DecoratedText {
            inner: interactive,
            layout,
            icons,
        }
        .into_any_element()
    };
    match &block.kind {
        MdBlockKind::Rule => div()
            .id(block_id(section, block.offset))
            .my_3()
            .h(px(1.))
            .w_full()
            .bg(t.border_subtle)
            .into_any_element(),
        MdBlockKind::Heading { level, text, runs } => {
            let mut heading = div()
                .id(block_id(section, block.offset))
                .text_color(t.text)
                .line_height(relative(ty.prose_line_height));
            if *level == 1 {
                heading = heading.mt_4().mb_1p5().text_xl().font_weight(FontWeight::BOLD);
            } else if *level == 2 {
                heading = heading.mt_3().mb_1().text_lg().font_weight(FontWeight::SEMIBOLD);
            } else if *level == 3 {
                heading = heading.mt_2p5().mb_1().text_base().font_weight(FontWeight::SEMIBOLD);
            } else {
                heading = heading.mt_2().mb_0p5().text_sm().font_weight(FontWeight::MEDIUM);
            }
            heading.child(styled(runs, text, t)).into_any_element()
        }
        MdBlockKind::Paragraph { text, runs } => div()
            .id(block_id(section, block.offset))
            .text_sm()
            .text_color(t.text)
            .line_height(relative(ty.prose_line_height))
            .child(styled(runs, text, t))
            .into_any_element(),
        MdBlockKind::Quote { text, runs } => div()
            .id(block_id(section, block.offset))
            .my_2()
            .flex()
            .gap_2p5()
            .px_3()
            .py_2()
            .rounded(px(RADIUS_CONTROL))
            .bg(t.surface_subtle)
            .border_1()
            .border_color(t.border_subtle)
            .child(
                div()
                    .w(px(3.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(soft(t.accent, 0.55)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(t.dim)
                    .line_height(relative(ty.prose_line_height))
                    .child(styled(runs, text, t)),
            )
            .into_any_element(),
        MdBlockKind::List { items } => div()
            .id(block_id(section, block.offset))
            .flex()
            .flex_col()
            .gap_1p5()
            .children(items.iter().map(|(number, text, runs)| {
                let marker = number
                    .map(|n| SharedString::from(format!("{n}.")))
                    .unwrap_or("•".into());
                div()
                    .flex()
                    .gap_2()
                    .text_sm()
                    .text_color(t.text)
                    .line_height(relative(ty.prose_line_height))
                    .child(
                        div()
                            .min_w(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.dim)
                            .child(marker),
                    )
                    .child(div().flex_1().min_w_0().child(styled(runs, text, t)))
            }))
            .into_any_element(),
        MdBlockKind::Table { columns, rows } => {
            // 行包进横向滚动容器(与代码块同法):宽表不裁切,可横向滚动。
            let mut body = div()
                .id(("mdtable-content", section * 1_000_003 + block.offset))
                .overflow_x_scroll()
                .flex()
                .flex_col();
            for (index, row) in rows.iter().enumerate() {
                let mut row_div = div().grid().grid_cols(columns.len() as u16).text_xs();
                if index == 0 {
                    row_div = row_div
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .bg(t.surface_subtle);
                } else {
                    row_div = row_div
                        .text_color(t.text)
                        .border_t_1()
                        .border_color(t.border_subtle);
                }
                body = body.child(row_div.children(row.iter().zip(columns).enumerate().map(
                    |(column, ((text, runs), alignment))| {
                        let cell = div().px_3().py_1p5();
                        let cell = match alignment {
                            Alignment::Center => cell.text_center(),
                            Alignment::Right => cell.text_right(),
                            _ => cell.text_left(),
                        };
                        let cell = if column > 0 {
                            cell.border_l_1().border_color(t.border_subtle)
                        } else {
                            cell
                        };
                        cell.child(styled(runs, text, t))
                    },
                )));
            }
            div()
                .id(block_id(section, block.offset))
                .my_2()
                .flex()
                .flex_col()
                .rounded(px(RADIUS_CARD))
                .border_1()
                .border_color(t.border_subtle)
                .bg(t.card)
                .overflow_hidden()
                .child(body)
                .into_any_element()
        }
        MdBlockKind::Code { lang, code } => {
            let mut card = div()
                .id(block_id(section, block.offset))
                .my_2()
                .flex()
                .flex_col()
                .rounded(px(RADIUS_CARD))
                .border_1()
                .border_color(t.border_subtle)
                .bg(t.code_bg)
                .overflow_hidden();
            let copy_code: SharedString = code.clone().into();
            let header = div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_1p5()
                .bg(t.surface_subtle)
                .border_b_1()
                .border_color(t.border_subtle)
                .child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(RADIUS_CHIP))
                        .bg(t.btn)
                        .font_family("monospace")
                        .text_xs()
                        .text_color(t.dim)
                        .child(lang.clone().unwrap_or_else(|| "text".into())),
                )
                .child(
                    div()
                        .id(("mdcopy", section * 1_000_003 + block.offset))
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
                        .on_click(move |_, _, cx: &mut App| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                copy_code.clone().into(),
                            ));
                        })
                        .child(Icon::new(IconName::Copy).with_size(Size::XSmall).text_color(t.dim))
                        .child("复制"),
                );
            card = card.child(header).child(
                div()
                    .id(("mdcode-content", section * 1_000_003 + block.offset))
                    .px_3p5()
                    .py_2p5()
                    .font_family("monospace")
                    .text_size(px(ty.code_font_size))
                    .line_height(relative(ty.prose_line_height))
                    .overflow_x_scroll()
                    .child(qaqh_gpui_render::highlight::code_text(code, lang.as_deref(), t)),
            );
            card.into_any_element()
        }
        MdBlockKind::Widget { path, title } => {
            let label = title.clone().unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "widget".into())
            });
            let mut card = div()
                .id(block_id(section, block.offset))
                .my_2()
                .flex()
                .flex_col()
                .gap_2()
                .px_3p5()
                .py_3()
                .rounded(px(RADIUS_CARD))
                .border_1()
                .border_color(t.border_subtle)
                .bg(t.card)
                .shadow(shadow_card(t))
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
                // 同机部署:path 即本地文件,系统浏览器是桌面端的合规宿主。
                let url = format!(
                    "file:///{}",
                    path.replace('\\', "/").trim_start_matches('/').to_owned()
                );
                card = card.child(
                    div()
                        .id(("widget-open", section * 1_000_003 + block.offset))
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
            card.into_any_element()
        }
    }
}
