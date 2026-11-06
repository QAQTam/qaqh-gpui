//! AST 代码高亮(M5):tree-sitter-highlight 单发高亮路径。
//!
//! 借鉴 zed 的设计(不搬 GPL 代码):`highlight_text_resolved` 的
//! 「直接 parse→query→runs,不进 buffer/syntax_map」思想,以及
//! `TextHighlightCache` 的「内容寻址缓存,未变代码块跨解析复用」思想。
//! query 文件取自 nvim-treesitter(Apache-2.0,见 assets/queries/README)。
//! 无语言 / 未注册语言的代码块走纯样式 fallback(markdown.rs 侧)。

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use gpui::FontStyle;
use gpui::HighlightStyle;
use gpui::StyledText;
use gpui::px;
use tree_sitter_highlight::HighlightConfiguration;
use tree_sitter_highlight::HighlightEvent;
use tree_sitter_highlight::Highlighter;

use crate::theme::Palette;

/// 捕获名表(与 configure 对齐;查询里未列出的捕获渲染为无样式)。
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "constant",
    "constant.builtin",
    "constant.macro",
    "constructor",
    "embedded",
    "error",
    "function",
    "function.builtin",
    "function.call",
    "function.macro",
    "include",
    "keyword",
    "keyword.command",
    "keyword.function",
    "keyword.import",
    "keyword.iterator",
    "keyword.modifier",
    "keyword.operator",
    "keyword.return",
    "keyword.type",
    "label",
    "method",
    "module",
    "number",
    "operator",
    "parameter",
    "property",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "repeat",
    "spell",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "tag.attribute",
    "type",
    "type.builtin",
    "type.definition",
    "type.qualifier",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
];

/// info string → 引擎语言键。
fn canonical(lang: &str) -> Option<&'static str> {
    match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some("rust"),
        "python" | "py" => Some("python"),
        "javascript" | "js" => Some("javascript"),
        "typescript" | "ts" => Some("typescript"),
        "go" | "golang" => Some("go"),
        "json" | "jsonc" => Some("json"),
        "bash" | "sh" | "shell" | "zsh" => Some("bash"),
        _ => None,
    }
}

/// 引擎 + 内容寻址缓存:(语言, 源哈希) → runs。
/// 流式时只有内容变化的块重算,已完成块跨帧复用同一 Arc。
struct Engine {
    configs: HashMap<&'static str, HighlightConfiguration>,
    cache: HashMap<(String, u64), Arc<Vec<(Range<usize>, usize)>>>,
}

impl Engine {
    fn new() -> Self {
        let mut configs = HashMap::new();
        macro_rules! load {
            ($key:literal, $query:literal, $language:expr) => {
                if let Ok(mut config) = HighlightConfiguration::new(
                    $language.into(),
                    $key,
                    include_str!($query),
                    "",
                    "",
                ) {
                    config.configure(HIGHLIGHT_NAMES);
                    configs.insert($key, config);
                }
            };
        }
        load!(
            "rust",
            "../assets/queries/rust.scm",
            tree_sitter_rust::LANGUAGE
        );
        load!(
            "python",
            "../assets/queries/python.scm",
            tree_sitter_python::LANGUAGE
        );
        load!(
            "javascript",
            "../assets/queries/javascript.scm",
            tree_sitter_javascript::LANGUAGE
        );
        load!(
            "typescript",
            "../assets/queries/typescript.scm",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT
        );
        load!("go", "../assets/queries/go.scm", tree_sitter_go::LANGUAGE);
        load!(
            "json",
            "../assets/queries/json.scm",
            tree_sitter_json::LANGUAGE
        );
        load!(
            "bash",
            "../assets/queries/bash.scm",
            tree_sitter_bash::LANGUAGE
        );
        Self {
            configs,
            cache: HashMap::new(),
        }
    }

