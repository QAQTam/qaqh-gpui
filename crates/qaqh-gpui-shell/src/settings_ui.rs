//! 设置页内页渲染(DESIGN LAW v2.0):
//! - 全局策略全面内联化:布尔项全走 Switch,文本/数值全走行内 Input + 回车/应用;
//! - 工作区页提供行内输入框 + 原生目录选择器 + 精工卡片列表;
//! - 多字段编辑(服务商/模型)使用内嵌圆角返回式表单卡。

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Div, Entity, FontWeight, KeyDownEvent,
    PathPromptOptions, SharedString, Window, div, px,
};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, Sizable, Size, WindowExt as _};
use gpui_kit::assets::IconName;
use qaqh_client::{
    ProviderUpsertDto, RingingCommand, SecretWrite, SessionSettingsPatch, ToolsSettingPatch,
};
use qaqh_config_api::{ConfigPatch, ExecPatch, GoalsPatch, SandboxPatch};
use qaqh_profile::{ModelDef, ProfilesDocument, ProviderDef};
use qaqh_types::{CredentialSource, ModelSelection, Wire};

use qaqh_gpui_kit::kit::{
    BtnKind, RADIUS_CHIP, RADIUS_CONTROL, button, card, card_padded, pill, relative_time, soft,
};
use qaqh_gpui_kit::primitives::{
    action_button, section_card, section_header, segmented_button, setting_row,
};
use qaqh_gpui_kit::settings;
use qaqh_gpui_kit::theme::Palette;
use qaqh_gpui_wire::bridge::{BridgeRequest, ConnectionState, ProviderPresetView};
use qaqh_gpui_wire::diagnostics;

use crate::daemon::{DaemonStore, clean_session_title};

type Submit = dyn Fn(&[Field], &mut DaemonStore, &mut Context<DaemonStore>) -> Result<(), String>;

pub struct Form {
    title: String,
    fields: Vec<Field>,
    submit: Box<Submit>,
}

struct Field {
    key: &'static str,
    label: &'static str,
    control: FieldControl,
}

enum FieldControl {
    Input(Entity<InputState>),
    Switch(bool),
    Choice { value: String, choices: Vec<String> },
}

impl Field {
    fn text(
        key: &'static str,
        label: &'static str,
        value: impl ToString,
        window: &mut Window,
        cx: &mut Context<DaemonStore>,
    ) -> Self {
        let initial = value.to_string();
        let state = cx.new(|cx| InputState::new(window, cx));
        state.update(cx, |state, cx| state.set_value(initial.as_str(), window, cx));
        Self {
            key,
            label,
            control: FieldControl::Input(state),
        }
    }

    fn secret(
        key: &'static str,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<DaemonStore>,
    ) -> Self {
        Self {
            key,
            label,
            control: FieldControl::Input(cx.new(|cx| InputState::new(window, cx).masked(true))),
        }
    }

    fn switch(key: &'static str, label: &'static str, value: bool) -> Self {
        Self {
            key,
            label,
            control: FieldControl::Switch(value),
        }
    }

    fn choice(
        key: &'static str,
        label: &'static str,
        value: &str,
        choices: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            key,
            label,
            control: FieldControl::Choice {
                value: value.to_owned(),
                choices: choices.into_iter().map(Into::into).collect(),
            },
        }
    }

    fn find<'a>(fields: &'a [Field], key: &str) -> Result<&'a Field, String> {
        fields
            .iter()
            .find(|field| field.key == key)
            .ok_or_else(|| format!("内部错误：表单缺少字段 {key}"))
    }

    fn value(&self, cx: &App) -> String {
        match &self.control {
            FieldControl::Input(state) => state.read(cx).value().trim().to_string(),
            FieldControl::Switch(checked) => if *checked { "是" } else { "否" }.to_owned(),
            FieldControl::Choice { value, .. } => value.trim().to_owned(),
        }
    }

    fn number<T: std::str::FromStr>(&self, cx: &App) -> Result<T, String> {
        self.value(cx)
            .parse()
            .map_err(|_| format!("{}的数字格式无效", self.label))
    }

    fn required(&self, cx: &App) -> Result<String, String> {
        let value = self.value(cx);
        if value.is_empty() {
            return Err(format!("请填写{}", self.label));
        }
        Ok(value)
    }
}

fn begin(
    daemon: &mut DaemonStore,
    title: impl Into<String>,
    fields: Vec<Field>,
    submit: impl Fn(&[Field], &mut DaemonStore, &mut Context<DaemonStore>) -> Result<(), String>
        + 'static,
) {
    daemon.settings_form = Some(Form {
        title: title.into(),
        fields,
        submit: Box::new(submit),
    });
}

fn send(daemon: &mut DaemonStore, request: BridgeRequest) -> Result<(), String> {
    daemon
        .requests
        .unbounded_send(request)
        .map_err(|_| "设置连接已断开".into())
}

fn save(daemon: &mut DaemonStore, patch: ConfigPatch) -> Result<(), String> {
    patch.validate()?;
    send(daemon, BridgeRequest::SaveConfig(patch))
}

fn catalog_command(
    daemon: &mut DaemonStore,
    revision: u64,
    command: RingingCommand,
) -> Result<(), String> {
    send(daemon, BridgeRequest::ManageCatalog { command, revision })
}

fn report(daemon: &mut DaemonStore, result: Result<(), String>) {
    if let Err(error) = result {
        daemon.lines.push(error);
    }
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn optional(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn token_label(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{}M", tokens / 1_000_000)
    } else {
        format!("{}k", tokens / 1_000)
    }
}

fn credential_label(credential: qaqh_types::CredentialSource) -> &'static str {
    match credential {
        qaqh_types::CredentialSource::ApiKey => "API 密钥",
        qaqh_types::CredentialSource::ChatgptPlan => "ChatGPT 套餐",
        qaqh_types::CredentialSource::DeepseekAccount => "DeepSeek 账号",
    }
}

fn model_label(model: &ModelSelection) -> String {
    format!("{} / {}", model.provider_id, model.model_id)
}

fn dim_button(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    id: impl Into<String>,
    label: &'static str,
    run: impl Fn(&mut DaemonStore, &mut Window, &mut Context<DaemonStore>) + 'static,
) -> AnyElement {
    action_button(
        cx,
        id.into(),
        label,
        t.dim,
        cx.listener(move |daemon, _, window: &mut Window, cx| {
            run(daemon, window, cx);
            cx.notify();
        }),
        t,
    )
    .into_any_element()
}

fn dim_confirm_command(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    id: String,
    title: String,
    revision: u64,
    command: RingingCommand,
) -> AnyElement {
    dim_button(cx, t, id, "删除", move |daemon, _, _| {
        let command = command.clone();
        begin(daemon, title.clone(), vec![], move |_, daemon, _| {
            catalog_command(daemon, revision, command.clone())
        });
    })
}

