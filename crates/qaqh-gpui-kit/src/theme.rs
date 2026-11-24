//! 主题(DESIGN LAW v3.0 · v5.0.0):
//! 双风格预设(WarmPaper 暖纸工作台 / PureStudio 纯白极简) × 亮/暗基线。
//! 彻底去除冷紫蓝塑料色,采用中性石墨/暖纸/微细描边体系。

use gpui_kit::App;
use gpui_kit::Global;
use gpui_kit::Hsla;
use gpui_kit::WindowAppearance;
use gpui_kit::hsla;
use gpui_kit::rgb;
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

    fn resolve(self, system: Appearance, preset: ThemePreset) -> Palette {
        match self {
            Self::System => Palette::for_appearance(system, preset),
            Self::Light => Palette::light(preset),
            Self::Dark => Palette::dark(preset),
        }
    }
}

/// 外观风格预设:WarmPaper(默认,暖米白纸感舞台) / PureStudio(纯白极简舞台)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemePreset {
    #[default]
    WarmPaper,
    PureStudio,
}

impl ThemePreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::WarmPaper => "暖纸工作台",
            Self::PureStudio => "纯白极简",
        }
    }
}

/// 语义色槽:UI 一律经此取色,禁止业务层写裸 rgb 常量。
#[derive(Clone, Copy)]
pub struct Palette {
    /// L0 外围半透明材质底槽(透出 Win11 Mica)。
    pub chrome: Hsla,
    /// L1 完全不透明悬浮主舞台底色。
    pub stage: Hsla,
    /// L2 柔和次级面板(轨迹组卡、代码块头、设置左导航)。
    pub surface_subtle: Hsla,
    /// 极柔结构细边。
    pub border_subtle: Hsla,
    /// 暗色顶部镜面高光线 / 亮色顶边提亮。
    pub specular: Hsla,
    /// 投影墨色基调。
    pub shadow_ink: Hsla,
    /// 主强调色(深石墨/近黑/亮白)悬停态。
    pub accent_hover: Hsla,
    /// 主强调色柔和透明底。
    pub accent_soft: Hsla,
    /// 主强调色上的前景色(亮色纯白 / 暗色深黑)。
    pub on_accent: Hsla,
    /// 模态弹窗背景压暗遮罩。
    pub scrim: Hsla,

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
    pub card_hover: Hsla,
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
    fn for_appearance(appearance: Appearance, preset: ThemePreset) -> Self {
        match appearance {
            Appearance::Light => Self::light(preset),
            Appearance::Dark => Self::dark(preset),
        }
    }

    fn dark(preset: ThemePreset) -> Self {
        let bg: Hsla = rgb(0x111113).into();
        let accent: Hsla = rgb(0xf4f4f5).into();
        let (stage, card, card_hover, surface_subtle, bubble, btn, btn_hover, selected, code_bg) =
            match preset {
                ThemePreset::WarmPaper => (
                    rgb(0x191816).into(),
                    rgb(0x21201d).into(),
                    rgb(0x292824).into(),
                    rgb(0x22211e).into(),
                    rgb(0x272521).into(),
                    rgb(0x292723).into(),
                    rgb(0x34312c).into(),
                    rgb(0x2b2924).into(),
                    rgb(0x262420).into(),
                ),
                ThemePreset::PureStudio => (
                    rgb(0x161619).into(),
                    rgb(0x1e1e22).into(),
                    rgb(0x26262b).into(),
                    rgb(0x1c1c20).into(),
                    rgb(0x252529).into(),
                    rgb(0x27272b).into(),
                    rgb(0x323237).into(),
                    rgb(0x29292d).into(),
                    rgb(0x25252a).into(),
                ),
            };
        Self {
            chrome: Hsla { a: 0.84, ..bg },
            stage,
            surface_subtle,
            border_subtle: hsla(0., 0., 1.0, 0.08),
            specular: hsla(0., 0., 1.0, 0.06),
            shadow_ink: hsla(0., 0., 0.0, 0.36),
            accent_hover: rgb(0xd4d4d8).into(),
            accent_soft: hsla(0., 0., 1.0, 0.10),
            on_accent: rgb(0x111114).into(),
            scrim: hsla(0., 0., 0.0, 0.54),

            bg,
            panel: rgb(0x141416).into(),
            border: hsla(0., 0., 1.0, 0.14),
            text: rgb(0xf4f4f5).into(),
            dim: rgb(0xa1a1aa).into(),
            faint: rgb(0x71717a).into(),
            accent,
            accent_bg: rgb(0x2a2a2e).into(),
            hover: hsla(0., 0., 1.0, 0.06),
            selected,
            ok: rgb(0x34d399).into(),
            warn: rgb(0xf59e0b).into(),
            bad: rgb(0xf87171).into(),
            bubble,
            card,
            card_hover,
            warn_bg: rgb(0x2a2113).into(),
            btn,
            btn_hover,
            code_bg,
            syn_keyword: rgb(0xc792ea).into(),
            syn_string: rgb(0xa3e635).into(),
            syn_number: rgb(0xf78c6c).into(),
            syn_function: rgb(0x82aaff).into(),
            syn_type: rgb(0xffcb6b).into(),
            syn_comment: rgb(0x71717a).into(),
        }
    }

