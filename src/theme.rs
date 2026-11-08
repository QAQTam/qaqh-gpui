//! 主题(M1):语义色板 × 亮/暗基线 + 全局状态 + 系统外观跟随。
//! 结构借鉴 zed 的 Appearance/SystemAppearance/Dynamic 模型(仅借鉴设计;
//! zed crates/theme 为 GPL,不搬代码)。暗色沿用本仓原色值,亮色参考
//! MIT 许可的 zed assets/themes(One Light 系)色值。

use gpui::App;
use gpui::Global;
use gpui::Hsla;
use gpui::WindowAppearance;
use gpui::rgb;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl From<WindowAppearance> for Appearance {
    fn from(value: WindowAppearance) -> Self {
        match value {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }
}

/// 主题模式:跟随系统外观,或固定亮/暗(真值存于 settings,主题只持快照)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "亮色",
            Self::Dark => "暗色",
        }
    }

    fn resolve(self, system: Appearance) -> Palette {
        match self {
            Self::System => Palette::for_appearance(system),
            Self::Light => Palette::light(),
            Self::Dark => Palette::dark(),
        }
    }
}

/// 语义色槽:UI 一律经此取色,不再写裸 rgb 常量。
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Hsla,
    pub panel: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub dim: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_bg: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
    pub bad: Hsla,
    pub bubble: Hsla,
    pub card: Hsla,
    pub warn_bg: Hsla,
    pub btn: Hsla,
    pub btn_hover: Hsla,
    pub code_bg: Hsla,
    /// 语法高亮槽(M5):keyword/string/number(function)/type/comment。
    pub syn_keyword: Hsla,
    pub syn_string: Hsla,
    pub syn_number: Hsla,
    pub syn_function: Hsla,
    pub syn_type: Hsla,
    pub syn_comment: Hsla,
}

impl Palette {
    fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self::light(),
            Appearance::Dark => Self::dark(),
        }
    }

    fn dark() -> Self {
        Self {
            bg: rgb(0x111419).into(),
            panel: rgb(0x171b22).into(),
            border: rgb(0x242a34).into(),
            text: rgb(0xd8dee9).into(),
            dim: rgb(0x8a93a2).into(),
            faint: rgb(0x5c6675).into(),
            accent: rgb(0x4f9cf9).into(),
            accent_bg: rgb(0x1f3350).into(),
            hover: rgb(0x1e242e).into(),
            selected: rgb(0x232c3a).into(),
            ok: rgb(0x4fbf7a).into(),
            warn: rgb(0xd9a03f).into(),
            bad: rgb(0xe06c75).into(),
            bubble: rgb(0x1e2a3d).into(),
            card: rgb(0x131720).into(),
            warn_bg: rgb(0x241f14).into(),
            btn: rgb(0x1f2630).into(),
            btn_hover: rgb(0x27303d).into(),
            code_bg: rgb(0x161b23).into(),
            syn_keyword: rgb(0xc678dd).into(),
            syn_string: rgb(0x98c379).into(),
            syn_number: rgb(0xd19a66).into(),
            syn_function: rgb(0x61afef).into(),
            syn_type: rgb(0xe5c07b).into(),
            syn_comment: rgb(0x5c6370).into(),
        }
    }

    fn light() -> Self {
        Self {
            bg: rgb(0xfafafa).into(),
            panel: rgb(0xf0f0f0).into(),
            border: rgb(0xd5d5d5).into(),
            text: rgb(0x383a42).into(),
            dim: rgb(0x696c77).into(),
            faint: rgb(0xa0a1a7).into(),
            accent: rgb(0x4078f2).into(),
            accent_bg: rgb(0xdde7fa).into(),
            hover: rgb(0xe9e9e9).into(),
            selected: rgb(0xdde7fa).into(),
            ok: rgb(0x50a14f).into(),
            warn: rgb(0xc18401).into(),
            bad: rgb(0xe45649).into(),
            bubble: rgb(0xdde7fa).into(),
            card: rgb(0xffffff).into(),
            warn_bg: rgb(0xfbf3db).into(),
            btn: rgb(0xe9e9e9).into(),
            btn_hover: rgb(0xdedede).into(),
            code_bg: rgb(0xf4f4f4).into(),
            syn_keyword: rgb(0xa626a4).into(),
            syn_string: rgb(0x50a14f).into(),
            syn_number: rgb(0x986801).into(),
            syn_function: rgb(0x4078f2).into(),
            syn_type: rgb(0xc18401).into(),
            syn_comment: rgb(0xa0a1a7).into(),
        }
    }
}

