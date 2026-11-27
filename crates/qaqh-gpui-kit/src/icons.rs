//! 彩色品牌与文档 SVG 资产、组合 [`AssetSource`] 与命令/文件图标匹配器。
//!
//! - 资产来源: `assets/icons/*.svg`(Material Icon Theme v5.40.0, MIT),
//!   统一规范为 `width="32" height="32"`,经 `img(Resource::Embedded(...))`
//!   保留多色矢量栅格化效果;通用 UI 图标继续由 [`gpui_kit::assets::AllAssets`] 提供。
//! - 持久控件仍遵循 `DESIGN-LAW.md` 与 Lucide `IconName`,本模块专供消息区文件引用、
//!   Shell 运行时命令摘要与用户终端页签档位标识使用。

use std::borrow::Cow;
use std::path::Path;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AssetSource, Hsla, ImageSource, ObjectFit, Resource, Result, SharedString, img, px,
};

macro_rules! define_brand_icons {
    ($( $variant:ident => $file:literal ),* $(,)?) => {
        /// 内嵌彩色语言/工具品牌图标枚举。
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum BrandIcon {
            $( $variant, )*
        }

        impl BrandIcon {
            /// 全部可用品牌图标。
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )* ];

            /// 在 GPUI `AssetSource` 中的嵌入路径。
            pub const fn asset_path(self) -> &'static str {
                match self {
                    $( Self::$variant => concat!("icons/brand/", $file), )*
                }
            }

            /// 对应的静态 SVG 字节切片。
            pub const fn bytes(self) -> &'static [u8] {
                match self {
                    $( Self::$variant => include_bytes!(concat!("../../../assets/icons/", $file)), )*
                }
            }

            /// 按 `AssetSource` 路径反查品牌图标。
            pub fn from_path(path: &str) -> Option<Self> {
                match path {
                    $( concat!("icons/brand/", $file) => Some(Self::$variant), )*
                    _ => None,
                }
            }
        }
    };
}

define_brand_icons! {
    Astro => "astro.svg",
    Bun => "bun.svg",
    C => "c.svg",
    Cmake => "cmake.svg",
    Console => "console.svg",
    Cpp => "cpp.svg",
    CSharp => "csharp.svg",
    Css => "css.svg",
    Database => "database.svg",
    Deno => "deno.svg",
    Diff => "diff.svg",
    Docker => "docker.svg",
    Git => "git.svg",
    Go => "go.svg",
    Graphql => "graphql.svg",
    Html => "html.svg",
    Java => "java.svg",
    JavaScript => "javascript.svg",
    Json => "json.svg",
    Just => "just.svg",
    Kotlin => "kotlin.svg",
    Kubernetes => "kubernetes.svg",
    Lock => "lock.svg",
    Lua => "lua.svg",
    Make => "makefile.svg",
    Markdown => "markdown.svg",
    Mdx => "mdx.svg",
    NodeJs => "nodejs.svg",
    Npm => "npm.svg",
    Php => "php.svg",
    Playwright => "playwright.svg",
    Pnpm => "pnpm.svg",
    PowerShell => "powershell.svg",
    Proto => "proto.svg",
    Python => "python.svg",
    React => "react.svg",
    ReactTs => "react_ts.svg",
    Ruby => "ruby.svg",
    Rust => "rust.svg",
    Sass => "sass.svg",
    Svelte => "svelte.svg",
    Svg => "svg.svg",
    Swift => "swift.svg",
    Terraform => "terraform.svg",
    Toml => "toml.svg",
    TsConfig => "tsconfig.svg",
    TypeScript => "typescript.svg",
    TypeScriptDef => "typescript-def.svg",
    Uv => "uv.svg",
    Vite => "vite.svg",
    Vitest => "vitest.svg",
    Vue => "vue.svg",
    Yaml => "yaml.svg",
    Yarn => "yarn.svg",
    Zig => "zig.svg",
    Zip => "zip.svg",
}

/// 统一图标结果:优先展示彩色品牌图,无法走彩色或无品牌色时回落单色 Lucide。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppIcon {
    Brand(BrandIcon),
    Lucide(IconName),
}