    fn light(preset: ThemePreset) -> Self {
        let (
            bg,
            stage,
            card,
            card_hover,
            surface_subtle,
            panel,
            accent,
            accent_hover,
            accent_bg,
            selected,
            bubble,
            btn,
            btn_hover,
            code_bg,
        ) = match preset {
            ThemePreset::WarmPaper => (
                rgb(0xf4f4f6).into(),
                rgb(0xf6f5f0).into(),
                rgb(0xffffff).into(),
                rgb(0xfaf9f5).into(),
                rgb(0xefede7).into(),
                rgb(0xf4f4f6).into(),
                rgb(0x18181b).into(),
                rgb(0x27272a).into(),
                rgb(0xeaeae6).into(),
                rgb(0xe8e7e2).into(),
                rgb(0xedece6).into(),
                rgb(0xedece7).into(),
                rgb(0xe3e2dc).into(),
                rgb(0xeae9e2).into(),
            ),
            ThemePreset::PureStudio => (
                rgb(0xf6f6f7).into(),
                rgb(0xffffff).into(),
                rgb(0xffffff).into(),
                rgb(0xf7f7f8).into(),
                rgb(0xf4f4f6).into(),
                rgb(0xf7f7f8).into(),
                rgb(0x111111).into(),
                rgb(0x27272a).into(),
                rgb(0xededef).into(),
                rgb(0xebebee).into(),
                rgb(0xf2f2f4).into(),
                rgb(0xefeff1).into(),
                rgb(0xe5e5e8).into(),
                rgb(0xf0f1f3).into(),
            ),
        };
        Self {
            chrome: Hsla { a: 0.80, ..bg },
            stage,
            surface_subtle,
            border_subtle: hsla(0., 0., 0.09, 0.08),
            specular: hsla(0., 0., 1.0, 0.90),
            shadow_ink: hsla(0., 0., 0.08, 0.07),
            accent_hover,
            accent_soft: hsla(0., 0., 0.09, 0.07),
            on_accent: rgb(0xffffff).into(),
            scrim: hsla(0., 0., 0.04, 0.22),

            bg,
            panel,
            border: hsla(0., 0., 0.09, 0.14),
            text: rgb(0x18181b).into(),
            dim: rgb(0x52525b).into(),
            faint: rgb(0x8a8a93).into(),
            accent,
            accent_bg,
            hover: hsla(0., 0., 0.09, 0.045),
            selected,
            ok: rgb(0x15803d).into(),
            warn: rgb(0xb45309).into(),
            bad: rgb(0xdc2626).into(),
            bubble,
            card,
            card_hover,
            warn_bg: rgb(0xfef7ec).into(),
            btn,
            btn_hover,
            code_bg,
            syn_keyword: rgb(0x9333ea).into(),
            syn_string: rgb(0x15803d).into(),
            syn_number: rgb(0xb45309).into(),
            syn_function: rgb(0x2563eb).into(),
            syn_type: rgb(0xa16207).into(),
            syn_comment: rgb(0x8c95a8).into(),
        }
    }
}

struct ThemeState {
    appearance: Appearance,
    preset: ThemePreset,
    palette: Palette,
}

impl Global for ThemeState {}

/// 启动初始化(须在 settings::init 与 gpui_kit::init 之后)。
pub fn init(cx: &mut App) {
    let appearance = Appearance::from(cx.window_appearance());
    let app_settings = &crate::settings::get(cx).appearance;
    let mode = app_settings.theme_mode;
    let preset = app_settings.theme_preset;
    cx.set_global(ThemeState {
        appearance,
        preset,
        palette: mode.resolve(appearance, preset),
    });
    sync_component_theme(cx);
}

/// 设置的 theme_mode 变化时重选色板并刷新窗口。
pub fn apply_mode(mode: ThemeMode, cx: &mut App) {
    let preset = crate::settings::get(cx).appearance.theme_preset;
    apply_appearance(mode, preset, cx);
}

/// 主题模式或风格预设变化时统一重算色板、同步组件库与动效偏好并刷新窗口。
pub fn apply_appearance(mode: ThemeMode, preset: ThemePreset, cx: &mut App) {
    let appearance = cx.global::<ThemeState>().appearance;
    let palette = mode.resolve(appearance, preset);
    let state = cx.global_mut::<ThemeState>();
    state.preset = preset;
    state.palette = palette;
    sync_component_theme(cx);
    cx.refresh_windows();
}