struct ThemeState {
    appearance: Appearance,
    palette: Palette,
}

impl Global for ThemeState {}

/// 启动初始化(须在 settings::init 与 gpui_component::init 之后):快照系统外观,
/// 按设置模式选色板,并把品牌色映射进组件库主题。
pub fn init(cx: &mut App) {
    let appearance = Appearance::from(cx.window_appearance());
    let mode = crate::settings::get(cx).appearance.theme_mode;
    cx.set_global(ThemeState {
        appearance,
        palette: mode.resolve(appearance),
    });
    sync_component_theme(cx);
}

/// 设置的 theme_mode 变化时重选色板并刷新窗口。
pub fn apply_mode(mode: ThemeMode, cx: &mut App) {
    let appearance = cx.global::<ThemeState>().appearance;
    let palette = mode.resolve(appearance);
    let state = cx.global_mut::<ThemeState>();
    state.palette = palette;
    sync_component_theme(cx);
    cx.refresh_windows();
}

/// 系统外观变化:更新快照;设置模式为 System 时重选色板。
pub fn system_appearance_changed(appearance: WindowAppearance, cx: &mut App) {
    let mode = crate::settings::get(cx).appearance.theme_mode;
    let state = cx.global_mut::<ThemeState>();
    state.appearance = appearance.into();
    if mode == ThemeMode::System {
        state.palette = Palette::for_appearance(state.appearance);
    }
    sync_component_theme(cx);
    cx.refresh_windows();
}

/// 把本仓色板映射进 gpui-component 全局主题:组件(按钮/输入框/菜单/弹出层)
/// 与手绘界面共用同一套品牌色。先按设置模式切亮/暗(会重载注册主题,
/// 覆盖定制色),再用 Theme::update 写入色板——顺序不可反。
pub fn sync_component_theme(cx: &mut App) {
    match crate::settings::get(cx).appearance.theme_mode {
        ThemeMode::System => gpui_component::Theme::sync_system_appearance(None, cx),
        ThemeMode::Light => gpui_component::Theme::change(gpui_component::ThemeMode::Light, None, cx),
        ThemeMode::Dark => gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx),
    }
    let p = *cx.colors();
    gpui_component::Theme::update(cx, |theme| {
        let c = &mut theme.colors;
        c.background = p.bg;
        c.foreground = p.text;
        c.border = p.border;
        c.input = p.border;
        c.ring = p.accent;
        c.popover = p.panel;
        c.muted = p.hover;
        c.muted_foreground = p.dim;
        c.accent = p.hover;
        c.accent_foreground = p.text;
        c.primary = p.accent;
        c.primary_foreground = p.bg;
        c.secondary = p.btn;
        c.secondary_foreground = p.text;
        c.danger = p.bad;
        c.danger_foreground = p.bg;
        c.success = p.ok;
        c.warning = p.warn;
        c.link = p.accent;
        c.selection = p.selected;
        c.title_bar = p.panel;
        c.sidebar = p.panel;
        c.sidebar_border = p.border;
        c.tab_active = p.card;
        c.button_primary = p.accent;
        c.button_primary_foreground = p.bg;
        c.button_secondary = p.btn;
        c.button_secondary_hover = p.btn_hover;
        c.button_secondary_foreground = p.text;
    });
}

pub trait ActiveTheme {
    fn colors(&self) -> &Palette;
    fn theme_mode(&self) -> ThemeMode;
}

impl ActiveTheme for App {
    fn colors(&self) -> &Palette {
        &self.global::<ThemeState>().palette
    }

    fn theme_mode(&self) -> ThemeMode {
        crate::settings::get(self).appearance.theme_mode
    }
}
