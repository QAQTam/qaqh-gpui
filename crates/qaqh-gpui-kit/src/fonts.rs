//! 内嵌大厂开源字体（SIL OFL 1.1）与跨平台 CJK Fallback 解析：
//! - UI 主字体：`Google Sans`（Regular / Medium / SemiBold / Bold 静态字重）
//! - 终端/代码字体：`Google Sans Code`（Regular / Bold / Italic 静态等宽字重）
//! - CJK 回退链在启动时按 `all_font_names()` 探测本机已安装中文字体，不把缺失字体写进 `FontFallbacks`。

use std::borrow::Cow;
use std::sync::OnceLock;

use gpui_kit::{App, FontFallbacks, SharedString};

pub const UI_FONT_FAMILY: &str = "Google Sans";
pub const MONO_FONT_FAMILY: &str = "Google Sans Code";

const UI_CJK_CANDIDATES: &[&str] = &[
    "PingFang SC",
    "Microsoft YaHei UI",
    "Microsoft YaHei",
    "Noto Sans CJK SC",
    "Source Han Sans SC",
    "WenQuanYi Micro Hei",
    "Sarasa UI SC",
    "Noto Sans",
];

const MONO_CJK_CANDIDATES: &[&str] = &[
    "Sarasa Mono SC",
    "Noto Sans Mono CJK SC",
    "Noto Sans Mono",
    "Noto Sans CJK SC",
    "Microsoft YaHei",
    "PingFang SC",
    "DejaVu Sans Mono",
    "Liberation Mono",
];

const BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/GoogleSans-Regular.ttf"),
    include_bytes!("../../../assets/fonts/GoogleSans-Medium.ttf"),
    include_bytes!("../../../assets/fonts/GoogleSans-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/GoogleSans-Bold.ttf"),
    include_bytes!("../../../assets/fonts/GoogleSansCode-Regular.ttf"),
    include_bytes!("../../../assets/fonts/GoogleSansCode-Bold.ttf"),
    include_bytes!("../../../assets/fonts/GoogleSansCode-Italic.ttf"),
];

static FONTS_REGISTERED: OnceLock<()> = OnceLock::new();
static UI_FALLBACKS: OnceLock<Option<FontFallbacks>> = OnceLock::new();
static MONO_FALLBACKS: OnceLock<Option<FontFallbacks>> = OnceLock::new();

/// 启动时一次性注册内嵌 Google Sans / Google Sans Code 并缓存本机 CJK Fallback 链。
pub fn init(cx: &mut App) {
    FONTS_REGISTERED.get_or_init(|| {
        let fonts: Vec<Cow<'static, [u8]>> = BUNDLED_FONTS
            .iter()
            .map(|bytes| Cow::Borrowed(*bytes))
            .collect();
        if let Err(err) = cx.text_system().add_fonts(fonts) {
            eprintln!("qaqh-gpui: 内嵌字体注册失败: {err}");
        }
        let installed = cx.text_system().all_font_names();
        let _ = UI_FALLBACKS.set(pick_fallbacks(UI_CJK_CANDIDATES, &installed));
        let _ = MONO_FALLBACKS.set(pick_fallbacks(MONO_CJK_CANDIDATES, &installed));
    });
}

pub fn ui_font_family() -> SharedString {
    SharedString::new_static(UI_FONT_FAMILY)
}

pub fn mono_font_family() -> SharedString {
    SharedString::new_static(MONO_FONT_FAMILY)
}

pub fn ui_font_fallbacks() -> Option<FontFallbacks> {
    UI_FALLBACKS.get().and_then(|v| v.clone())
}

pub fn mono_font_fallbacks() -> Option<FontFallbacks> {
    MONO_FALLBACKS.get().and_then(|v| v.clone())
}

fn pick_fallbacks(candidates: &[&str], installed: &[String]) -> Option<FontFallbacks> {
    let picked: Vec<String> = candidates
        .iter()
        .filter(|candidate| installed.iter().any(|name| name == *candidate))
        .map(|candidate| (*candidate).to_owned())
        .collect();
    (!picked.is_empty()).then(|| FontFallbacks::from_fonts(picked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_are_non_empty_ttf() {
        assert_eq!(BUNDLED_FONTS.len(), 7);
        for bytes in BUNDLED_FONTS {
            assert!(bytes.len() > 1024, "embedded font unexpectedly small");
        }
    }

    #[test]
    fn pick_fallbacks_keeps_priority_and_filters_missing() {
        let installed = vec![
            "Adwaita Sans".to_owned(),
            "Noto Sans Mono".to_owned(),
            "Noto Sans Mono CJK SC".to_owned(),
        ];
        let fb = pick_fallbacks(MONO_CJK_CANDIDATES, &installed).expect("should match installed");
        assert_eq!(
            fb.fallback_list(),
            &["Noto Sans Mono CJK SC".to_owned(), "Noto Sans Mono".to_owned()]
        );
        assert!(pick_fallbacks(&["NonExistentFont"], &installed).is_none());
    }
}