/// 工作区页:行内输入框 + 原生目录选择 + 精工卡片列表。
fn workspaces(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let add_card = card_padded(t)
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child("添加工作区目录"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.dim)
                        .child("工作区目录位于 daemon 所在机器，移除分组仅删除分组记录，不删除本地文件。"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .on_key_down(cx.listener(|daemon, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "enter" {
                        let path = daemon.workspace_path_input.read(cx).value().trim().to_string();
                        if !path.is_empty() {
                            let res = send(
                                daemon,
                                BridgeRequest::Command {
                                    target: qaqh_client::CommandTarget::Global,
                                    command: RingingCommand::WorkspaceCreate { path },
                                },
                            );
                            if res.is_ok() {
                                daemon
                                    .workspace_path_input
                                    .update(cx, |f, cx| f.set_value("", window, cx));
                            }
                            report(daemon, res);
                            cx.notify();
                        }
                    }
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&daemon.workspace_path_input).small()),
                )
                .child(button(
                    cx,
                    BtnKind::Soft,
                    "ws-browse-dir",
                    "浏览目录…",
                    t,
                    cx.listener(|daemon, _, window, cx| {
                        let entity = cx.entity();
                        let picker = cx.prompt_for_paths(PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: false,
                            prompt: Some("选择工作区目录".into()),
                        });
                        let win_handle = window.window_handle();
                        let _ = daemon;
                        cx.spawn(async move |_, cx| {
                            if let Ok(Ok(Some(paths))) = picker.await
                                && let Some(path) = paths.into_iter().next()
                            {
                                let path_str = path.to_string_lossy().into_owned();
                                let _ = win_handle.update(cx, |_, window, cx| {
                                    let _ = entity.update(cx, |daemon, cx| {
                                        daemon
                                            .workspace_path_input
                                            .update(cx, |f, cx| f.set_value(&path_str, window, cx));
                                        cx.notify();
                                    });
                                });
                            }
                        })
                        .detach();
                    }),
                ))
                .child(button(
                    cx,
                    BtnKind::Primary,
                    "create-workspace",
                    "添加工作区",
                    t,
                    cx.listener(|daemon, _, window, cx| {
                        let path = daemon.workspace_path_input.read(cx).value().trim().to_string();
                        if path.is_empty() {
                            daemon.lines.push("请输入或选择工作区目录路径".into());
                            cx.notify();
                            return;
                        }
                        let res = send(
                            daemon,
                            BridgeRequest::Command {
                                target: qaqh_client::CommandTarget::Global,
                                command: RingingCommand::WorkspaceCreate { path },
                            },
                        );
                        if res.is_ok() {
                            daemon
                                .workspace_path_input
                                .update(cx, |f, cx| f.set_value("", window, cx));
                        }
                        report(daemon, res);
                        cx.notify();
                    }),
                )),
        );

    let mut body = div().flex().flex_col().gap_3().child(add_card);

    if daemon.workspaces.is_empty() {
        return body.child(
            card_padded(t).child(
                div()
                    .text_xs()
                    .text_color(t.dim)
                    .child("暂无已配置的工作区。"),
            ),
        );
    }

    for workspace in &daemon.workspaces {
        let id = workspace.id.clone();
        let title = workspace.title.clone();
        let rename_id = id.clone();
        body = body.child(
            card(t)
                .p_3p5()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .size(px(36.))
                                .flex_shrink_0()
                                .rounded(px(RADIUS_CONTROL))
                                .bg(t.accent_soft)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Folder)
                                        .with_size(Size::Small)
                                        .text_color(t.accent),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(t.text)
                                                .truncate()
                                                .child(title.clone()),
                                        )
                                        .children(workspace.missing_dir.then(|| {
                                            pill("目录不存在", t.bad, t)
                                        })),
                                )
                                .child(
                                    div()
                                        .font_family("monospace")
                                        .text_xs()
                                        .text_color(t.dim)
                                        .truncate()
                                        .child(workspace.path.clone()),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .flex_shrink_0()
                        .child(button(
                            cx,
                            BtnKind::Soft,
                            format!("rename-workspace-{id}"),
                            "重命名",
                            t,
                            cx.listener(move |daemon, _, window, cx| {
                                let id = rename_id.clone();
                                let title = title.clone();
                                begin(
                                    daemon,
                                    "重命名工作区",
                                    vec![Field::text("title", "名称", &title, window, cx)],
                                    move |fields, daemon, cx| {
                                        send(
                                            daemon,
                                            BridgeRequest::Command {
                                                target: qaqh_client::CommandTarget::Workspace {
                                                    workspace_id: id.clone(),
                                                },
                                                command: RingingCommand::WorkspaceRename {
                                                    title: fields[0].required(cx)?,
                                                },
                                            },
                                        )
                                    },
                                );
                                cx.notify();
                            }),
                        ))
                        .child(button(
                            cx,
                            BtnKind::Ghost,
                            format!("remove-workspace-{id}"),
                            "移除分组",
                            t,
                            cx.listener(move |daemon, _, _, cx| {
                                let result = send(
                                    daemon,
                                    BridgeRequest::Command {
                                        target: qaqh_client::CommandTarget::Workspace {
                                            workspace_id: id.clone(),
                                        },
                                        command: RingingCommand::WorkspaceDelete {},
                                    },
                                );
                                report(daemon, result);
                                cx.notify();
                            }),
                        )),
                ),
        );
    }
    body
}

/// 账号页:siwc(ChatGPT 套餐)与 dsaccount(DeepSeek 账号)。
pub(crate) fn accounts(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let mut body = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .px_1()
                .text_xs()
                .text_color(t.dim)
                .child("浏览器授权登录，凭据保存在本机账号管理器中，不会写入 daemon 明文配置。"),
        )
        .child(siwc_card(daemon, cx, t))
        .child(dsaccount_card(daemon, cx, t));
    if let Some(line) = &daemon.account_line {
        body = body.child(
            div()
                .px_3()
                .py_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(soft(t.warn, 0.10))
                .text_xs()
                .text_color(t.warn)
                .child(line.clone()),
        );
    }
    body
}