/// 系统外观变化:更新快照;设置模式为 System 时重选色板。
pub fn system_appearance_changed(appearance: WindowAppearance, cx: &mut App) {
    let app_settings = &crate::settings::get(cx).appearance;
    let mode = app_settings.theme_mode;
    let preset = app_settings.theme_preset;
    let state = cx.global_mut::<ThemeState>();
    state.appearance = appearance.into();
    state.preset = preset;
    if mode == ThemeMode::System {
        state.palette = Palette::for_appearance(state.appearance, preset);
    }
    sync_component_theme(cx);
    cx.refresh_windows();
}

/// 把本仓中性色板映射进 gpui-component 全局主题,并同步动效降级偏好。
pub fn sync_component_theme(cx: &mut App) {
    let appearance_settings = crate::settings::get(cx).appearance.clone();
    match appearance_settings.theme_mode {
        ThemeMode::System => gpui_kit::component::Theme::sync_system_appearance(None, cx),
        ThemeMode::Light => gpui_kit::component::Theme::change(
            gpui_kit::component::ThemeMode::Light,
            None,
            cx,
        ),
        ThemeMode::Dark => gpui_kit::component::Theme::change(
            gpui_kit::component::ThemeMode::Dark,
            None,
            cx,
        ),
    }
    gpui_kit::base::set_reduce_motion_preference(
        match appearance_settings.motion_level {
            crate::settings::MotionLevel::Off => gpui_kit::base::ReduceMotionPreference::Reduce,
            crate::settings::MotionLevel::Full | crate::settings::MotionLevel::Reduced => {
                gpui_kit::base::ReduceMotionPreference::System
            }
        },
        cx,
    );
    let p = *cx.colors();
    let shift = |c: Hsla, delta: f32| Hsla {
        l: (c.l + delta).clamp(0., 1.),
        ..c
    };
    let light_theme = p.bg.l > 0.5;
    let danger_step = if light_theme { -0.06 } else { 0.06 };
    gpui_kit::component::Theme::update(cx, |theme| {
        theme.radius = px_val(crate::kit::RADIUS_CONTROL);
        let c = &mut theme.colors;
        c.background = p.stage;
        c.foreground = p.text;
        c.border = p.border;
        c.input = p.border;
        c.ring = p.accent;
        c.popover = p.card;
        c.popover_foreground = p.text;
        c.muted = p.surface_subtle;
        c.muted_foreground = p.dim;
        c.accent = p.hover;
        c.accent_foreground = p.text;
        c.primary = p.accent;
        c.primary_foreground = p.on_accent;
        c.primary_hover = p.accent_hover;
        c.secondary = p.btn;
        c.secondary_foreground = p.text;
        c.danger = p.bad;
        c.danger_foreground = rgb(0xffffff).into();
        c.success = p.ok;
        c.warning = p.warn;
        c.link = p.text;
        c.selection = p.selected;
        c.switch = if light_theme {
            rgb(0xd4d4d8).into()
        } else {
            rgb(0x63636b).into()
        };
        c.switch_thumb = p.on_accent;
        c.title_bar = gpui_kit::transparent_black();
        c.title_bar_border = gpui_kit::transparent_black();
        c.sidebar = p.chrome;
        c.sidebar_border = p.border_subtle;
        c.tab_active = p.card;
        c.button_primary = p.accent;
        c.button_primary_foreground = p.on_accent;
        c.button_primary_hover = p.accent_hover;
        c.button_primary_active = shift(p.accent_hover, -0.04);
        c.button_secondary = p.btn;
        c.button_secondary_hover = p.btn_hover;
        c.button_secondary_foreground = p.text;
        c.button_danger = p.bad;
        c.button_danger_foreground = rgb(0xffffff).into();
        c.button_danger_hover = shift(p.bad, danger_step);
        c.button_danger_active = shift(p.bad, danger_step * 1.5);
    });
}

fn px_val(v: f32) -> gpui_kit::Pixels {
    gpui_kit::px(v)
}

pub trait ActiveTheme {
    fn colors(&self) -> &Palette;
    fn theme_mode(&self) -> ThemeMode;
    fn theme_preset(&self) -> ThemePreset;
}

impl ActiveTheme for App {
    fn colors(&self) -> &Palette {
        &self.global::<ThemeState>().palette
    }

    fn theme_mode(&self) -> ThemeMode {
        crate::settings::get(self).appearance.theme_mode
    }

    fn theme_preset(&self) -> ThemePreset {
        crate::settings::get(self).appearance.theme_preset
    }
}

