//! 根渲染与弹簧可收折侧栏(DESIGN LAW v3.0 · v5.0.0):
//! - L0 半透明 chrome 外框(透出 Win11 Mica)+ 顶部 42px TitleBar;
//! - 侧栏/工作区分组均由 gpui-base 物理弹簧与 MotionReveal 驱动;
//! - 设置弹窗以 Presence 居中浮层呈现,不再双侧栏套娃。

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Context, FontWeight, KeyDownEvent,
    MouseButton, SharedString, Window, div, px, relative,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{
    RADIUS_CONTROL, RADIUS_POPOVER, kbd, shadow_card, shadow_popover, soft, stage_surface,
};
use qaqh_gpui_kit::motion::{
    accordion_reveal, effective_motion, page_enter, presence_overlay, spring_chevron,
    spring_f32, spring_panel_policy,
};
use qaqh_gpui_kit::primitives::{icon_button, nav_pill};
use qaqh_gpui_kit::settings;
use qaqh_gpui_kit::settings::MotionLevel;
use qaqh_gpui_kit::theme::{ActiveTheme, Palette};
use qaqh_gpui_session::store::SettingsEditor;
use qaqh_gpui_session::store::WorkspaceGroup;
use qaqh_gpui_wire::bridge::BridgeRequest;
use qaqh_client::{CommandTarget, RingingCommand};

use crate::daemon::{DaemonStore, PrimarySection, clean_session_title};
use crate::pages;
use crate::views::{inspector_dock, settings_page, sidebar_row};

impl Render for DaemonStore {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.colors();
        let settings = settings::get(cx).clone();
        window.set_rem_size(px(settings.typography.ui_font_size));

        let query = self.session_search.read(cx).value().to_lowercase();
        let visible: Vec<_> = self
            .sessions
            .iter()
            .filter(|row| {
                row.archived == self.show_archived
                    && (query.is_empty()
                        || row.title.to_lowercase().contains(&query)
                        || row.id.contains(&query))
            })
            .cloned()
            .collect();
        let groups: Vec<WorkspaceGroup> = qaqh_gpui_session::store::groups(&visible, &self.workspaces);
        let daemon_entity = cx.entity();
        let active_id = self
            .current
            .as_ref()
            .map(|current| current.read(cx).session_id.clone());
        let active_tone = self
            .current
            .as_ref()
            .and_then(|current| current.read(cx).last_turn_tone());
        let animate_motion = effective_motion(cx) != MotionLevel::Off;
        let mut rows: Vec<AnyElement> = Vec::new();
        for group in &groups {
            let key = group.id.clone().unwrap_or_else(|| "ungrouped".into());
            let collapsed = self.collapsed.contains(&key);
            let title = key.clone();
            let was_collapsed = collapsed;
            let group_id = group.id.clone();

            let move_in = group_id.as_ref().map(|workspace_id| {
                let workspace_id = workspace_id.clone();
                div()
                    .id(SharedString::from(format!("move-in-{key}")))
                    .px_1()
                    .cursor_pointer()
                    .opacity(0.0)
                    .group_hover("grp", |style| style.opacity(1.0))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("将当前会话移入此工作区")
                            .build(window, cx)
                    })
                    .on_click(cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(current) = daemon.current.clone() {
                            let workspace_id = workspace_id.clone();
                            current.update(cx, |store, _| store.move_to_workspace(workspace_id));
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(IconName::FolderInput)
                            .with_size(Size::XSmall)
                            .text_color(t.accent),
                    )
            });