fn siwc_card(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    use qaqh_client::service_types as st;
    let base = card_padded(t).flex().flex_col().gap_3();
    let Some(status) = &daemon.siwc else {
        return base.child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child("ChatGPT 套餐（SIWC）：正在读取状态…"),
        );
    };
    let (state, color) = match status.status {
        st::SiwcConnectionState::Connected if status.sharing => ("已登录，套餐直连可用", t.ok),
        st::SiwcConnectionState::Connected => ("已登录，未授予套餐直连", t.warn),
        st::SiwcConnectionState::Connecting => ("登录流程进行中", t.warn),
        st::SiwcConnectionState::ReauthRequired => ("凭据已失效，请重新登录", t.bad),
        st::SiwcConnectionState::Disconnected => ("未登录", t.dim),
    };
    let who = status
        .email
        .as_deref()
        .or(status.name.as_deref())
        .unwrap_or_default();

    let mut actions = div().flex().items_center().gap_2();
    let signing = daemon
        .sign_in
        .as_ref()
        .is_some_and(|flow| flow.service == qaqh_gpui_session::store::AccountService::Siwc);
    if signing {
        actions = actions.child(button(
            cx,
            BtnKind::Ghost,
            "siwc-signin-cancel",
            "取消登录",
            t,
            cx.listener(|daemon, _, _, cx| {
                daemon.sign_in_cancel();
                cx.notify();
            }),
        ));
    } else {
        match status.status {
            st::SiwcConnectionState::Connected | st::SiwcConnectionState::ReauthRequired => {
                actions = actions
                    .child(button(
                        cx,
                        BtnKind::Soft,
                        "siwc-usage",
                        "查看用量",
                        t,
                        cx.listener(|daemon, _, _, cx| {
                            if let Some(status) = &daemon.siwc {
                                cx.open_url(&status.usage_url);
                            }
                        }),
                    ))
                    .child(button(
                        cx,
                        BtnKind::Ghost,
                        "siwc-signout",
                        "登出",
                        t,
                        cx.listener(|daemon, _, _, cx| {
                            daemon.sign_out(qaqh_gpui_session::store::AccountService::Siwc);
                            cx.notify();
                        }),
                    ));
                if status.status == st::SiwcConnectionState::ReauthRequired {
                    actions = actions.child(button(
                        cx,
                        BtnKind::Primary,
                        "siwc-signin",
                        "重新登录",
                        t,
                        cx.listener(|daemon, _, _, cx| {
                            daemon.sign_in_start(qaqh_gpui_session::store::AccountService::Siwc);
                            cx.notify();
                        }),
                    ));
                }
            }
            _ => {
                actions = actions.child(button(
                    cx,
                    BtnKind::Primary,
                    "siwc-signin",
                    "登录 ChatGPT 套餐",
                    t,
                    cx.listener(|daemon, _, _, cx| {
                        daemon.sign_in_start(qaqh_gpui_session::store::AccountService::Siwc);
                        cx.notify();
                    }),
                ));
            }
        }
    }

    base.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .child("ChatGPT 套餐（SIWC）"),
                            )
                            .child(pill(state, color, t)),
                    )
                    .when(!who.is_empty(), |col| {
                        col.child(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .child(SharedString::from(who.to_owned())),
                        )
                    }),
            )
            .child(actions),
    )
}

fn dsaccount_card(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let mut base = card_padded(t).flex().flex_col().gap_3();
    let Some(status) = &daemon.dsaccount else {
        return base.child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child("DeepSeek 账号：正在读取状态…"),
        );
    };
    let (state, color) = if status.signed_in {
        ("已登录", t.ok)
    } else if status.flow.state.is_running() {
        ("登录流程进行中", t.warn)
    } else {
        ("未登录", t.dim)
    };
    let who = status
        .account
        .as_ref()
        .map(|account| account.label.as_str())
        .filter(|label| !label.is_empty())
        .unwrap_or_default();

    let mut actions = div().flex().items_center().gap_2();
    let signing = daemon
        .sign_in
        .as_ref()
        .is_some_and(|flow| flow.service == qaqh_gpui_session::store::AccountService::Dsaccount);
    if signing {
        actions = actions.child(button(
            cx,
            BtnKind::Ghost,
            "ds-signin-cancel",
            "取消登录",
            t,
            cx.listener(|daemon, _, _, cx| {
                daemon.sign_in_cancel();
                cx.notify();
            }),
        ));
    } else if status.signed_in {
        actions = actions
            .child(button(
                cx,
                BtnKind::Soft,
                "ds-topup",
                "充值",
                t,
                cx.listener(|daemon, _, _, cx| {
                    if let Some(status) = &daemon.dsaccount {
                        cx.open_url(&status.top_up_url);
                    }
                }),
            ))
            .child(button(
                cx,
                BtnKind::Ghost,
                "ds-signout",
                "登出",
                t,
                cx.listener(|daemon, _, _, cx| {
                    daemon.sign_out(qaqh_gpui_session::store::AccountService::Dsaccount);
                    cx.notify();
                }),
            ));
    } else {
        actions = actions.child(button(
            cx,
            BtnKind::Primary,
            "ds-signin",
            "登录 DeepSeek 账号",
            t,
            cx.listener(|daemon, _, _, cx| {
                daemon.sign_in_start(qaqh_gpui_session::store::AccountService::Dsaccount);
                cx.notify();
            }),
        ));
    }

    base = base.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .child("DeepSeek 账号"),
                            )
                            .child(pill(state, color, t)),
                    )
                    .when(!who.is_empty(), |col| {
                        col.child(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .child(SharedString::from(who.to_owned())),
                        )
                    }),
            )
            .child(actions),
    );

    if let Some(error) = &status.storage_error {
        base = base.child(
            div()
                .text_xs()
                .text_color(t.bad)
                .child(SharedString::from(format!("本地凭据读取失败: {error}"))),
        );
    }

    if let Some(balance) = &daemon.ds_balance {
        let mut wallet_chips = Vec::new();
        for wallet in &balance.recharge {
            wallet_chips.push(format!("充值余额 {} {}", wallet.balance, wallet.currency));
        }
        for wallet in &balance.bonus {
            wallet_chips.push(format!("赠金 {} {}", wallet.balance, wallet.currency));
        }
        for cost in &balance.total_cost {
            wallet_chips.push(format!("累计消费 {} {}", cost.amount, cost.currency));
        }
        if !wallet_chips.is_empty() {
            base = base.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .pt_1()
                    .border_t_1()
                    .border_color(t.border_subtle)
                    .children(wallet_chips.into_iter().map(|text| {
                        div()
                            .px_2p5()
                            .py_1()
                            .rounded(px(RADIUS_CHIP))
                            .bg(t.surface_subtle)
                            .font_family("monospace")
                            .text_xs()
                            .text_color(t.dim)
                            .child(text)
                    })),
            );
        }
        if !balance.has_usable_balance {
            base = base.child(div().text_xs().text_color(t.bad).child("当前没有可用余额"));
        }
    }
    base
}

fn archived(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let archived_rows: Vec<_> = daemon.sessions.iter().filter(|row| row.archived).collect();
    if archived_rows.is_empty() {
        return card_padded(t)
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_8()
            .child(
                Icon::new(IconName::Archive)
                    .with_size(Size::Medium)
                    .text_color(t.faint),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text)
                    .child("暂无已归档会话"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(t.dim)
                    .child("在侧栏对任意会话右键选择「归档」，或恢复后可继续使用。"),
            );
    }

    let cards = archived_rows.into_iter().map(|row| {
        let title = clean_session_title(row, &daemon.workspaces).to_string();
        let ws_name = row
            .workspace_id
            .as_ref()
            .and_then(|wid| daemon.workspaces.iter().find(|w| &w.id == wid))
            .map(|w| w.title.clone())
            .or_else(|| row.cwd.clone())
            .unwrap_or_else(|| "默认目录".to_owned());
        let when = relative_time(row.updated_at);
        let id_view = row.id.clone();
        let id_restore = row.id.clone();
        let id_delete = row.id.clone();
        let title_for_del = title.clone();

        card_padded(t)
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .truncate()
                            .child(SharedString::from(format!("{ws_name} · {when}"))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .flex_shrink_0()
                    .child(button(
                        cx,
                        BtnKind::Ghost,
                        format!("arch-open-{id_view}"),
                        "查看",
                        t,
                        cx.listener(move |daemon: &mut DaemonStore, _, window, cx| {
                            daemon.settings_open = false;
                            daemon.settings_form = None;
                            daemon.select(id_view.clone(), window, cx);
                        }),
                    ))
                    .child(button(
                        cx,
                        BtnKind::Soft,
                        format!("arch-restore-{id_restore}"),
                        "恢复",
                        t,
                        cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                            let res = send(
                                daemon,
                                BridgeRequest::Command {
                                    target: qaqh_client::CommandTarget::Session {
                                        session_id: id_restore.clone(),
                                    },
                                    command: RingingCommand::SessionUnarchive {},
                                },
                            );
                            report(daemon, res);
                            cx.notify();
                        }),
                    ))
                    .child(dim_button(
                        cx,
                        t,
                        format!("arch-del-{id_delete}"),
                        "删除",
                        move |daemon, _, _| {
                            let seed = id_delete.clone();
                            begin(
                                daemon,
                                format!("永久删除已归档会话「{title_for_del}」？"),
                                vec![],
                                move |_, daemon, _| {
                                    send(
                                        daemon,
                                        BridgeRequest::DeleteSession { seed: seed.clone() },
                                    )
                                },
                            );
                        },
                    )),
            )
    });

    div().flex().flex_col().gap_2p5().children(cards)
}

