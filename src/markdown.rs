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

use gpui::prelude::*;
use gpui::AnyElement;
use gpui::App;
use gpui::ClipboardItem;
use gpui::ElementId;
use gpui::FontStyle;
use gpui::FontWeight;
use gpui::HighlightStyle;
use gpui::InteractiveElement;
use gpui::SharedString;
use gpui::StrikethroughStyle;
use gpui::Styled;
use gpui::StyledText;
use gpui::UnderlineStyle;
use gpui::div;
use gpui::px;
use gpui::relative;
use pulldown_cmark::Alignment;
use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;

use crate::settings::Typography;
use crate::theme::Palette;

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
        assert_eq!(rows[2][2].0, "");
    }
}

/// 块 id:section key 区分不同消息,offset 区分块内位置(状态跨帧稳定)。
fn block_id(section: usize, offset: usize) -> ElementId {
    ("mdb", section * 1_000_003 + offset).into()
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 解析缓存:section key → (源哈希, 解析结果)。
/// 流式追加只改尾部 section 的哈希,其余块跨帧复用解析结果。
#[derive(Default)]
pub struct ParseCache {
    entries: RefCell<HashMap<usize, CachedBlocks>>,
}

impl ParseCache {
    pub fn get(&self, section: usize, source: &str) -> Arc<Vec<MdBlock>> {
        let hash = fnv1a(source);
        let mut entries = self.entries.borrow_mut();
        if let Some((previous, blocks)) = entries.get(&section)
            && *previous == hash
        {
            return blocks.clone();
        }
        let parsed = Arc::new(parse_blocks(source));
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

/// 向文本槽追加 chunk,栈顶 inline 样式(若有)记 run。
fn push_text(sink: &mut String, runs: &mut Vec<MdRun>, styles: &[MdRunKind], chunk: &str) {
    let start = sink.len();
    sink.push_str(chunk);
    if let Some(kind) = inline_style(styles) {
        runs.push(MdRun {
            range: start..sink.len(),
            kind: kind.clone(),
        });
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
            // 数学块(M4):无渲染器,按行内/块级文本展示。
            Event::InlineMath(chunk) | Event::DisplayMath(chunk) => {
                if depth > 0 && matches!(blk, Some(Blk::Table(_))) {
                    push_text(&mut text, &mut runs, &styles, &chunk);
                }
            }
            Event::FootnoteReference(_) => {}
        }
    }

    blocks
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

/// 渲染一组块(每个块的 div id 按 source offset 命名,状态跨帧稳定)。
pub fn render_blocks(
    blocks: &[MdBlock],
    section: usize,
    t: &Palette,
    ty: &Typography,
    session: &gpui::Entity<crate::store::SessionStore>,
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
    session: &gpui::Entity<crate::store::SessionStore>,
) -> AnyElement {
    let mut inline_index = 0usize;
    let mut styled = |runs: &[MdRun], text: &str, t: &Palette| {
        inline_index += 1;
        let mut ranges = Vec::new();
        let mut targets = Vec::new();
        for run in runs {
            let target = match &run.kind {
                MdRunKind::Link(target) => crate::code_panel::Location::parse(target, false),
                MdRunKind::Code => text.get(run.range.clone())
                    .and_then(|text| crate::code_panel::Location::parse(text, true)),
                _ => None,
            };
            if let Some(target) = target {
                ranges.push(run.range.clone());
                targets.push(target);
            }
        }
        if targets.is_empty() {
            return styled(runs, text, t).into_any_element();
        }
        let text = StyledText::new(text).with_highlights(
            runs.iter().map(|run| {
                let mut style = run_style(&run.kind, t);
                if ranges.contains(&run.range) {
                    style.color = Some(t.accent);
                    style.underline = Some(UnderlineStyle {
                        thickness: px(1.), color: None, wavy: false,
                    });
                }
                (run.range.clone(), style)
            }),
        );
        let session = session.downgrade();
        gpui::InteractiveText::new(("source-link", inline_index), text)
            .on_click(ranges, move |index, _, cx| {
                let _ = session.update(cx, |store, cx| {
                    crate::code_panel::open_source(store, targets[index].clone(), cx);
                });
            })
            .into_any_element()
    };
    match &block.kind {
        MdBlockKind::Rule => div()
            .id(block_id(section, block.offset))
            .my_2()
            .h(px(1.))
            .w_full()
            .bg(t.border)
            .into_any_element(),
        MdBlockKind::Heading { level, text, runs } => {
            let mut heading = div()
                .id(block_id(section, block.offset))
                .mt_2()
                .mb_1()
                .font_weight(FontWeight::BOLD)
                .line_height(relative(ty.prose_line_height));
            if *level == 1 {
                heading = heading.text_base();
            } else if *level == 2 {
                heading = heading.text_lg();
            } else {
                heading = heading.text_sm();
            }
            heading.child(styled(runs, text, t)).into_any_element()
        }
        MdBlockKind::Paragraph { text, runs } => div()
            .id(block_id(section, block.offset))
            .text_sm()
            .line_height(relative(ty.prose_line_height))
            .child(styled(runs, text, t))
            .into_any_element(),
        MdBlockKind::Quote { text, runs } => div()
            .id(block_id(section, block.offset))
            .my_1p5()
            .pl_3()
            .py_1()
            .border_l_2()
            .border_color(t.border)
            .text_sm()
            .text_color(t.dim)
            .line_height(relative(ty.prose_line_height))
            .child(styled(runs, text, t))
            .into_any_element(),
        MdBlockKind::List { items } => div()
            .id(block_id(section, block.offset))
            .flex()
            .flex_col()
            .gap_1()
            .children(items.iter().map(|(number, text, runs)| {
                let marker = number
                    .map(|n| SharedString::from(format!("{n}.")))
                    .unwrap_or("•".into());
                div()
                    .flex()
                    .gap_2()
                    .text_sm()
                    .line_height(relative(ty.prose_line_height))
                    .child(div().text_color(t.dim).child(marker))
                    .child(div().flex_1().child(styled(runs, text, t)))
            }))
            .into_any_element(),
        MdBlockKind::Table { columns, rows } => {
            let mut table = div()
                .id(block_id(section, block.offset))
                .my_1p5()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .overflow_hidden();
            for (index, row) in rows.iter().enumerate() {
                let mut row_div = div().grid().grid_cols(columns.len() as u16).text_xs();
                if index == 0 {
                    row_div = row_div.font_weight(FontWeight::BOLD).bg(t.code_bg);
                } else {
                    row_div = row_div.border_t_1().border_color(t.border);
                }
                table = table.child(row_div.children(row.iter().zip(columns).enumerate().map(
                    |(column, ((text, runs), alignment))| {
                        let cell = div().min_w_0().px_2().py_1();
                        let cell = match alignment {
                            Alignment::Center => cell.text_center(),
                            Alignment::Right => cell.text_right(),
                            _ => cell.text_left(),
                        };
                        let cell = if column > 0 {
                            cell.border_l_1().border_color(t.border)
                        } else {
                            cell
                        };
                        cell.child(styled(runs, text, t))
                    },
                )));
            }
            table.into_any_element()
        }
        MdBlockKind::Code { lang, code } => {
            let mut card = div()
                .id(block_id(section, block.offset))
                .my_1p5()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .bg(t.code_bg)
                .overflow_hidden();
            // 头部栏:语言标签 + 复制(electron md-code-block 头栏模式)。
            let copy_code: SharedString = code.clone().into();
            let mut header = div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .pt_1p5();
            header = match lang {
                Some(lang) => header.child(
                    div()
                        .text_xs()
                        .text_color(t.dim)
                        .child(lang.clone()),
                ),
                None => header.child(div()),
            };
            card = card
                .child(header.child(
                    div()
                        .id(("mdcopy", section * 1_000_003 + block.offset))
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .text_xs()
                        .cursor_pointer()
                        .bg(t.btn)
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.btn_hover))
                        .on_click(move |_, _, cx: &mut App| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                copy_code.clone().into(),
                            ));
                        })
                        .child("复制"),
                ))
                .child(
                    div()
                        .id(("mdcode-content", section * 1_000_003 + block.offset))
                        .px_3()
                        .py_2()
                        .font_family("monospace")
                        .text_size(px(ty.code_font_size))
                        .line_height(relative(ty.prose_line_height))
                        .overflow_x_scroll()
                        .child(crate::highlight::code_text(code, lang.as_deref(), t)),
                );
            card.into_any_element()
        }
    }
}
