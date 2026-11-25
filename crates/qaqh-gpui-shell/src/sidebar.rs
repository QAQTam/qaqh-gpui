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

            let is_renaming_ws = group_id.as_ref().is_some_and(|wid| {
                self.rename_draft
                    .as_ref()
                    .is_some_and(|(target, _)| *target == crate::daemon::RenameTarget::Workspace(wid.clone()))
            });
            if is_renaming_ws {
                let draft = self.rename_draft.as_ref().unwrap().1.clone();
                let daemon_esc = daemon_entity.clone();
                rows.push(
                    div()
                        .id(SharedString::from(format!("ws-rename-{key}")))
                        .flex_shrink_0()
                        .mx_1()
                        .mt_2()
                        .mb_0p5()
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
                        .child(Input::new(&draft).small())
                        .into_any_element(),
                );
            } else {
                let group_id_for_ctx = group_id.clone();
                let group_title_for_ctx = group.title.clone();
                let group_session_ids: Vec<String> =
                    group.rows.iter().map(|row| row.id.clone()).collect();
                rows.push(
                    div()
                        .id(SharedString::from(format!("grp-{key}")))
                        .group("grp")
                        .flex_shrink_0()
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
                            move |daemon: &mut DaemonStore,
                                  event: &ClickEvent,
                                  window: &mut Window,
                                  cx: &mut Context<DaemonStore>| {
                                if event.is_secondary() {
                                    if let Some(workspace_id) = group_id_for_ctx.clone() {
                                        let position = event.mouse_position().unwrap_or_default();
                                        let size = window.viewport_size();
                                        daemon.ctx_menu = Some(crate::daemon::SidebarCtxMenu {
                                            target: crate::daemon::SidebarCtxTarget::Workspace {
                                                workspace_id,
                                                title: group_title_for_ctx.clone(),
                                                archived_view: daemon.show_archived,
                                                session_ids: group_session_ids.clone(),
                                            },
                                            position: gpui_kit::point(
                                                position.x.min(size.width - px(180.)).max(px(0.)),
                                                position.y.min(size.height - px(120.)).max(px(0.)),
                                            ),
                                            confirm_delete: false,
                                        });
                                        cx.notify();
                                    }
                                    return;
                                }
                                if was_collapsed {
                                    daemon.collapsed.remove(&title);
                                } else {
                                    daemon.collapsed.insert(title.clone());
                                    daemon.expanded_more.remove(&title);
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
            }
            const WORKSPACE_PREVIEW_LIMIT: usize = 5;
            let group_rows = group.rows.clone();
            let group_titles: Vec<SharedString> = group_rows
                .iter()
                .map(|row| clean_session_title(row, &self.workspaces))
                .collect();
            let group_parents: Vec<Option<SharedString>> = group_rows
                .iter()
                .map(|row| {
                    row.handoff_from_session_id.as_ref().and_then(|parent_id| {
                        self.sessions
                            .iter()
                            .find(|item| &item.id == parent_id)
                            .map(|parent| clean_session_title(parent, &self.workspaces))
                    })
                })
                .collect();
            let rename_draft = self.rename_draft.clone();
            let active_id_for_grp = active_id.clone();
            let daemon_for_grp = daemon_entity.clone();
            let anchors_for_grp = self.handoff_anchors.clone();
            let has_more = query.is_empty() && group_rows.len() > WORKSPACE_PREVIEW_LIMIT;
            let show_all = !has_more || self.expanded_more.contains(&key);
            let primary_len = if has_more {
                WORKSPACE_PREVIEW_LIMIT
            } else {
                group_rows.len()
            };
            let extra_count = group_rows.len().saturating_sub(primary_len);
            let extra_panel = has_more
                .then(|| {
                    accordion_reveal(
                        SharedString::from(format!("ws-more-{key}")),
                        !collapsed && show_all,
                        window,
                        cx,
                        |_cx| {
                            render_session_rows(
                                &group_rows[primary_len..],
                                &group_titles[primary_len..],
                                &group_parents[primary_len..],
                                &rename_draft,
                                active_id_for_grp.as_deref(),
                                active_tone,
                                &daemon_for_grp,
                                &anchors_for_grp,
                                &t,
                                animate_motion,
                            )
                            .into_any_element()
                        },
                    )
                })
                .flatten();
            let reveal_key = SharedString::from(format!("ws-grp-{key}"));
            if let Some(panel) = accordion_reveal(reveal_key, !collapsed, window, cx, |_cx| {
                let mut list = render_session_rows(
                    &group_rows[..primary_len],
                    &group_titles[..primary_len],
                    &group_parents[..primary_len],
                    &rename_draft,
                    active_id_for_grp.as_deref(),
                    active_tone,
                    &daemon_for_grp,
                    &anchors_for_grp,
                    &t,
                    animate_motion,
                );
                if let Some(extra) = extra_panel {
                    list = list.child(extra);
                }
                if has_more {
                    let more_key = key.clone();
                    let daemon_more = daemon_for_grp.clone();
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("ws-more-btn-{key}")))
                            .mx_0p5()
                            .px_2p5()
                            .py_1()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_xs()
                            .text_color(t.dim)
                            .hover(|style| style.bg(t.hover).text_color(t.text))
                            .on_click(move |_, _, cx| {
                                let _ = daemon_more.update(cx, |daemon, cx| {
                                    if !daemon.expanded_more.remove(&more_key) {
                                        daemon.expanded_more.insert(more_key.clone());
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                Icon::new(if show_all {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .with_size(Size::XSmall)
                                .text_color(t.faint),
                            )
                            .child(if show_all {
                                SharedString::from("收起多余会话")
                            } else {
                                SharedString::from(format!("显示更多（余 {extra_count} 条）"))
                            }),
                    );
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
            .child({
                let anchors = self.handoff_anchors.clone();
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
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
                    .child(qaqh_gpui_session::handoff::anchor_probe(move |bounds| {
                        anchors.record_sidebar_list(bounds);
                    }))
            })
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
                .when(self.settings_open, |el| el.occlude())
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
            .children(self.fly_box.as_ref().map(|fly| {
                qaqh_gpui_session::handoff::fly_box_overlay(
                    fly.from,
                    fly.target_session_id.clone(),
                    fly.started_at,
                    self.handoff_anchors.clone(),
                    t,
                )
            }))
    }
}

/// 侧栏右键菜单(会话/工作区统一圆角悬浮卡,含轻柔入场浮现动画)。
fn ctx_menu_element(
    menu: &crate::daemon::SidebarCtxMenu,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    animate_motion: bool,
) -> AnyElement {
    let confirm_delete = menu.confirm_delete;
    let (anim_key, items): (String, Vec<AnyElement>) = match &menu.target {
        crate::daemon::SidebarCtxTarget::Session {
            session_id,
            archived,
        } => {
            let session_id = session_id.clone();
            let archived = *archived;
            let rename_item = ctx_item(
                "ctx-rename",
                IconName::Pencil,
                "重命名".into(),
                t.text,
                t,
                cx.listener({
                    let session_id = session_id.clone();
                    move |daemon: &mut DaemonStore, _, window: &mut Window, cx| {
                        let Some(row) = daemon.sessions.iter().find(|row| row.id == session_id)
                        else {
                            return;
                        };
                        let title = if row.title.is_empty() {
                            String::new()
                        } else {
                            row.title.clone()
                        };
                        let state =
                            cx.new(|cx| InputState::new(window, cx).placeholder("输入新标题…"));
                        state.update(cx, |field, cx| field.set_value(&title, window, cx));
                        let emitter = state.clone();
                        let sid = session_id.clone();
                        cx.subscribe(
                            &state,
                            move |daemon: &mut DaemonStore, _, event: &InputEvent, cx| {
                                if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                                    let title = emitter.read(cx).value().trim().to_string();
                                    if !title.is_empty() {
                                        let _ = daemon.requests.unbounded_send(
                                            BridgeRequest::RenameSession {
                                                seed: sid.clone(),
                                                title,
                                            },
                                        );
                                    }
                                    daemon.rename_draft = None;
                                    cx.notify();
                                }
                            },
                        )
                        .detach();
                        daemon.rename_draft =
                            Some((crate::daemon::RenameTarget::Session(session_id.clone()), state));
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
                if archived {
                    "取消归档".into()
                } else {
                    "归档".into()
                },
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
                            && matches!(
                                &menu.target,
                                crate::daemon::SidebarCtxTarget::Session { session_id: id, .. }
                                if id == &session_id
                            )
                            && !menu.confirm_delete
                        {
                            menu.confirm_delete = true;
                            cx.notify();
                            return;
                        }
                        let _ = daemon.requests.unbounded_send(BridgeRequest::DeleteSession {
                            seed: session_id.clone(),
                        });
                        if daemon.rename_draft.as_ref().is_some_and(|(target, _)| {
                            *target == crate::daemon::RenameTarget::Session(session_id.clone())
                        }) {
                            daemon.rename_draft = None;
                        }
                        daemon.ctx_menu = None;
                        cx.notify();
                    }
                }),
            );
            (
                format!("sess-{session_id}"),
                vec![rename_item, archive_item, delete_item],
            )
        }
        crate::daemon::SidebarCtxTarget::Workspace {
            workspace_id,
            title,
            archived_view,
            session_ids,
        } => {
            let workspace_id = workspace_id.clone();
            let current_title = title.clone();
            let archived_view = *archived_view;
            let session_ids = session_ids.clone();
            let has_sessions = !session_ids.is_empty();

            let rename_item = ctx_item(
                "ctx-ws-rename",
                IconName::Pencil,
                "重命名".into(),
                t.text,
                t,
                cx.listener({
                    let workspace_id = workspace_id.clone();
                    let current_title = current_title.clone();
                    move |daemon: &mut DaemonStore, _, window: &mut Window, cx| {
                        let state = cx
                            .new(|cx| InputState::new(window, cx).placeholder("输入工作区名称…"));
                        state.update(cx, |field, cx| field.set_value(&current_title, window, cx));
                        let emitter = state.clone();
                        let wid = workspace_id.clone();
                        cx.subscribe(
                            &state,
                            move |daemon: &mut DaemonStore, _, event: &InputEvent, cx| {
                                if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                                    let new_title = emitter.read(cx).value().trim().to_string();
                                    if !new_title.is_empty() {
                                        let _ = daemon.requests.unbounded_send(
                                            BridgeRequest::Command {
                                                target: CommandTarget::Workspace {
                                                    workspace_id: wid.clone(),
                                                },
                                                command: RingingCommand::WorkspaceRename {
                                                    title: new_title,
                                                },
                                            },
                                        );
                                    }
                                    daemon.rename_draft = None;
                                    cx.notify();
                                }
                            },
                        )
                        .detach();
                        daemon.rename_draft = Some((
                            crate::daemon::RenameTarget::Workspace(workspace_id.clone()),
                            state,
                        ));
                        daemon.ctx_menu = None;
                        cx.notify();
                    }
                }),
            );

            let archive_label: SharedString = if archived_view {
                "取消归档".into()
            } else {
                "归档".into()
            };
            let archive_item = ctx_item(
                "ctx-ws-archive",
                if archived_view {
                    IconName::ArchiveRestore
                } else {
                    IconName::Archive
                },
                archive_label,
                if has_sessions { t.text } else { t.faint },
                t,
                cx.listener({
                    let session_ids = session_ids.clone();
                    move |daemon: &mut DaemonStore, _, _, cx| {
                        if !has_sessions {
                            return;
                        }
                        for sid in &session_ids {
                            let _ = daemon.requests.unbounded_send(BridgeRequest::Command {
                                target: CommandTarget::Session {
                                    session_id: sid.clone(),
                                },
                                command: if archived_view {
                                    RingingCommand::SessionUnarchive {}
                                } else {
                                    RingingCommand::SessionArchive {}
                                },
                            });
                        }
                        daemon.ctx_menu = None;
                        cx.notify();
                    }
                }),
            );

            let delete_item = ctx_item(
                "ctx-ws-delete",
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
                    let workspace_id = workspace_id.clone();
                    move |daemon: &mut DaemonStore, _, _, cx| {
                        if let Some(menu) = daemon.ctx_menu.as_mut()
                            && matches!(
                                &menu.target,
                                crate::daemon::SidebarCtxTarget::Workspace { workspace_id: id, .. }
                                if id == &workspace_id
                            )
                            && !menu.confirm_delete
                        {
                            menu.confirm_delete = true;
                            cx.notify();
                            return;
                        }
                        let _ = daemon.requests.unbounded_send(BridgeRequest::Command {
                            target: CommandTarget::Workspace {
                                workspace_id: workspace_id.clone(),
                            },
                            command: RingingCommand::WorkspaceDelete {},
                        });
                        if daemon.rename_draft.as_ref().is_some_and(|(target, _)| {
                            *target == crate::daemon::RenameTarget::Workspace(workspace_id.clone())
                        }) {
                            daemon.rename_draft = None;
                        }
                        daemon.collapsed.remove(&workspace_id);
                        daemon.expanded_more.remove(&workspace_id);
                        daemon.ctx_menu = None;
                        cx.notify();
                    }
                }),
            );

            (
                format!("ws-{workspace_id}"),
                vec![rename_item, archive_item, delete_item],
            )
        }
    };

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
        .children(items);

    let card_el: AnyElement = if animate_motion {
        card.with_animation(
            SharedString::from(format!("ctx-enter-{anim_key}")),
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
                .occlude()
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

#[allow(clippy::too_many_arguments)]
fn render_session_rows(
    rows: &[qaqh_gpui_session::store::SessionRow],
    titles: &[SharedString],
    parents: &[Option<SharedString>],
    rename_draft: &Option<(crate::daemon::RenameTarget, gpui_kit::Entity<InputState>)>,
    active_id: Option<&str>,
    active_tone: Option<bool>,
    daemon_entity: &gpui_kit::Entity<DaemonStore>,
    anchors: &qaqh_gpui_session::handoff::HandoffAnchors,
    t: &Palette,
    animate_motion: bool,
) -> gpui_kit::Div {
    let mut list = div().flex().flex_col().gap_0p5();
    for ((row, display_title), parent_title) in rows.iter().zip(titles.iter()).zip(parents.iter()) {
        if rename_draft.as_ref().is_some_and(|(target, _)| {
            *target == crate::daemon::RenameTarget::Session(row.id.clone())
        }) {
            let draft = rename_draft.as_ref().unwrap().1.clone();
            let daemon_esc = daemon_entity.clone();
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
        let selected = active_id == Some(row.id.as_str());
        let id = row.id.clone();
        let archived = row.archived;
        let daemon_click = daemon_entity.clone();
        let listener = move |event: &ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
            let _ = daemon_click.update(cx, |daemon, cx| {
                if event.is_secondary() {
                    let position = event.mouse_position().unwrap_or_default();
                    let size = window.viewport_size();
                    daemon.ctx_menu = Some(crate::daemon::SidebarCtxMenu {
                        target: crate::daemon::SidebarCtxTarget::Session {
                            session_id: id.clone(),
                            archived,
                        },
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
            parent_title.clone(),
            anchors.clone(),
            selected,
            tone,
            t,
            animate_motion,
            listener,
        ));
    }
    list
}