fn about(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let conn_text = match &daemon.connection {
        ConnectionState::Ready { client_session_id } => {
            format!("已连接 · 客户端会话 {client_session_id}")
        }
        ConnectionState::Connecting => "正在连接本地 daemon…".to_owned(),
        ConnectionState::Failed { detail } => format!("连接异常：{detail}"),
    };
    let arch_text = format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH);
    let rev_text = format!(
        "Commands Rev {} · Session Rev {}",
        qaqh_client::ringing::COMMAND_CONTRACT_REVISION,
        qaqh_client::ringing::SESSION_CONTRACT_REVISION,
    );
    let config_path = settings::config_path().display().to_string();
    let diag_path = diagnostics::path().display().to_string();

    let copy_diag_btn = button(
        cx,
        BtnKind::Soft,
        "about-copy-diagnostics",
        "复制诊断报告",
        t,
        cx.listener(|_, _, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(diagnostics::report()));
            window.push_notification(
                gpui_kit::component::notification::Notification::success("诊断报告已复制到剪贴板"),
                cx,
            );
        }),
    );

    let engine_value = div()
        .text_xs()
        .text_color(t.text)
        .child(SharedString::from(format!(
            "QAQHarness v{}",
            diagnostics::version()
        )))
        .into_any_element();
    let rev_value = div()
        .font_family("monospace")
        .text_xs()
        .text_color(t.dim)
        .child(rev_text)
        .into_any_element();
    let arch_value = div()
        .font_family("monospace")
        .text_xs()
        .text_color(t.dim)
        .child(arch_text)
        .into_any_element();
    let cfg_value = div()
        .font_family("monospace")
        .text_xs()
        .text_color(t.dim)
        .child(config_path)
        .into_any_element();
    let log_value = div()
        .font_family("monospace")
        .text_xs()
        .text_color(t.dim)
        .child(diag_path)
        .into_any_element();

    div()
        .flex()
        .flex_col()
        .gap_5()
        .child(section_card(
            "桌面壳与协议",
            vec![
                setting_row(
                    "应用版本",
                    "gpui-kit 0.11.0（纯原生 DX11 / DirectWrite 渲染，零 WebView）",
                    engine_value,
                    t,
                ),
                setting_row("RingingV2 契约", "当前前端绑定协议版本", rev_value, t),
                setting_row("运行平台", "目标操作系统与指令集架构", arch_value, t),
                setting_row("Daemon 连接", &conn_text, div().into_any_element(), t),
            ],
            t,
        ))
        .child(section_card(
            "本地路径与排障",
            vec![
                setting_row("界面偏好文件", "Theme / MotionLevel / 排版配置持久化路径", cfg_value, t),
                setting_row("诊断事件日志", "脱敏记录协议状态，不含对话或密钥内容", log_value, t),
                setting_row(
                    "导出诊断快照",
                    "复制版本、契约修订号与近期事件摘要到剪贴板",
                    copy_diag_btn,
                    t,
                ),
            ],
            t,
        ))
}

pub fn render(
    daemon: &DaemonStore,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    if let Some(form) = &daemon.settings_form {
        return render_form(form, cx, t);
    }
    match daemon.settings_editor {
        qaqh_gpui_session::store::SettingsEditor::Local => div(),
        qaqh_gpui_session::store::SettingsEditor::Config => global(daemon, cx, t),
        qaqh_gpui_session::store::SettingsEditor::Catalog => catalog(daemon, cx, t),
        qaqh_gpui_session::store::SettingsEditor::Accounts => accounts(daemon, cx, t),
        qaqh_gpui_session::store::SettingsEditor::Session => session(daemon, cx, t),
        qaqh_gpui_session::store::SettingsEditor::Workspaces => workspaces(daemon, cx, t),
        qaqh_gpui_session::store::SettingsEditor::Archived => archived(daemon, cx, t),
        qaqh_gpui_session::store::SettingsEditor::About => about(daemon, cx, t),
    }
}

pub(crate) fn render_form(
    form: &Form,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let mut body = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pb_3()
                .border_b_1()
                .border_color(t.border_subtle)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(form.title.clone()),
                )
                .child(button(
                    cx,
                    BtnKind::Ghost,
                    "settings-form-back",
                    "返回",
                    t,
                    cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                        daemon.settings_form = None;
                        cx.notify();
                    }),
                )),
        );

    let mut index = 0;
    while index < form.fields.len() {
        let field = &form.fields[index];
        if matches!(field.control, FieldControl::Switch(_)) {
            let mut run = Vec::new();
            while index < form.fields.len()
                && matches!(form.fields[index].control, FieldControl::Switch(_))
            {
                run.push(index);
                index += 1;
            }
            body = body.child(switch_run_row(form, &run, cx, t));
            continue;
        }
        let control = match &field.control {
            FieldControl::Input(state) => {
                div().w(px(300.)).child(Input::new(state).small()).into_any_element()
            }
            FieldControl::Switch(_) => unreachable!(),
            FieldControl::Choice { value, choices } => {
                let choices = choices.iter().map(|choice| {
                    segmented_button(
                        format!("settings-choice-{index}-{choice}"),
                        choice.clone(),
                        value.as_str() == choice.as_str(),
                        t,
                        {
                            let choice = choice.clone();
                            cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                                if let Some(form) = &mut daemon.settings_form
                                    && let FieldControl::Choice { value, .. } =
                                        &mut form.fields[index].control
                                {
                                    *value = choice.clone();
                                }
                                cx.notify();
                            })
                        },
                    )
                });
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(choices)
                    .into_any_element()
            }
        };
        body = body.child(setting_row(field.label, "", control, t));
        index += 1;
    }

    body = body.child(
        div()
            .flex()
            .justify_end()
            .gap_2()
            .pt_3()
            .child(button(
                cx,
                BtnKind::Ghost,
                "settings-cancel",
                "取消",
                t,
                cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                    daemon.settings_form = None;
                    cx.notify();
                }),
            ))
            .child(button(
                cx,
                if form.fields.is_empty() {
                    BtnKind::Danger
                } else {
                    BtnKind::Primary
                },
                "settings-save",
                if form.fields.is_empty() {
                    "确认执行"
                } else {
                    "保存"
                },
                t,
                cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                    if let Some(form) = daemon.settings_form.take()
                        && let Err(error) = (form.submit)(&form.fields, daemon, cx)
                    {
                        daemon.settings_form = Some(form);
                        daemon.lines.push(error);
                    }
                    cx.notify();
                }),
            )),
    );
    card_padded(t).child(body)
}

