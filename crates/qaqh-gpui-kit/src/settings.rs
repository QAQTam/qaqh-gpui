//! 设置(M6):本地应用偏好。
//!
//! 模型借鉴 zed 的设计(GPL,仅借鉴):「UI 无状态、读写全落文件、
//! 变更 → refresh_windows」。分层 = 代码内默认值 + 用户 JSON 文件
//! (字段缺省自动落默认,serde(default))。
//!
//! 注意分层边界:daemon 侧配置(config.toml/provider/profile)是另一层,
//! 走 RingingV3 `config.patchFull` 命令面(`qaqh-gpui-shell::settings_ui`)。

use std::path::PathBuf;

use gpui_kit::App;
use gpui_kit::Global;
use serde::Deserialize;
use serde::Serialize;

use crate::theme::{ThemeMode, ThemePreset};

/// GPU 动效档位:Full(120fps 完整物理弹簧与进退场) / Reduced(仅轻柔淡入淡出) / Off(零动效)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MotionLevel {
    #[default]
    Full,
    Reduced,
    Off,
}

impl MotionLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "流畅物理",
            Self::Reduced => "精简动效",
            Self::Off => "零动效",
        }
    }
}

/// UI 密度:影响时间线留白(electron 的 --space 节奏三档)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
    Compact,
    #[default]
    Standard,
    Comfortable,
}

impl Density {
    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "紧凑",
            Self::Standard => "标准",
            Self::Comfortable => "舒适",
        }
    }

    /// 留白偏移(px):紧凑 -2 / 标准 0 / 舒适 +2。
    pub fn shift(self) -> f32 {
        match self {
            Self::Compact => -2.0,
            Self::Standard => 0.0,
            Self::Comfortable => 2.0,
        }
    }
}

/// 终端与命令输出面板配色方案。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalScheme {
    #[default]
    Auto,
    Obsidian,
    OneDark,
    TokyoNight,
    WarmPaper,
}

impl TerminalScheme {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Obsidian,
        Self::OneDark,
        Self::TokyoNight,
        Self::WarmPaper,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "跟随主题",
            Self::Obsidian => "极夜黑",
            Self::OneDark => "One Dark",
            Self::TokyoNight => "Tokyo Night",
            Self::WarmPaper => "暖纸",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceContent {
    pub theme_mode: ThemeMode,
    pub theme_preset: ThemePreset,
    pub motion_level: MotionLevel,
    pub density: Density,
    pub terminal_scheme: TerminalScheme,
}

impl Default for AppearanceContent {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::default(),
            theme_preset: ThemePreset::default(),
            motion_level: MotionLevel::default(),
            density: Density::default(),
            terminal_scheme: TerminalScheme::default(),
        }
    }
}

/// 消息区最大列宽(px);窄窗口或二分抽屉压缩时通过 `.w_full().max_w(...)` 自动收缩。
pub const MESSAGE_MAX_WIDTH_PX: f32 = 880.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypographyContent {
    /// 全 UI rem 基准(等价 zed ui_font_size):改它 = 所有 rem 字号等比缩放。
    pub ui_font_size: f32,
    /// UI 字体族;空串 = 平台默认(gpui 文本系统回退链兜底)。
    pub ui_font_family: String,
    /// 代码块字号(绝对 px,不随 rem)。
    pub code_font_size: f32,
    /// 后台执行观察窗终端字号(绝对 px,默认 10px 以高密度对齐 160 列)。
    pub exec_font_size: f32,
    /// 消息段落行高倍数。
    pub prose_line_height: f32,
}