/// 应用级组合资源源:先查内嵌彩色品牌 SVG,再回退完整 Lucide `AllAssets`。
#[derive(Clone, Copy, Debug, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(icon) = BrandIcon::from_path(path) {
            return Ok(Some(Cow::Borrowed(icon.bytes())));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::AllAssets.list(path)?;
        paths.extend(
            BrandIcon::ALL
                .iter()
                .map(|icon| icon.asset_path())
                .filter(|asset_path| asset_path.starts_with(path))
                .map(SharedString::from),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// 以全彩 RGBA 栅格路径渲染内嵌品牌 SVG(保留多色,不受 `svg()` 单色遮罩限制)。
pub fn render_brand_icon(icon: BrandIcon, size_px: f32) -> AnyElement {
    img(ImageSource::Resource(Resource::Embedded(
        SharedString::from(icon.asset_path()),
    )))
    .size(px(size_px))
    .object_fit(ObjectFit::Contain)
    .flex_shrink_0()
    .into_any_element()
}

/// 统一渲染 `AppIcon`:品牌图标走彩色 `img()`,Lucide 走 `Icon` 并应用传入前景色。
pub fn render_app_icon(icon: AppIcon, size_px: f32, lucide_color: Hsla) -> AnyElement {
    match icon {
        AppIcon::Brand(brand) => render_brand_icon(brand, size_px),
        AppIcon::Lucide(name) => Icon::new(name)
            .with_size(Size::Size(px(size_px)))
            .text_color(lucide_color)
            .into_any_element(),
    }
}

/// 根据文件路径匹配彩色文档/语言图标;无特定品牌时返回 `None` 交由上层走 Lucide 兜底。
pub fn file_icon(path: &Path) -> Option<BrandIcon> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if let Some(icon) = match file_name.as_str() {
        "cargo.toml" => Some(BrandIcon::Rust),
        "cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lock"
        | "bun.lockb" => Some(BrandIcon::Lock),
        "package.json" => Some(BrandIcon::NodeJs),
        "tsconfig.json" | "jsconfig.json" => Some(BrandIcon::TsConfig),
        "dockerfile" | "compose.yml" | "compose.yaml" | "docker-compose.yml"
        | "docker-compose.yaml" => Some(BrandIcon::Docker),
        "justfile" | ".justfile" => Some(BrandIcon::Just),
        "makefile" | "gnumakefile" => Some(BrandIcon::Make),
        "cmakelists.txt" => Some(BrandIcon::Cmake),
        "go.mod" | "go.sum" => Some(BrandIcon::Go),
        "pyproject.toml" => Some(BrandIcon::Python),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig" => Some(BrandIcon::Git),
        _ => None,
    } {
        return Some(icon);
    }

    if file_name.ends_with(".d.ts")
        || file_name.ends_with(".d.mts")
        || file_name.ends_with(".d.cts")
    {
        return Some(BrandIcon::TypeScriptDef);
    }

    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match ext.as_str() {
        "rs" => Some(BrandIcon::Rust),
        "ts" | "mts" | "cts" => Some(BrandIcon::TypeScript),
        "tsx" => Some(BrandIcon::ReactTs),
        "js" | "mjs" | "cjs" => Some(BrandIcon::JavaScript),
        "jsx" => Some(BrandIcon::React),
        "py" | "pyi" => Some(BrandIcon::Python),
        "md" | "markdown" => Some(BrandIcon::Markdown),
        "mdx" => Some(BrandIcon::Mdx),
        "json" | "jsonc" | "jsonl" => Some(BrandIcon::Json),
        "toml" => Some(BrandIcon::Toml),
        "yaml" | "yml" => Some(BrandIcon::Yaml),
        "go" => Some(BrandIcon::Go),
        "c" | "h" => Some(BrandIcon::C),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(BrandIcon::Cpp),
        "cs" => Some(BrandIcon::CSharp),
        "java" => Some(BrandIcon::Java),
        "kt" | "kts" => Some(BrandIcon::Kotlin),
        "swift" => Some(BrandIcon::Swift),
        "rb" => Some(BrandIcon::Ruby),
        "php" => Some(BrandIcon::Php),
        "lua" => Some(BrandIcon::Lua),
        "zig" => Some(BrandIcon::Zig),
        "vue" => Some(BrandIcon::Vue),
        "svelte" => Some(BrandIcon::Svelte),
        "astro" => Some(BrandIcon::Astro),
        "html" | "htm" => Some(BrandIcon::Html),
        "css" => Some(BrandIcon::Css),
        "scss" | "sass" => Some(BrandIcon::Sass),
        "svg" => Some(BrandIcon::Svg),
        "proto" => Some(BrandIcon::Proto),
        "graphql" | "gql" => Some(BrandIcon::Graphql),
        "sql" | "db" | "sqlite" | "sqlite3" => Some(BrandIcon::Database),
        "diff" | "patch" => Some(BrandIcon::Diff),
        "lock" => Some(BrandIcon::Lock),
        "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" => Some(BrandIcon::Zip),
        "sh" | "bash" | "zsh" | "fish" => Some(BrandIcon::Console),
        "ps1" | "psm1" | "psd1" => Some(BrandIcon::PowerShell),
        "tf" | "tfvars" => Some(BrandIcon::Terraform),
        _ => None,
    }
}