fn switch_run_row(
    form: &Form,
    indexes: &[usize],
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let items = indexes.iter().map(|&index| {
        let FieldControl::Switch(checked) = &form.fields[index].control else {
            unreachable!();
        };
        let label = form.fields[index].label;
        let entity = cx.entity();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_xs().text_color(t.dim).child(label))
            .child(
                Switch::new(SharedString::from(format!("settings-switch-{index}")))
                    .checked(*checked)
                    .small()
                    .on_change(move |checked: &bool, _, cx: &mut gpui_kit::App| {
                        entity.update(cx, |daemon, cx| {
                            if let Some(form) = &mut daemon.settings_form
                                && let FieldControl::Switch(current) =
                                    &mut form.fields[index].control
                            {
                                *current = *checked;
                            }
                            cx.notify();
                        });
                    }),
            )
    });
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x_6()
        .gap_y_2()
        .py_3()
        .border_b_1()
        .border_color(t.border_subtle)
        .children(items)
}

fn inline_switch_row(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    current: bool,
    patch: impl Fn(bool) -> ConfigPatch + 'static,
) -> Div {
    let entity = cx.entity();
    let control = Switch::new(id)
        .checked(current)
        .small()
        .on_change(move |checked: &bool, _, cx: &mut App| {
            let next = *checked;
            let p = patch(next);
            entity.update(cx, |daemon, cx| {
                let res = save(daemon, p);
                report(daemon, res);
                cx.notify();
            });
        })
        .into_any_element();
    setting_row(label, desc, control, t)
}

fn inline_input_row(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    width: f32,
    field: &Entity<InputState>,
    apply: impl Fn(&str) -> Result<ConfigPatch, String> + Clone + 'static,
) -> Div {
    let field_apply = field.clone();
    let apply_enter = apply.clone();
    let field_enter = field.clone();
    let control = div()
        .flex()
        .items_center()
        .gap_1p5()
        .on_key_down(cx.listener(move |daemon: &mut DaemonStore, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "enter" {
                let raw = field_enter.read(cx).value().trim().to_string();
                let res = apply_enter(&raw).and_then(|patch| save(daemon, patch));
                report(daemon, res);
                cx.notify();
            }
        }))
        .child(div().w(px(width)).child(Input::new(field).small()))
        .child(button(
            cx,
            BtnKind::Soft,
            id,
            "应用",
            t,
            cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                let raw = field_apply.read(cx).value().trim().to_string();
                let res = apply(&raw).and_then(|patch| save(daemon, patch));
                report(daemon, res);
                cx.notify();
            }),
        ))
        .into_any_element();
    setting_row(label, desc, control, t)
}

/// 全局策略:全 Switch + 行内输入框即时应用,彻底废除点编辑跳单字段页。
fn global(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let Some(config) = &daemon.config else {
        return card_padded(t).child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child("正在读取 daemon 全局配置…"),
        );
    };

    let run_policy = section_card(
        "运行与安全策略",
        vec![
            crate::views::permission_row(daemon, cx, t),
            inline_switch_row(
                cx,
                t,
                "cfg-sandbox",
                "命令沙箱",
                "在隔离沙箱环境中执行工具命令",
                config.sandbox.enabled,
                |enabled| ConfigPatch {
                    sandbox: Some(SandboxPatch {
                        enabled: Some(enabled),
                    }),
                    ..Default::default()
                },
            ),
            inline_switch_row(
                cx,
                t,
                "cfg-allowlist",
                "全局工具白名单",
                "仅放行下方列出的受信任工具",
                config.tool_allowlist_enabled,
                |enabled| ConfigPatch {
                    tool_allowlist_enabled: Some(enabled),
                    ..Default::default()
                },
            ),
            inline_input_row(
                cx,
                t,
                "apply-allowlist",
                "允许的工具名单",
                "使用英文逗号分隔工具名称",
                220.,
                &daemon.global_inputs.tool_allowlist,
                |raw| {
                    Ok(ConfigPatch {
                        tool_allowlist: Some(list(raw)),
                        ..Default::default()
                    })
                },
            ),
            inline_input_row(
                cx,
                t,
                "apply-compact",
                "自动压缩阈值 (0–1)",
                "上下文占用超过该比例时自动整理历史",
                96.,
                &daemon.global_inputs.auto_compact,
                |raw| {
                    let val = raw.parse().map_err(|_| "请输入 0 到 1 之间的小数".to_string())?;
                    Ok(ConfigPatch {
                        auto_compact_threshold: Some(val),
                        ..Default::default()
                    })
                },
            ),
            inline_input_row(
                cx,
                t,
                "apply-idle-unload",
                "空闲卸载时长 (秒)",
                "超过秒数无交互后卸载会话内存，0 为关闭",
                96.,
                &daemon.global_inputs.idle_unload,
                |raw| {
                    let val = raw.parse().map_err(|_| "请输入有效非负整数".to_string())?;
                    Ok(ConfigPatch {
                        session_idle_unload_secs: Some(val),
                        ..Default::default()
                    })
                },
            ),
            inline_input_row(
                cx,
                t,
                "apply-shell",
                "默认 Shell",
                "如 pwsh / bash / cmd，填 auto 自动探测",
                140.,
                &daemon.global_inputs.default_shell,
                |raw| {
                    Ok(ConfigPatch {
                        exec: Some(ExecPatch {
                            default_shell: Some(raw.to_owned()),
                        }),
                        ..Default::default()
                    })
                },
            ),
        ],
        t,
    );

    let goal_section = section_card(
        "目标续跑模式 (Goal)",
        vec![
            inline_switch_row(
                cx,
                t,
                "cfg-goals",
                "启用 Goal 模式",
                "允许代理围绕长期目标自动续跑多轮直到达成",
                config.goals.enabled,
                |enabled| ConfigPatch {
                    goals: Some(GoalsPatch {
                        enabled: Some(enabled),
                        max_token_budget: None,
                    }),
                    ..Default::default()
                },
            ),
            inline_input_row(
                cx,
                t,
                "apply-goal-budget",
                "默认 Token 预算",
                "留空应用即可清除预算上限；不允许填 0",
                140.,
                &daemon.global_inputs.goal_budget,
                |raw| {
                    let budget = if raw.is_empty() {
                        None
                    } else {
                        let value: u64 = raw.parse().map_err(|_| "预算必须是正整数".to_string())?;
                        if value == 0 {
                            return Err("预算必须大于 0（留空可清除上限）".into());
                        }
                        Some(value)
                    };
                    Ok(ConfigPatch {
                        goals: Some(GoalsPatch {
                            enabled: None,
                            max_token_budget: Some(budget),
                        }),
                        ..Default::default()
                    })
                },
            ),
        ],
        t,
    );

    let integration_rows: Vec<Div> = [
        (
            "MCP 服务集成",
            config.mcp.enabled,
            config
                .mcp
                .servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
        ),
        (
            "LSP 语言服务",
            config.lsp.enabled,
            config
                .lsp
                .servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
        ),
    ]
    .into_iter()
    .map(|(name, enabled, servers)| {
        let desc = if servers.is_empty() {
            "未配置任何服务实例".to_owned()
        } else {
            servers.join("、")
        };
        let status_pill = if enabled {
            pill("已启用", t.ok, t)
        } else {
            pill("已停用", t.dim, t)
        };
        setting_row(name, &desc, status_pill.into_any_element(), t)
    })
    .collect();

    div()
        .flex()
        .flex_col()
        .gap_5()
        .child(run_policy)
        .child(goal_section)
        .child(section_card("工具集成（只读）", integration_rows, t))
}