impl Default for TypographyContent {
    fn default() -> Self {
        Self {
            ui_font_size: 16.0,
            ui_font_family: String::new(),
            code_font_size: 12.0,
            exec_font_size: 10.0,
            prose_line_height: 1.55,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorContent {
    /// 流式时自动跟随底部(滚轮上滚暂停/下滚恢复)。
    pub stream_follow: bool,
    /// 折叠过程轨迹：把思考链、工具调用与已被新正文替代的中间回复收进限高视窗。
    pub collapse_work_trace: bool,
    /// 工具/执行输出尾窗上限(字节,UTF-8 安全截断)。
    pub output_tail_max: usize,
    /// 实验性特性：`apply_patch` 开始流式输出时自动右侧展开目标文件并锁定行号预演改动。
    pub exp_live_patch_preview: bool,
}

impl Default for BehaviorContent {
    fn default() -> Self {
        Self {
            stream_follow: true,
            collapse_work_trace: true,
            output_tail_max: 16 * 1024,
            exp_live_patch_preview: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsContent {
    pub appearance: AppearanceContent,
    pub typography: TypographyContent,
    pub behavior: BehaviorContent,
}

impl SettingsContent {
    /// 渲染层快照(避免深层结构穿层借用)。
    pub fn typography(&self) -> Typography {
        Typography {
            code_font_size: self.typography.code_font_size,
            exec_font_size: self.typography.exec_font_size,
            prose_line_height: self.typography.prose_line_height,
            density_shift: self.appearance.density.shift(),
        }
    }
}

/// 渲染用排版快照。
#[derive(Clone, Copy, Debug)]
pub struct Typography {
    pub code_font_size: f32,
    pub exec_font_size: f32,
    pub prose_line_height: f32,
    pub density_shift: f32,
}

struct SettingsState {
    content: SettingsContent,
    path: PathBuf,
}

impl Global for SettingsState {}

/// XDG 配置路径:$XDG_CONFIG_HOME/qaqh-gpui/settings.json。
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    base.join("qaqh-gpui").join("settings.json")
}

/// 启动加载:文件缺字段自动落默认;解析失败回默认值并提示。
pub fn init(cx: &mut App) {
    let path = config_path();
    let content = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| match serde_json::from_str::<SettingsContent>(&text) {
            Ok(content) => Some(content),
            Err(err) => {
                eprintln!("qaqh-gpui: 设置解析失败,使用默认值({err}):{path:?}");
                None
            }
        })
        .unwrap_or_default();
    cx.set_global(SettingsState { content, path });
}

pub fn get(cx: &App) -> &SettingsContent {
    &cx.global::<SettingsState>().content
}

/// 唯一写入口:改内存 → 原子落盘 → 应用主题副作用(内部已 refresh_windows)。
pub fn update(cx: &mut App, f: impl FnOnce(&mut SettingsContent)) {
    let state = cx.global_mut::<SettingsState>();
    f(&mut state.content);
    save(&state.path, &state.content);
    let mode = state.content.appearance.theme_mode;
    let preset = state.content.appearance.theme_preset;
    crate::theme::apply_appearance(mode, preset, cx);
}

/// tmp + rename 原子写,防半截文件。
fn save(path: &PathBuf, content: &SettingsContent) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    match serde_json::to_string_pretty(content) {
        Ok(text) => {
            if let Err(err) = std::fs::write(&tmp, text)
                .and_then(|()| std::fs::rename(&tmp, path))
            {
                eprintln!("qaqh-gpui: 设置写入失败:{err}");
            }
        }
        Err(err) => eprintln!("qaqh-gpui: 设置序列化失败:{err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsContent;

    /// 持久化契约:settings.json 是跨版本承诺——
    /// 1) 默认值 roundtrip 恒等;2) 旧文件/手写缺字段的 JSON 必须能读且缺项落默认。
    #[test]
    fn settings_json_contract() {
        let default = SettingsContent::default();
        let text = serde_json::to_string_pretty(&default).expect("serialize");
        let parsed: SettingsContent = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(parsed, default, "roundtrip 后内容改变");

        // 旧版本文件:只含部分字段(甚至空对象)也要能读。
        let legacy = "{}";
        let parsed: SettingsContent = serde_json::from_str(legacy).expect("legacy parse");
        assert_eq!(parsed, default, "缺字段未落默认");
        assert_eq!(parsed.typography.exec_font_size, 10.0);

        let partial = r#"{"typography": {"ui_font_size": 18.0}}"#;
        let parsed: SettingsContent = serde_json::from_str(partial).expect("partial parse");
        assert_eq!(parsed.typography.ui_font_size, 18.0);
        assert_eq!(parsed.typography.prose_line_height, 1.55, "未提字段应落默认");
    }
}