/// 根据交互式终端档位名(`pwsh`/`bash`/`cmd`)返回对应图标。
pub fn shell_kind_icon(kind: &str) -> Option<AppIcon> {
    let lower = kind.trim().to_ascii_lowercase();
    let short = lower.rsplit(['/', '\\']).next().unwrap_or(lower.as_str());
    let short = short.strip_suffix(".exe").unwrap_or(short);
    match short {
        "pwsh" | "powershell" => Some(AppIcon::Brand(BrandIcon::PowerShell)),
        "bash" | "sh" | "zsh" | "fish" => Some(AppIcon::Brand(BrandIcon::Console)),
        "cmd" => Some(AppIcon::Lucide(IconName::Terminal)),
        _ => None,
    }
}

/// 根据 Shell 命令行与显式 `shell` 参数识别开发工具/运行时图标。
pub fn command_icon(cmd: Option<&str>, shell: Option<&str>) -> Option<AppIcon> {
    if let Some(cmd) = cmd
        && let Some(found) = detect_command_icon(cmd)
    {
        return Some(found);
    }
    shell.and_then(shell_kind_icon)
}

fn detect_command_icon(command: &str) -> Option<AppIcon> {
    for segment in command_segments(command) {
        let words = first_words(segment);
        let mut idx = 0usize;
        while idx < words.len() && is_env_assignment(&words[idx]) {
            idx += 1;
        }
        while idx < words.len() && is_command_wrapper(&words[idx].to_ascii_lowercase()) {
            idx += 1;
        }
        let Some(word) = words.get(idx) else {
            continue;
        };
        let leaf = word.rsplit(['/', '\\']).next().unwrap_or(word.as_str());
        let executable = strip_executable_ext(leaf).to_ascii_lowercase();
        if let Some(icon) = lookup_command_icon(&executable) {
            return Some(icon);
        }
    }
    None
}

fn lookup_command_icon(executable: &str) -> Option<AppIcon> {
    match executable {
        "git" => Some(AppIcon::Brand(BrandIcon::Git)),
        "gh" => Some(AppIcon::Lucide(IconName::Github)),
        "pnpm" => Some(AppIcon::Brand(BrandIcon::Pnpm)),
        "npm" | "npx" => Some(AppIcon::Brand(BrandIcon::Npm)),
        "yarn" => Some(AppIcon::Brand(BrandIcon::Yarn)),
        "bun" | "bunx" => Some(AppIcon::Brand(BrandIcon::Bun)),
        "cargo" | "rustc" | "rustup" | "rustfmt" | "clippy-driver" => {
            Some(AppIcon::Brand(BrandIcon::Rust))
        }
        "python" | "python3" | "py" | "pip" | "pip3" | "pytest" => {
            Some(AppIcon::Brand(BrandIcon::Python))
        }
        "uv" | "uvx" => Some(AppIcon::Brand(BrandIcon::Uv)),
        "node" | "nodejs" => Some(AppIcon::Brand(BrandIcon::NodeJs)),
        "deno" => Some(AppIcon::Brand(BrandIcon::Deno)),
        "tsc" | "tsserver" | "tsx" => Some(AppIcon::Brand(BrandIcon::TypeScript)),
        "vite" => Some(AppIcon::Brand(BrandIcon::Vite)),
        "vitest" => Some(AppIcon::Brand(BrandIcon::Vitest)),
        "playwright" => Some(AppIcon::Brand(BrandIcon::Playwright)),
        "docker" | "docker-compose" | "podman" => Some(AppIcon::Brand(BrandIcon::Docker)),
        "kubectl" | "helm" => Some(AppIcon::Brand(BrandIcon::Kubernetes)),
        "terraform" | "tofu" => Some(AppIcon::Brand(BrandIcon::Terraform)),
        "just" => Some(AppIcon::Brand(BrandIcon::Just)),
        "make" | "gmake" | "mingw32-make" => Some(AppIcon::Brand(BrandIcon::Make)),
        "cmake" => Some(AppIcon::Brand(BrandIcon::Cmake)),
        "go" => Some(AppIcon::Brand(BrandIcon::Go)),
        "bash" | "sh" | "zsh" | "fish" => Some(AppIcon::Brand(BrandIcon::Console)),
        "pwsh" | "powershell" => Some(AppIcon::Brand(BrandIcon::PowerShell)),
        _ => None,
    }
}