fn provider_form(
    daemon: &mut DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    revision: u64,
    existing: Option<(String, ProviderDef)>,
    preset: Option<ProviderPresetView>,
) {
    let provider = existing.as_ref().map(|(_, provider)| provider);
    let wire = provider
        .map(|provider| provider.wire.as_str())
        .or_else(|| preset.as_ref().map(|p| p.wire.as_str()))
        .unwrap_or("chat");
    let credential = credential_label(
        provider
            .map(|provider| provider.credential)
            .or_else(|| preset.as_ref().map(|p| p.credential))
            .unwrap_or_default(),
    );
    let mut fields = Vec::new();
    if existing.is_none() {
        fields.push(Field::text(
            "provider_id",
            "服务商 ID",
            preset.as_ref().map(|p| p.id.as_str()).unwrap_or_default(),
            window,
            cx,
        ));
    }
    fields.extend([
        Field::text(
            "name",
            "显示名称",
            provider
                .map(|p| p.name.as_str())
                .or_else(|| preset.as_ref().map(|p| p.name.as_str()))
                .unwrap_or_default(),
            window,
            cx,
        ),
        Field::text(
            "base_url",
            "端点 URL",
            provider
                .map(|p| p.base_url.as_str())
                .or_else(|| preset.as_ref().map(|p| p.base_url.as_str()))
                .unwrap_or_default(),
            window,
            cx,
        ),
        Field::choice(
            "wire",
            "接口协议",
            wire,
            ["chat", "responses", "messages", "gemini"],
        ),
        Field::choice(
            "credential",
            "凭据来源",
            credential,
            ["API 密钥", "ChatGPT 套餐", "DeepSeek 账号"],
        ),
        Field::secret("api_key", "新 API 密钥（留空保留）", window, cx),
        Field::switch("clear_key", "清除已存密钥", false),
    ]);
    let title = existing
        .as_ref()
        .map(|(id, _)| format!("编辑服务商 · {id}"))
        .or_else(|| preset.as_ref().map(|p| format!("从官方预设添加 · {}", p.name)))
        .unwrap_or_else(|| "添加服务商".into());
    begin(daemon, title, fields, move |fields, daemon, cx| {
        let id = match &existing {
            Some((id, _)) => id.clone(),
            None => Field::find(fields, "provider_id")?.required(cx)?,
        };
        let provider = existing.as_ref().map(|(_, p)| p);
        let api_key = match (
            Field::find(fields, "api_key")?.value(cx).as_str(),
            Field::find(fields, "clear_key")?.value(cx).as_str(),
        ) {
            ("", "是") => SecretWrite::Clear,
            ("", _) => SecretWrite::Keep,
            (_, "是") => return Err("请在设置新密钥与清除密钥之间选择一项".into()),
            (key, _) => SecretWrite::Set(key.into()),
        };
        catalog_command(
            daemon,
            revision,
            RingingCommand::ProviderUpsert {
                id,
                provider: ProviderUpsertDto {
                    name: Field::find(fields, "name")?.required(cx)?,
                    base_url: Field::find(fields, "base_url")?.required(cx)?,
                    wire: Wire::parse(&Field::find(fields, "wire")?.value(cx))
                        .ok_or("请选择接口协议")?,
                    credential: match Field::find(fields, "credential")?.value(cx).as_str() {
                        "ChatGPT 套餐" => CredentialSource::ChatgptPlan,
                        "DeepSeek 账号" => CredentialSource::DeepseekAccount,
                        _ => CredentialSource::ApiKey,
                    },
                    headers: provider.map(|p| p.headers.clone()).unwrap_or_default(),
                    compat: provider.and_then(|p| p.compat.clone()),
                    api_key,
                },
            },
        )
    });
}

fn model_form(
    daemon: &mut DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    revision: u64,
    provider_id: String,
    existing: Option<(String, ModelDef)>,
) {
    let model = existing
        .as_ref()
        .map(|(_, model)| model.clone())
        .unwrap_or_default();
    let mut fields = Vec::new();
    if existing.is_none() {
        fields.push(Field::text("model_id", "模型 ID", "", window, cx));
    }
    fields.extend([
        Field::text(
            "display_name",
            "显示名称",
            model.display_name.as_deref().unwrap_or_default(),
            window,
            cx,
        ),
        Field::text(
            "context_window",
            "上下文窗口",
            model.context_window,
            window,
            cx,
        ),
        Field::text(
            "max_output_tokens",
            "输出 Token 上限",
            model.max_output_tokens,
            window,
            cx,
        ),
        Field::switch("enabled", "启用", model.enabled),
        Field::switch("tool_calls", "工具调用", model.capabilities.tool_calls),
        Field::switch("image_input", "图片输入", model.capabilities.image_input),
        Field::switch(
            "native_web_search",
            "原生网页搜索",
            model.capabilities.native_web_search,
        ),
        Field::switch("reasoning", "推理", model.capabilities.reasoning),
        Field::text(
            "reasoning_levels",
            "推理档位（逗号分隔）",
            model.reasoning_levels.join(", "),
            window,
            cx,
        ),
        Field::text(
            "default_reasoning_effort",
            "默认推理档位",
            model
                .defaults
                .reasoning_effort
                .as_deref()
                .unwrap_or_default(),
            window,
            cx,
        ),
        Field::text(
            "default_max_output_tokens",
            "默认输出 Token",
            model
                .defaults
                .max_output_tokens
                .map(|n| n.to_string())
                .unwrap_or_default(),
            window,
            cx,
        ),
        Field::switch(
            "default_native_web_search",
            "默认启用原生搜索",
            model.defaults.native_web_search,
        ),
    ]);
    let title = existing
        .as_ref()
        .map(|(id, _)| format!("编辑模型 · {id}"))
        .unwrap_or_else(|| format!("添加模型 · {provider_id}"));
    begin(daemon, title, fields, move |fields, daemon, cx| {
        let model_id = match &existing {
            Some((id, _)) => id.clone(),
            None => Field::find(fields, "model_id")?.required(cx)?,
        };
        let mut model = model.clone();
        model.display_name = optional(&Field::find(fields, "display_name")?.value(cx));
        model.context_window = Field::find(fields, "context_window")?.number(cx)?;
        model.max_output_tokens = Field::find(fields, "max_output_tokens")?.number(cx)?;
        model.enabled = Field::find(fields, "enabled")?.value(cx) == "是";
        model.capabilities.tool_calls = Field::find(fields, "tool_calls")?.value(cx) == "是";
        model.capabilities.image_input = Field::find(fields, "image_input")?.value(cx) == "是";
        model.capabilities.native_web_search =
            Field::find(fields, "native_web_search")?.value(cx) == "是";
        model.capabilities.reasoning = Field::find(fields, "reasoning")?.value(cx) == "是";
        model.reasoning_levels = list(&Field::find(fields, "reasoning_levels")?.value(cx));
        model.defaults.reasoning_effort =
            optional(&Field::find(fields, "default_reasoning_effort")?.value(cx));
        let default_tokens = Field::find(fields, "default_max_output_tokens")?;
        model.defaults.max_output_tokens = optional(&default_tokens.value(cx))
            .map(|_| default_tokens.number(cx))
            .transpose()?;
        model.defaults.native_web_search =
            Field::find(fields, "default_native_web_search")?.value(cx) == "是";
        catalog_command(
            daemon,
            revision,
            RingingCommand::ProviderModelUpsert {
                provider_id: provider_id.clone(),
                model_id,
                model,
            },
        )
    });
}