    fn highlight(
        &mut self,
        lang: &'static str,
        code: &str,
    ) -> Option<Arc<Vec<(Range<usize>, usize)>>> {
        let hash = fnv1a(code);
        let key = (lang.to_string(), hash);
        if let Some(runs) = self.cache.get(&key) {
            return Some(runs.clone());
        }
        let config = self.configs.get_mut(lang)?;
        let mut highlighter = Highlighter::new();
        let events = highlighter
            .highlight(config, code.as_bytes(), None, |_| None)
            .ok()?;
        let mut runs = Vec::new();
        let mut current: Option<usize> = None;
        for event in events.flatten() {
            match event {
                HighlightEvent::HighlightStart(index) => current = Some(index.0),
                HighlightEvent::HighlightEnd => current = None,
                HighlightEvent::Source { start, end } => {
                    if let Some(index) = current {
                        runs.push((start..end, index));
                    }
                }
            }
        }
        let runs = Arc::new(runs);
        self.cache.insert(key, runs.clone());
        Some(runs)
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();

/// 捕获名 → 样式(主题色板驱动,亮暗自适应)。
fn name_style(name: &str, t: &Palette) -> HighlightStyle {
    if name.starts_with("comment") || name == "spell" {
        return HighlightStyle {
            color: Some(t.syn_comment),
            font_style: Some(FontStyle::Italic),
            ..Default::default()
        };
    }
    match name {
        "string" | "string.escape" | "string.special" | "embedded" => HighlightStyle {
            color: Some(t.syn_string),
            ..Default::default()
        },
        "number" | "boolean" | "constant" | "constant.builtin" | "constant.macro" | "label" => {
            HighlightStyle {
                color: Some(t.syn_number),
                ..Default::default()
            }
        }
        "function" | "function.builtin" | "function.call" | "function.macro" | "method"
        | "attribute" => HighlightStyle {
            color: Some(t.syn_function),
            ..Default::default()
        },
        "type" | "type.builtin" | "type.definition" | "type.qualifier" | "constructor" | "module"
        | "tag" | "variable.builtin" => HighlightStyle {
            color: Some(t.syn_type),
            ..Default::default()
        },
        "keyword" | "keyword.command" | "keyword.function" | "keyword.import"
        | "keyword.iterator" | "keyword.modifier" | "keyword.operator" | "keyword.return"
        | "keyword.type" | "repeat" | "include" | "operator" => HighlightStyle {
            color: Some(t.syn_keyword),
            ..Default::default()
        },
        "punctuation.bracket" | "punctuation.delimiter" | "punctuation.special" | "tag.attribute" => {
            HighlightStyle {
                color: Some(t.faint),
                ..Default::default()
            }
        }
        "error" => HighlightStyle {
            color: Some(t.bad),
            underline: Some(gpui::UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: true,
            }),
            ..Default::default()
        },
        _ => HighlightStyle::default(),
    }
}

/// 代码块文本:已知语言走 AST 高亮,否则纯样式 fallback。
pub fn code_text(code: &str, lang: Option<&str>, t: &Palette) -> StyledText {
    let Some(runs) = lang.and_then(canonical).and_then(|lang| {
        let engine = ENGINE.get_or_init(|| Mutex::new(Engine::new()));
        let mut guard = engine.lock().expect("highlight engine mutex poisoned");
        guard.highlight(lang, code)
    }) else {
        return StyledText::new(code);
    };
    StyledText::new(code).with_highlights(
        runs.iter()
            .map(|(range, index)| (range.clone(), name_style(HIGHLIGHT_NAMES[*index], t))),
    )
}

#[cfg(test)]
mod tests {
    use super::Engine;

    /// 资产集成冒烟:7 个 query 全部能随 grammar 编译加载(Apache query +
    /// MIT grammar 与 highlight 运行时 ABI 兼容),并产出非空 runs。
    /// 这是唯一能验证 assets/queries 与 grammar ABI 的路径,编译期查不出。
    #[test]
    fn engine_loads_all_queries_and_highlights() {
        let mut engine = Engine::new();
        let langs = ["rust", "python", "javascript", "typescript", "go", "json", "bash"];
        for lang in langs {
            assert!(
                engine.configs.contains_key(lang),
                "{lang} query 加载失败(assets/queries/{lang}.scm 与 grammar 不兼容)"
            );
            let code = match lang {
                "rust" => "fn main() { let x = 1; }",
                "python" => "def f(x):\n    return x + 1",
                "javascript" | "typescript" => "function f(x) { return x ?? 1; }",
                "go" => "func f(x int) int { return x }",
                "json" => "{\"a\": 1}",
                "bash" => "echo hi",
                _ => continue,
            };
            let runs = engine
                .highlight(lang, code)
                .unwrap_or_else(|| panic!("{lang} 高亮产出为空"));
            assert!(!runs.is_empty(), "{lang} 无高亮 runs");
        }
    }
}