/// 终端与只读 ANSI 执行输出面板的 16 色调色板与底前景色/光标/选区色。
#[derive(Clone, Copy)]
pub struct TerminalPalette {
    pub bg: Hsla,
    pub fg: Hsla,
    pub cursor: Hsla,
    pub cursor_text: Hsla,
    pub selection_bg: Hsla,
    pub ansi16: [u32; 16],
}

const ANSI16_DARK_VSCODE: [u32; 16] = [
    0x000000FF, 0xCD3131FF, 0x0DBC79FF, 0xE5E510FF, 0x2472C8FF, 0xBC3FBCFF, 0x11A8CDFF,
    0xE5E5E5FF, 0x666666FF, 0xF14C4CFF, 0x23D18BFF, 0xF5F543FF, 0x3B8EEAFF, 0xD670D6FF,
    0x29B8DBFF, 0xFFFFFFFF,
];

const ANSI16_LIGHT_READABLE: [u32; 16] = [
    0x1F2024FF, 0xC62828FF, 0x1E7E46FF, 0x9C6500FF, 0x1E5EB6FF, 0x8E3B9CFF, 0x0E7490FF,
    0x5A5D65FF, 0x71757EFF, 0xDC2626FF, 0x16A34AFF, 0xB45309FF, 0x2563EBFF, 0x9333EAFF,
    0x0891B2FF, 0x18191DFF,
];

const ANSI16_ONE_DARK: [u32; 16] = [
    0x282C34FF, 0xE06C75FF, 0x98C379FF, 0xE5C07BFF, 0x61AFEEFF, 0xC678DDFF, 0x56B6C2FF,
    0xABB2BFFF, 0x5C6370FF, 0xE06C75FF, 0x98C379FF, 0xD19A66FF, 0x61AFEEFF, 0xC678DDFF,
    0x56B6C2FF, 0xFFFFFFFF,
];

const ANSI16_TOKYO_NIGHT: [u32; 16] = [
    0x15161EFF, 0xF7768EFF, 0x9ECE6AFF, 0xE0AF68FF, 0x7AA2F7FF, 0xBB9AF7FF, 0x7DCFFFFF,
    0xA9B1D6FF, 0x414868FF, 0xF7768EFF, 0x9ECE6AFF, 0xE0AF68FF, 0x7AA2F7FF, 0xBB9AF7FF,
    0x7DCFFFFF, 0xC0CAF5FF,
];

pub fn terminal_palette(
    scheme: crate::settings::TerminalScheme,
    t: &Palette,
) -> TerminalPalette {
    use crate::settings::TerminalScheme;
    match scheme {
        TerminalScheme::Auto => {
            let dark = t.code_bg.l < 0.5;
            TerminalPalette {
                bg: t.code_bg,
                fg: t.text,
                cursor: t.accent,
                cursor_text: t.code_bg,
                selection_bg: if dark {
                    hsla(0.58, 0.70, 0.60, 0.28)
                } else {
                    hsla(0.58, 0.72, 0.50, 0.22)
                },
                ansi16: if dark {
                    ANSI16_DARK_VSCODE
                } else {
                    ANSI16_LIGHT_READABLE
                },
            }
        }
        TerminalScheme::Obsidian => TerminalPalette {
            bg: rgb(0x121417).into(),
            fg: rgb(0xE4E6EB).into(),
            cursor: rgb(0xE4E6EB).into(),
            cursor_text: rgb(0x121417).into(),
            selection_bg: hsla(0.58, 0.70, 0.60, 0.28),
            ansi16: ANSI16_DARK_VSCODE,
        },
        TerminalScheme::OneDark => TerminalPalette {
            bg: rgb(0x1E2127).into(),
            fg: rgb(0xABB2BF).into(),
            cursor: rgb(0x528BFF).into(),
            cursor_text: rgb(0x1E2127).into(),
            selection_bg: hsla(0.60, 0.65, 0.58, 0.30),
            ansi16: ANSI16_ONE_DARK,
        },
        TerminalScheme::TokyoNight => TerminalPalette {
            bg: rgb(0x1A1B26).into(),
            fg: rgb(0xC0CAF5).into(),
            cursor: rgb(0xC0CAF5).into(),
            cursor_text: rgb(0x1A1B26).into(),
            selection_bg: hsla(0.62, 0.65, 0.62, 0.28),
            ansi16: ANSI16_TOKYO_NIGHT,
        },
        TerminalScheme::WarmPaper => TerminalPalette {
            bg: rgb(0xF8F7F4).into(),
            fg: rgb(0x1F2024).into(),
            cursor: rgb(0x18181B).into(),
            cursor_text: rgb(0xF8F7F4).into(),
            selection_bg: hsla(0.58, 0.72, 0.50, 0.20),
            ansi16: ANSI16_LIGHT_READABLE,
        },
    }
}