fn catalog(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let Some(catalog) = daemon.providers.as_deref() else {
        return card_padded(t).child(div().text_xs().text_color(t.dim).child("正在读取模型目录…"));
    };
    let revision = catalog.revision;
    let add = button(
        cx,
        BtnKind::Primary,
        "provider-add",
        "自定义添加",
        t,
        cx.listener(move |daemon, _, window, cx| {
            provider_form(daemon, window, cx, revision, None, None);
            cx.notify();
        }),
    );
    let mut body = div().flex().flex_col().gap_3().child(
        card_padded(t).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child("服务商目录"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .child("管理模型端点、接口协议与凭据来源"),
                        ),
                )
                .child(add),
        ),
    );
    if !daemon.provider_presets.is_empty() {
        let chips = daemon.provider_presets.iter().map(|preset| {
            let preset_owned = preset.clone();
            let configured = catalog.providers.get(&preset.id).cloned();
            let exists = configured.is_some();
            div()
                .id(SharedString::from(format!("preset-chip-{}", preset.id)))
                .px_2p5()
                .py_1p5()
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(if exists { t.border } else { t.border_subtle })
                .bg(if exists { t.selected } else { t.surface_subtle })
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(move |daemon, _, window, cx| {
                    if let Some(existing_def) = &configured {
                        provider_form(
                            daemon,
                            window,
                            cx,
                            revision,
                            Some((preset_owned.id.clone(), existing_def.clone())),
                            None,
                        );
                    } else {
                        provider_form(
                            daemon,
                            window,
                            cx,
                            revision,
                            None,
                            Some(preset_owned.clone()),
                        );
                    }
                    cx.notify();
                }))
                .child(
                    Icon::new(if exists {
                        IconName::Check
                    } else {
                        IconName::Zap
                    })
                    .with_size(Size::XSmall)
                    .text_color(if exists { t.ok } else { t.dim }),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text)
                        .child(preset.name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.faint)
                        .child(preset.wire.as_str()),
                )
        });
        body = body.child(
            card_padded(t)
                .flex()
                .flex_col()
                .gap_2p5()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.dim)
                        .child("官方预设一键填充（点击填入表单并配置密钥）"),
                )
                .child(div().flex().flex_wrap().gap_2().children(chips)),
        );
    }
    for (id, provider) in &catalog.providers {
        body = body.child(provider_card(catalog, id, provider, cx, t));
    }
    body
}

fn provider_card(
    catalog: &ProfilesDocument,
    id: &str,
    provider: &ProviderDef,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let revision = catalog.revision;
    let provider_id = id.to_owned();
    let add_model = button(
        cx,
        BtnKind::Soft,
        format!("model-add-{id}"),
        "添加模型",
        t,
        cx.listener(move |daemon, _, window, cx| {
            model_form(daemon, window, cx, revision, provider_id.clone(), None);
            cx.notify();
        }),
    );
    let existing = (id.to_owned(), provider.clone());
    let edit = dim_button(
        cx,
        t,
        format!("provider-edit-{id}"),
        "编辑",
        move |daemon, window, cx| {
            provider_form(daemon, window, cx, revision, Some(existing.clone()), None);
        },
    );
    let remove = dim_confirm_command(
        cx,
        t,
        format!("provider-delete-{id}"),
        format!("删除服务商「{}」及其全部模型？", provider.name),
        revision,
        RingingCommand::ProviderDelete { id: id.into() },
    );
    let meta = format!(
        "{} · {} · {}",
        provider.base_url,
        provider.wire.as_str(),
        credential_label(provider.credential)
    );
    let mut c = card(t)
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap_4()
                .px_4()
                .py_3()
                .bg(t.surface_subtle)
                .border_b_1()
                .border_color(t.border_subtle)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text)
                                        .truncate()
                                        .child(provider.name.clone()),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py(px(1.))
                                        .rounded_full()
                                        .bg(t.btn)
                                        .text_xs()
                                        .text_color(t.dim)
                                        .child(SharedString::from(format!(
                                            "{} 个模型",
                                            provider.models.len()
                                        ))),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .truncate()
                                .child(SharedString::from(meta)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(add_model)
                        .child(edit)
                        .child(remove),
                ),
        );
    for (model_id, model) in &provider.models {
        c = c.child(model_row(catalog, id, model_id, model, cx, t));
    }
    if provider.models.is_empty() {
        c = c.child(
            div()
                .px_4()
                .py_3()
                .text_xs()
                .text_color(t.faint)
                .child("暂无模型，点击右上角「添加模型」即可配置。"),
        );
    }
    c
}

fn model_row(
    catalog: &ProfilesDocument,
    provider_id: &str,
    model_id: &str,
    model: &ModelDef,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let revision = catalog.revision;
    let selection = ModelSelection {
        provider_id: provider_id.into(),
        model_id: model_id.into(),
        options: Default::default(),
    };
    let is_default = catalog
        .default_model_selection
        .as_ref()
        .is_some_and(|m| m.provider_id == provider_id && m.model_id == model_id);
    let id = format!("{provider_id}/{model_id}");
    let valid = qaqh_profile::resolve_model_selection(catalog, &selection).is_ok();
    let existing = (model_id.to_owned(), model.clone());
    let provider = provider_id.to_owned();
    let edit = dim_button(
        cx,
        t,
        format!("model-edit-{id}"),
        "编辑",
        move |daemon, window, cx| {
            model_form(
                daemon,
                window,
                cx,
                revision,
                provider.clone(),
                Some(existing.clone()),
            );
        },
    );
    let remove = dim_confirm_command(
        cx,
        t,
        format!("model-delete-{id}"),
        format!("删除模型「{id}」？"),
        revision,
        RingingCommand::ProviderModelDelete {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        },
    );
    let mut actions = div().flex().items_center().gap_1();
    if valid && !is_default {
        actions = actions.child(dim_button(
            cx,
            t,
            format!("model-default-{id}"),
            "设为默认",
            move |daemon, _, _| {
                let result = catalog_command(
                    daemon,
                    revision,
                    RingingCommand::ModelSelectionSetDefault {
                        selection: selection.clone(),
                    },
                );
                report(daemon, result);
            },
        ));
    }
    let mut head = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .text_sm()
                .text_color(t.text)
                .truncate()
                .child(SharedString::from(
                    model.display_name.as_deref().unwrap_or(model_id),
                )),
        );
    if is_default {
        head = head.child(pill("默认", t.accent, t));
    }
    if !model.enabled {
        head = head.child(pill("已停用", t.warn, t));
    }
    let subtitle = match model.display_name.as_deref() {
        Some(name) if name != model_id => {
            format!("{model_id} · {} ctx", token_label(model.context_window))
        }
        _ => format!("{} ctx", token_label(model.context_window)),
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .px_4()
        .py_2p5()
        .border_t_1()
        .border_color(t.border_subtle)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .min_w_0()
                .child(head)
                .child(
                    div()
                        .font_family("monospace")
                        .text_xs()
                        .text_color(t.dim)
                        .truncate()
                        .child(SharedString::from(subtitle)),
                ),
        )
        .child(actions.child(edit).child(remove))
}

