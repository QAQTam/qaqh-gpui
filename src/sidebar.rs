use crate::ui::{new_session_panel, settings_page, sidebar_row, todo_dock};
use crate::{
    bridge::BridgeRequest,
    settings,
    store::{DaemonStore, WorkspaceGroup},
    theme::ActiveTheme,
};
use gpui::prelude::*;
use gpui::{AnyElement, Context, KeyDownEvent, SharedString, Window, div, px};

impl Render for DaemonStore {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.colors();
        let mode = cx.theme_mode();
        let settings = settings::get(cx).clone();
        // rem 缩放:改设置里的界面字号 → 全 UI(rem 字号)等比缩放。
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
        let groups: Vec<WorkspaceGroup> = crate::store::groups(&visible, &self.workspaces);
        let mut rows: Vec<AnyElement> = Vec::new();
        for group in &groups {
            let key = group.id.clone().unwrap_or_else(|| "ungrouped".into());
            let collapsed = self.collapsed.contains(&key);
            let title = key.clone();
            let was_collapsed = collapsed;
            let group_id = group.id.clone();
            // hover 显现「移入」:把当前会话移进该分组;未分组头不渲染
            // (「脱离」由会话头 chip 承担,避免两处入口做同一件事)。
            let move_in = group_id.as_ref().map(|workspace_id| {
                let workspace_id = workspace_id.clone();
                div()
                    .id(SharedString::from(format!("move-in-{key}")))
                    .px_1()
                    .text_xs()
                    .text_color(t.accent)
                    .cursor_pointer()
                    .opacity(0.0)
                    .group_hover("grp", |style| style.opacity(1.0))
                    .on_click(cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(current) = daemon.current.clone() {
                            let workspace_id = workspace_id.clone();
                            current.update(cx, |store, _| store.move_to_workspace(workspace_id));
                        }
                        cx.notify();
                    }))
                    .child("移入")
            });
            // 分组头:可折叠 disclosure(借鉴 zed Disclosure chevron 模式)+
            // hover 显现「移入」(把当前会话移进该分组,electron move_session 同语义)。
            rows.push(
                div()
                    .id(SharedString::from(format!("grp-{key}")))
                    .group("grp")
                    .px_2p5()
                    .pt_2()
                    .pb_1()
                    .mx_0p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_sm()
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
                    .child(div().text_xs().text_color(t.dim).child(if collapsed {
                        "▸"
                    } else {
                        "▾"
                    }))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(SharedString::from(format!(
                                "{} ({})",
                                group.title,
                                group.rows.len()
                            ))),
                    )
                    .children(move_in)
                    .into_any_element(),
            );
            if collapsed {
                continue;
            }
            for row in &group.rows {
                let selected = self
                    .current
                    .as_ref()
                    .is_some_and(|current| current.read(cx).session_id == row.id);
                let id = row.id.clone();
                let listener = cx.listener(move |daemon: &mut DaemonStore, _event, window, cx| {
                    daemon.select(id.clone(), window, cx);
                });
                rows.push(sidebar_row(row, selected, &t, listener).into_any_element());
            }
        }

        let sidebar = div()
            .w(px(self.workbench.sidebar_width.unwrap_or(260.)))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(format!("会话({})", visible.len())),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("new-session")
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .bg(t.btn)
                                    .text_color(t.dim)
                                    .hover(|style| style.bg(t.btn_hover))
                                    .on_click(cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                                        daemon.new_session_open = !daemon.new_session_open;
                                        daemon.settings_form = None;
                                        if daemon.new_session_open {
                                            daemon.settings_open = false;
                                            let _ = daemon
                                                .requests
                                                .unbounded_send(BridgeRequest::ListProviders);
                                        }
                                        cx.notify();
                                    }))
                                    .child("新建"),
                            )
                            .child(
                                div()
                                    .id("theme-toggle")
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .bg(t.btn)
                                    .text_color(t.dim)
                                    .hover(|style| style.bg(t.btn_hover))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        let next = settings::get(cx).appearance.theme_mode.next();
                                        settings::update(cx, |content| {
                                            content.appearance.theme_mode = next;
                                        });
                                    }))
                                    .child(mode.label()),
                            )
                            .child(
                                div()
                                    .id("open-settings")
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .bg(if self.settings_open {
                                        t.accent_bg
                                    } else {
                                        t.btn
                                    })
                                    .text_color(if self.settings_open { t.accent } else { t.dim })
                                    .hover(|style| style.bg(t.btn_hover))
                                    .on_click(cx.listener(
                                        |daemon: &mut DaemonStore, _, window: &mut Window, cx| {
                                            daemon.settings_open = !daemon.settings_open;
                                            if daemon.settings_open {
                                                daemon.new_session_open = false;
                                                daemon.settings_form = None;
                                                // 打开时把当前值种进输入框,避免显示漂移;
                                                // 同时拉取 daemon 侧配置与服务商列表。
                                                let family = settings::get(cx)
                                                    .typography
                                                    .ui_font_family
                                                    .clone();
                                                daemon.font_field.update(cx, |field, cx| {
                                                    field.set_value(&family, window, cx)
                                                });
                                                let _ = daemon
                                                    .requests
                                                    .unbounded_send(BridgeRequest::QueryConfig);
                                                let _ = daemon
                                                    .requests
                                                    .unbounded_send(BridgeRequest::ListProviders);
                                            } else {
                                                daemon.settings_form = None;
                                            }
                                            cx.notify();
                                        },
                                    ))
                                    .child("设置"),
                            ),
                    ),
            )
            .child(crate::desktop::sidebar_tools(self, cx, &t))
            .child(
                div()
                    .id("session-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .child(crate::desktop::connection(self, cx, &t))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(t.border)
                    .text_xs()
                    .text_color(t.dim)
                    .child(self.lines.last().cloned().unwrap_or_default()),
            );

        let main = if self.new_session_open {
            new_session_panel(self, cx)
        } else if self.settings_open {
            settings_page(self, cx)
        } else {
            match self.current.clone() {
                Some(session) => div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .child(session)
                    .children(todo_dock(self, cx)),
                None => crate::desktop::welcome(self, cx, &t),
            }
        };

        let family = settings.typography.ui_font_family;
        div()
            .size_full()
            .flex()
            .on_key_down(cx.listener(|daemon, event: &KeyDownEvent, window, cx| {
                if !event.keystroke.modifiers.control {
                    return;
                }
                match event.keystroke.key.as_str() {
                    // 快捷键与按钮路径同源:面板互斥、表单清理、请求发送保持一致。
                    "n" => {
                        if !daemon.new_session_open {
                            daemon.new_session_open = true;
                            daemon.settings_open = false;
                            daemon.settings_form = None;
                            let _ = daemon
                                .requests
                                .unbounded_send(BridgeRequest::ListProviders);
                        }
                    }
                    "k" => daemon
                        .session_search
                        .update(cx, |field, cx| field.focus(window, cx)),
                    "," => {
                        if !daemon.settings_open {
                            daemon.settings_open = true;
                            daemon.new_session_open = false;
                            daemon.settings_form = None;
                            let family = settings::get(cx).typography.ui_font_family.clone();
                            daemon.font_field.update(cx, |field, cx| {
                                field.set_value(&family, window, cx)
                            });
                            let _ = daemon.requests.unbounded_send(BridgeRequest::QueryConfig);
                            let _ = daemon.requests.unbounded_send(BridgeRequest::ListProviders);
                        }
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
            .bg(t.bg)
            .text_color(t.text)
            .when(!family.is_empty(), |root| {
                root.font_family(SharedString::from(family))
            })
            .child(sidebar)
            .child(main)
    }
}