            let new_in = group_id.as_ref().map(|workspace_id| {
                let workspace_id = workspace_id.clone();
                div()
                    .id(SharedString::from(format!("new-in-{key}")))
                    .px_1()
                    .cursor_pointer()
                    .opacity(0.0)
                    .group_hover("grp", |style| style.opacity(1.0))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("在此工作区新建对话")
                            .build(window, cx)
                    })
                    .on_click(cx.listener(move |daemon: &mut DaemonStore, _, window, cx| {
                        cx.stop_propagation();
                        let index = daemon
                            .workspaces
                            .iter()
                            .position(|entry| entry.id == workspace_id);
                        daemon.open_startup(index, window, cx);
                    }))
                    .child(
                        Icon::new(IconName::FolderPlus)
                            .with_size(Size::XSmall)
                            .text_color(t.accent),
                    )
            });

            rows.push(
                div()
                    .id(SharedString::from(format!("grp-{key}")))
                    .group("grp")
                    .px_2()
                    .pt_2p5()
                    .pb_1()
                    .mx_0p5()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(RADIUS_CONTROL))
                    .cursor_pointer()
                    .hover(|style| style.bg(t.hover))
                    .on_click(cx.listener(
                        move |daemon: &mut DaemonStore, _, _, cx: &mut Context<DaemonStore>| {
                            if was_collapsed {
                                daemon.collapsed.remove(&title);
                            } else {
                                daemon.collapsed.insert(title.clone());
                            }
                            cx.notify();
                        },
                    ))
                    .child(spring_chevron(
                        SharedString::from(format!("ws-chev-{key}")),
                        !collapsed,
                        t.faint,
                        animate_motion,
                    ))
                    .child(
                        Icon::new(if collapsed {
                            IconName::Folder
                        } else {
                            IconName::FolderOpen
                        })
                        .with_size(Size::XSmall)
                        .text_color(t.dim),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .truncate()
                            .child(group.title.clone()),
                    )
                    .children(move_in)
                    .children(new_in)
                    .into_any_element(),
            );
            let group_rows = group.rows.clone();
            let group_titles: Vec<SharedString> = group_rows
                .iter()
                .map(|row| clean_session_title(row, &self.workspaces))
                .collect();
            let rename_draft = self.rename_draft.clone();
            let active_id_for_grp = active_id.clone();
            let daemon_for_grp = daemon_entity.clone();
            let reveal_key = SharedString::from(format!("ws-grp-{key}"));
            if let Some(panel) = accordion_reveal(reveal_key, !collapsed, window, cx, |_cx| {
                let mut list = div().flex().flex_col().gap_0p5();
                for (row, display_title) in group_rows.iter().zip(group_titles.iter()) {
                    if rename_draft
                        .as_ref()
                        .is_some_and(|(rid, _)| rid == &row.id)
                    {
                        let draft = rename_draft.as_ref().unwrap().1.clone();
                        let daemon_esc = daemon_for_grp.clone();
                        list = list.child(
                            div()
                                .id(SharedString::from(format!("sess-rename-{}", row.id)))
                                .mx_1()
                                .my_0p5()
                                .px_2()
                                .py_1()
                                .rounded(px(RADIUS_CONTROL))
                                .bg(t.card)
                                .border_1()
                                .border_color(t.accent)
                                .on_key_down(move |event: &KeyDownEvent, _, cx| {
                                    if event.keystroke.key == "escape" {
                                        let _ = daemon_esc.update(cx, |daemon, cx| {
                                            daemon.rename_draft = None;
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }
                                })
                                .child(Input::new(&draft).small()),
                        );
                        continue;
                    }
                    let selected = active_id_for_grp.as_deref() == Some(row.id.as_str());
                    let id = row.id.clone();
                    let archived = row.archived;
                    let daemon_click = daemon_for_grp.clone();
                    let listener =
                        move |event: &ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                            let _ = daemon_click.update(cx, |daemon, cx| {
                                if event.is_secondary() {
                                    let position = event.mouse_position().unwrap_or_default();
                                    let size = window.viewport_size();
                                    daemon.ctx_menu = Some(crate::daemon::SessionCtxMenu {
                                        session_id: id.clone(),
                                        archived,
                                        position: gpui_kit::point(
                                            position.x.min(size.width - px(180.)).max(px(0.)),
                                            position.y.min(size.height - px(120.)).max(px(0.)),
                                        ),
                                        confirm_delete: false,
                                    });
                                } else {
                                    daemon.select(id.clone(), window, cx);
                                }
                                cx.notify();
                            });
                        };
                    let tone = if selected { active_tone } else { None };
                    list = list.child(sidebar_row(
                        row,
                        display_title.as_ref(),
                        selected,
                        tone,
                        &t,
                        animate_motion,
                        listener,
                    ));
                }
                list.into_any_element()
            }) {
                rows.push(panel);
            }
        }

        let base_width = self.workbench.sidebar_width.unwrap_or(264.);
        let full_width = base_width + 8.0;
        let target_width = if self.sidebar_open { full_width } else { 0.0 };

        let new_chat_card = div()
            .id("sidebar-new-session")
            .px_3()
            .py_2()
            .rounded(px(RADIUS_CONTROL))
            .bg(t.card)
            .border_1()
            .border_color(if self.section == PrimarySection::Home && self.new_session_open {
                t.accent
            } else {
                t.border_subtle
            })
            .shadow(shadow_card(&t))
            .flex()
            .items_center()
            .justify_between()
            .cursor_pointer()
            .hover(|style| style.bg(t.card_hover))
            .on_click(cx.listener(|daemon: &mut DaemonStore, _, window, cx| {
                daemon.open_startup(None, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::SquarePen)
                            .with_size(Size::XSmall)
                            .text_color(t.accent),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child("新建对话"),
                    ),
            )
            .child(kbd("Ctrl+N", &t));

        let skills_badge = self.skills_report.as_ref().and_then(|report| {
            let count = report.project.len() + report.user.len();
            (count > 0).then(|| SharedString::from(count.to_string()))
        });
        let primary_nav = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .pb_1p5()
            .border_b_1()
            .border_color(t.border_subtle)
            .child(nav_pill(
                "sb-nav-home",
                IconName::House,
                "主页",
                None,
                !self.settings_open && self.section == PrimarySection::Home,
                &t,
                cx.listener(|daemon, _, window, cx| {
                    daemon.navigate(PrimarySection::Home, window, cx);
                }),
            ))
            .child(nav_pill(
                "sb-nav-tools",
                IconName::Wrench,
                "工具",
                skills_badge,
                !self.settings_open && self.section == PrimarySection::Tools,
                &t,
                cx.listener(|daemon, _, window, cx| {
                    daemon.navigate(PrimarySection::Tools, window, cx);
                }),
            ))
            .child(nav_pill(
                "sb-nav-team",
                IconName::Users,
                "团队",
                None,
                !self.settings_open && self.section == PrimarySection::Team,
                &t,
                cx.listener(|daemon, _, window, cx| {
                    daemon.navigate(PrimarySection::Team, window, cx);
                }),
            ))
            .child(nav_pill(
                "sb-nav-access",
                IconName::KeyRound,
                "接入",
                None,
                !self.settings_open && self.section == PrimarySection::Access,
                &t,
                cx.listener(|daemon, _, window, cx| {
                    daemon.navigate(PrimarySection::Access, window, cx);
                }),
            ));

        let search_bar = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "escape") {
                            window.blur(cx);
                            cx.notify();
                        }
                    }))
                    .child(Input::new(&self.session_search).small()),
            )
            .child(icon_button(
                cx,
                "sidebar-archive-toggle",
                IconName::Archive,
                if self.show_archived { t.accent } else { t.dim },
                if self.show_archived {
                    "正在查看已归档会话（点击切回活跃）"
                } else {
                    "查看已归档会话"
                },
                cx.listener(|daemon, _, _, cx| {
                    daemon.show_archived = !daemon.show_archived;
                    cx.notify();
                }),
                &t,
            ));

        let utility_footer = div()
            .pt_2()
            .border_t_1()
            .border_color(t.border_subtle)
            .flex()
            .flex_col()
            .gap_1()
            .child(nav_pill(
                "sb-nav-settings",
                IconName::Settings,
                "设置",
                None,
                self.settings_open,
                &t,
                cx.listener(|daemon, _, window, cx| {
                    daemon.open_settings(SettingsEditor::Local, window, cx);
                }),
            ));

        let sidebar_inner = div()
            .w(px(full_width))
            .h_full()
            .pr_2()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(new_chat_card)
            .child(primary_nav)
            .child(search_bar)
            .child(
                div()
                    .id("session-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .children(rows),
            )
            .child(utility_footer);

        let animated_width = spring_f32(
            "sidebar-width",
            target_width,
            spring_panel_policy(),
            window,
            cx,
        )
        .max(0.0);
        let sidebar_ratio = (animated_width / full_width).clamp(0.0, 1.0);
        let sidebar_shell = if !self.sidebar_open && animated_width <= 0.5 {
            div().w(px(0.)).h_full().flex_shrink_0()
        } else {
            div()
                .h_full()
                .w(px(animated_width))
                .opacity(sidebar_ratio)
                .flex_shrink_0()
                .overflow_hidden()
                .child(sidebar_inner)
        };

        let raw_main: AnyElement = match self.section {
            PrimarySection::Tools => stage_surface(&t)
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(pages::tools_page(self, window, cx, &t))
                .into_any_element(),
            PrimarySection::Team => stage_surface(&t)
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(pages::team_page(self, window, cx, &t))
                .into_any_element(),
            PrimarySection::Access => stage_surface(&t)
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(pages::access_page(self, window, cx, &t))
                .into_any_element(),
            PrimarySection::Home => {
                if self.new_session_open || self.current.is_none() {
                    stage_surface(&t)
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(pages::startup_page(self, window, cx, &t))
                        .into_any_element()
                } else {
                    let session = self.current.clone().expect("checked is_some above");
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .child(session)
                        .children(inspector_dock(self, window, cx))
                        .into_any_element()
                }
            }
        };
        let main = page_enter(
            format!("main-page-{}", self.page_epoch),
            raw_main,
            window,
            cx,
        );

        let family = settings.typography.ui_font_family;
        let main_area = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().flex().child(main))
            .children(crate::shell_panel::dock(self, window, cx));

        let settings_sample = presence_overlay("settings-modal", self.settings_open, 200, window, cx);
        let settings_modal = settings_sample.should_render().then(|| {
            let p = settings_sample.progress.clamp(0.0, 1.0);
            div()
                .id("settings-modal-backdrop")
                .absolute()
                .inset_0()
                .bg(soft(t.scrim, t.scrim.a * p))
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|daemon, _, _, cx| {
                    daemon.settings_open = false;
                    daemon.settings_form = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("settings-modal-card")
                        .w(relative(0.86))
                        .max_w(px(940.))
                        .h(relative(0.84))
                        .max_h(px(700.))
                        .mt(px((1.0 - p) * 14.0))
                        .opacity(p)
                        .rounded(px(18.))
                        .bg(t.card)
                        .border_1()
                        .border_color(t.border)
                        .shadow(shadow_popover(&t))
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(settings_page(self, cx)),
                )
        });

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(t.chrome)
            .text_color(t.text)
            .when(!family.is_empty(), |root| {
                root.font_family(SharedString::from(family))
            })
            .on_key_down(cx.listener(|daemon, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if daemon.ctx_menu.is_some() {
                        daemon.ctx_menu = None;
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if daemon.settings_open {
                        daemon.settings_open = false;
                        daemon.settings_form = None;
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if daemon.new_session_open {
                        daemon.new_session_open = false;
                        daemon.settings_form = None;
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                }
                if !event.keystroke.modifiers.control {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "b" => {
                        daemon.sidebar_open = !daemon.sidebar_open;
                    }
                    "n" => {
                        daemon.open_startup(None, window, cx);
                    }
                    "k" => {
                        daemon.sidebar_open = true;
                        daemon
                            .session_search
                            .update(cx, |field, cx| field.focus(window, cx));
                    }
                    "," => {
                        daemon.open_settings(SettingsEditor::Local, window, cx);
                    }
                    "f" => {
                        if let Some(current) = &daemon.current {
                            current.update(cx, |store, cx| {
                                store.search_open = true;
                                store.search.update(cx, |field, cx| field.focus(window, cx));
                                cx.notify();
                            });
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(crate::titlebar::render(self, cx, &t))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .px_2()
                    .pb_2()
                    .child(sidebar_shell)
                    .child(main_area),
            )
            .children(settings_modal)
            .children(
                self.ctx_menu
                    .as_ref()
                    .map(|menu| ctx_menu_element(menu, cx, &t, animate_motion)),
            )
    }
}

/// 会话右键菜单:圆角悬浮卡(含轻柔入场浮现动画)。
fn ctx_menu_element(
    menu: &crate::daemon::SessionCtxMenu,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    animate_motion: bool,
) -> AnyElement {
    let session_id = menu.session_id.clone();
    let archived = menu.archived;
    let confirm_delete = menu.confirm_delete;

    let rename_item = ctx_item(
        "ctx-rename",
        IconName::Pencil,
        "重命名".into(),
        t.text,
        t,
        cx.listener({
            let session_id = session_id.clone();
            move |daemon: &mut DaemonStore, _, window: &mut Window, cx| {
                let Some(row) = daemon.sessions.iter().find(|row| row.id == session_id) else {
                    return;
                };
                let title = if row.title.is_empty() {
                    String::new()
                } else {
                    row.title.clone()
                };
                let state = cx.new(|cx| InputState::new(window, cx).placeholder("输入新标题…"));
                state.update(cx, |field, cx| field.set_value(&title, window, cx));
                let emitter = state.clone();
                let sid = session_id.clone();
                cx.subscribe(&state, move |daemon: &mut DaemonStore, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                        let title = emitter.read(cx).value().trim().to_string();
                        if !title.is_empty() {
                            let _ = daemon.requests.unbounded_send(BridgeRequest::RenameSession {
                                seed: sid.clone(),
                                title,
                            });
                        }
                        daemon.rename_draft = None;
                        cx.notify();
                    }
                })
                .detach();
                daemon.rename_draft = Some((session_id.clone(), state));
                daemon.ctx_menu = None;
                cx.notify();
            }
        }),
    );
    let archive_item = ctx_item(
        "ctx-archive",
        if archived {
            IconName::ArchiveRestore
        } else {
            IconName::Archive
        },
        if archived { "取消归档".into() } else { "归档".into() },
        t.text,
        t,
        cx.listener({
            let session_id = session_id.clone();
            move |daemon: &mut DaemonStore, _, _, cx| {
                let _ = daemon.requests.unbounded_send(BridgeRequest::Command {
                    target: CommandTarget::Session {
                        session_id: session_id.clone(),
                    },
                    command: if archived {
                        RingingCommand::SessionUnarchive {}
                    } else {
                        RingingCommand::SessionArchive {}
                    },
                });
                daemon.ctx_menu = None;
                cx.notify();
            }
        }),
    );
    let delete_item = ctx_item(
        "ctx-delete",
        if confirm_delete {
            IconName::TriangleAlert
        } else {
            IconName::Trash
        },
        if confirm_delete {
            "确认删除".into()
        } else {
            "删除".into()
        },
        if confirm_delete { t.bad } else { t.text },
        t,
        cx.listener({
            let session_id = session_id.clone();
            move |daemon: &mut DaemonStore, _, _, cx| {
                if let Some(menu) = daemon.ctx_menu.as_mut()
                    && menu.session_id == session_id
                    && !menu.confirm_delete
                {
                    menu.confirm_delete = true;
                    cx.notify();
                    return;
                }
                let _ = daemon.requests.unbounded_send(BridgeRequest::DeleteSession {
                    seed: session_id.clone(),
                });
                if daemon
                    .rename_draft
                    .as_ref()
                    .is_some_and(|(rid, _)| rid == &session_id)
                {
                    daemon.rename_draft = None;
                }
                daemon.ctx_menu = None;
                cx.notify();
            }
        }),
    );

    let card = div()
        .absolute()
        .left(menu.position.x)
        .top(menu.position.y)
        .w(px(168.))
        .flex()
        .flex_col()
        .gap_0p5()
        .rounded(px(RADIUS_POPOVER))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .p_1p5()
        .shadow(shadow_popover(t))
        .children([rename_item, archive_item, delete_item]);

    let card_el: AnyElement = if animate_motion {
        card.with_animation(
            SharedString::from(format!("ctx-enter-{}", menu.session_id)),
            Animation::new(Duration::from_millis(120))
                .with_easing(gpui_kit::base::animation::ease_out_cubic),
            |el, v| el.opacity(v).mt(px((1.0 - v) * 4.0)),
        )
        .into_any_element()
    } else {
        card.into_any_element()
    };

    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .id("ctx-menu-backdrop")
                .absolute()
                .inset_0()
                .on_click(cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                    daemon.ctx_menu = None;
                    cx.notify();
                })),
        )
        .child(card_el)
        .into_any_element()
}

fn ctx_item(
    id: &'static str,
    icon: IconName,
    label: SharedString,
    tone: gpui_kit::Hsla,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2p5()
        .py_1p5()
        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CHIP))
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(tone)
        .cursor_pointer()
        .hover({
            let hover = t.hover;
            move |style| style.bg(hover)
        })
        .child(Icon::new(icon).with_size(Size::XSmall).text_color(tone))
        .child(label)
        .on_click(listener)
        .into_any_element()
}