fn patch_session(
    daemon: &DaemonStore,
    cx: &mut Context<DaemonStore>,
    session_id: &str,
    patch: SessionSettingsPatch,
) -> Result<(), String> {
    let current = daemon.current.as_ref().ok_or("请先选择会话")?;
    let store = current.read(cx);
    if store.session_id != session_id || store.settings.is_none() || store.settings_pending {
        return Err("会话设置暂不可写，请确认当前会话并等待同步完成".into());
    }
    current.update(cx, |store, cx| {
        store.patch_settings(patch);
        cx.notify();
    });
    Ok(())
}

fn session(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let Some(store) = daemon.current.as_ref().map(|current| current.read(cx)) else {
        return card_padded(t).child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child("当前未打开任何会话，请先在左侧选择或新建会话。"),
        );
    };
    let Some(settings) = &store.settings else {
        return card_padded(t).child(div().text_xs().text_color(t.dim).child("正在读取会话设置…"));
    };
    let meta = settings.meta.clone();
    let title_text = store.display_title();
    let mut rows = Vec::new();
    let current_model = meta.model_selection.as_ref();

    if let Some(catalog) = &daemon.providers {
        let target = meta.session_id.clone();
        let selected = current_model
            .map(model_label)
            .unwrap_or_else(|| "未选择".into());
        let model_picker = button(
            cx,
            BtnKind::Soft,
            "session-model",
            "更换模型",
            t,
            cx.listener({
                let selected = selected.clone();
                move |daemon, _, _, cx| {
                    let target = target.clone();
                    choose_model(
                        daemon,
                        cx,
                        "选择会话模型",
                        &selected,
                        move |model, daemon, cx| {
                            patch_session(
                                daemon,
                                cx,
                                &target,
                                SessionSettingsPatch {
                                    model_selection: Some(qaqh_types::SettingValue::Use {
                                        value: model,
                                    }),
                                    ..Default::default()
                                },
                            )
                        },
                    );
                    cx.notify();
                }
            }),
        );
        rows.push(setting_row("会话模型", &selected, model_picker, t));

        if let Some((selection, model)) = current_model.and_then(|selection| {
            let model = catalog
                .providers
                .get(&selection.provider_id)?
                .models
                .get(&selection.model_id)?;
            model.capabilities.reasoning.then_some((selection, model))
        }) {
            let selected = selection
                .options
                .reasoning_effort
                .as_deref()
                .unwrap_or("模型默认");
            let levels = std::iter::once("模型默认")
                .chain(model.reasoning_levels.iter().map(String::as_str));
            let controls = levels.map(|level| {
                let target = meta.session_id.clone();
                let mut model = selection.clone();
                model.options.reasoning_effort = (level != "模型默认").then(|| level.to_owned());
                segmented_button(
                    format!("session-effort-{level}"),
                    level.to_owned(),
                    selected == level,
                    t,
                    cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                        let result = patch_session(
                            daemon,
                            cx,
                            &target,
                            SessionSettingsPatch {
                                model_selection: Some(qaqh_types::SettingValue::Use {
                                    value: model.clone(),
                                }),
                                ..Default::default()
                            },
                        );
                        report(daemon, result);
                        cx.notify();
                    }),
                )
            });
            rows.push(setting_row(
                "推理强度",
                "仅对当前支持深度推理的模型生效",
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(controls)
                    .into_any_element(),
                t,
            ));
        }
    }

    let tools = [
        (qaqh_types::STANDARD, "标准"),
        (qaqh_types::MINIMAL, "精简"),
        (qaqh_types::MINIMAL_B, "精简 B"),
        (qaqh_types::MINIMAL_C, "精简 C"),
    ]
    .map(|(preset, label)| {
        let target = meta.session_id.clone();
        let active = meta.tool_mode == preset
            || (meta.tool_mode.is_empty() && preset == qaqh_types::STANDARD);
        segmented_button(
            format!("session-tools-{preset}"),
            label,
            active,
            t,
            cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                let result = patch_session(
                    daemon,
                    cx,
                    &target,
                    SessionSettingsPatch {
                        tools: Some(ToolsSettingPatch::Preset {
                            name: preset.into(),
                        }),
                        ..Default::default()
                    },
                );
                report(daemon, result);
                cx.notify();
            }),
        )
    });
    rows.push(setting_row(
        "工具预设",
        if meta.tool_mode == qaqh_types::CUSTOM {
            "当前正在使用自定义工具名单"
        } else {
            "控制本会话允许代理调用的工具集合"
        },
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(tools)
            .into_any_element(),
        t,
    ));

    let target = meta.session_id.clone();
    let names = meta.custom_tools.join(", ");
    let edit = button(
        cx,
        BtnKind::Soft,
        "session-custom-tools",
        "编辑名单",
        t,
        cx.listener({
            let names = names.clone();
            move |daemon, _, window, cx| {
                let target = target.clone();
                let fields = vec![Field::text(
                    "names",
                    "工具名称（英文逗号分隔）",
                    &names,
                    window,
                    cx,
                )];
                begin(
                    daemon,
                    "自定义会话工具",
                    fields,
                    move |fields, daemon, cx| {
                        let names = list(&fields[0].value(cx));
                        if names.is_empty() {
                            return Err("请至少填写一个工具名称".into());
                        }
                        patch_session(
                            daemon,
                            cx,
                            &target,
                            SessionSettingsPatch {
                                tools: Some(ToolsSettingPatch::Custom { names }),
                                ..Default::default()
                            },
                        )
                    },
                );
                cx.notify();
            }
        }),
    );
    rows.push(setting_row(
        "自定义工具名单",
        if names.is_empty() {
            "未覆盖（沿用预设）"
        } else {
            &names
        },
        edit,
        t,
    ));

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_header(title_text.as_ref(), t))
        .child(section_card("会话参数配置", rows, t))
}

fn choose_model(
    daemon: &mut DaemonStore,
    _cx: &mut Context<DaemonStore>,
    title: &'static str,
    current: &str,
    select: impl Fn(ModelSelection, &mut DaemonStore, &mut Context<DaemonStore>) -> Result<(), String>
        + 'static,
) {
    let Some(catalog) = &daemon.providers else {
        daemon.lines.push("模型目录尚未载入".into());
        return;
    };
    let options = qaqh_gpui_session::store::model_options_from(catalog);
    if options.is_empty() {
        daemon.lines.push("请先在设置中添加并启用服务商模型".into());
        return;
    }
    let fields = vec![Field::choice(
        "model",
        "模型",
        current,
        options.iter().map(model_label),
    )];
    begin(daemon, title, fields, move |fields, daemon, cx| {
        let model = options
            .iter()
            .find(|model| model_label(model) == fields[0].value(cx))
            .cloned()
            .ok_or("请选择可用模型")?;
        select(model, daemon, cx)
    });
}