fn strip_executable_ext(name: &str) -> &str {
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if name.len() > suffix.len() {
            let split_at = name.len() - suffix.len();
            if name.is_char_boundary(split_at)
                && let Some(tail) = name.get(split_at..)
                && tail.eq_ignore_ascii_case(suffix)
            {
                return name.get(..split_at).unwrap_or(name);
            }
        }
    }
    name
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_command_wrapper(word: &str) -> bool {
    matches!(
        word,
        "sudo" | "env" | "time" | "command" | "exec" | "nohup" | "nice" | "corepack"
    )
}

/// 仅在未加引号的 `;` / `|` / `||` / `&&` / `\n` 处切分命令段(展示级启发式)。
fn command_segments(command: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut skip_next_at: Option<usize> = None;

    for (idx, ch) in command.char_indices() {
        if skip_next_at == Some(idx) {
            skip_next_at = None;
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            continue;
        }
        let next_is_same = command
            .get(idx + ch.len_utf8()..)
            .and_then(|rest| rest.chars().next())
            == Some(ch);
        let is_sep = ch == ';' || ch == '|' || ch == '\n' || (ch == '&' && next_is_same);
        if is_sep {
            if let Some(seg) = command.get(start..idx) {
                segments.push(seg);
            }
            if (ch == '|' || ch == '&') && next_is_same {
                let next_pos = idx + ch.len_utf8();
                skip_next_at = Some(next_pos);
                start = next_pos + ch.len_utf8();
            } else {
                start = idx + ch.len_utf8();
            }
        }
    }
    if let Some(tail) = command.get(start..) {
        segments.push(tail);
    }
    segments
}

/// 提取命令段前几个可执行词元(兼容单/双/反引号路径)。
fn first_words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = segment.chars().peekable();
    while words.len() < 6 {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        if matches!(first, ';' | '&' | '|') {
            break;
        }
        let mut word = String::new();
        match first {
            '"' => {
                chars.next();
                while let Some(ch) = chars.next() {
                    if ch == '\\' {
                        if matches!(chars.peek(), Some('"' | '\\')) {
                            if let Some(next) = chars.next() {
                                word.push(next);
                            }
                        } else {
                            word.push('\\');
                        }
                    } else if ch == '"' {
                        break;
                    } else {
                        word.push(ch);
                    }
                }
            }
            '\'' => {
                chars.next();
                while let Some(ch) = chars.next() {
                    if ch == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                            word.push('\'');
                        } else {
                            break;
                        }
                    } else {
                        word.push(ch);
                    }
                }
            }
            '`' => {
                chars.next();
                for ch in chars.by_ref() {
                    if ch == '`' {
                        break;
                    }
                    word.push(ch);
                }
            }
            _ => {
                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() || matches!(ch, ';' | '&' | '|') {
                        break;
                    }
                    word.push(ch);
                    chars.next();
                }
            }
        }
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_icon_handles_wrappers_builtins_and_paths() {
        assert_eq!(
            command_icon(Some("cd crates && RUST_BACKTRACE=1 cargo test -p qaqh-gpui"), None),
            Some(AppIcon::Brand(BrandIcon::Rust))
        );
        assert_eq!(
            command_icon(Some(r#""C:\Program Files\Git\bin\git.exe" status"#), None),
            Some(AppIcon::Brand(BrandIcon::Git))
        );
        assert_eq!(
            command_icon(Some("echo hi"), Some("pwsh")),
            Some(AppIcon::Brand(BrandIcon::PowerShell))
        );
        assert_eq!(
            command_icon(Some("gh pr view 12"), None),
            Some(AppIcon::Lucide(IconName::Github))
        );
    }

    #[test]
    fn file_icon_recognizes_special_names_and_extensions() {
        assert_eq!(file_icon(Path::new("crates/foo/src/main.rs")), Some(BrandIcon::Rust));
        assert_eq!(file_icon(Path::new("src/types/global.d.ts")), Some(BrandIcon::TypeScriptDef));
        assert_eq!(file_icon(Path::new("docs/DESIGN-LAW.md")), Some(BrandIcon::Markdown));
        assert_eq!(file_icon(Path::new("Cargo.toml")), Some(BrandIcon::Rust));
        assert_eq!(file_icon(Path::new("package.json")), Some(BrandIcon::NodeJs));
    }

    #[test]
    fn app_assets_embeds_all_brand_icons() {
        for icon in BrandIcon::ALL {
            assert!(!icon.bytes().is_empty());
            assert_eq!(BrandIcon::from_path(icon.asset_path()), Some(*icon));
        }
    }
}
