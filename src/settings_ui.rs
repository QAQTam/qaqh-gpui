use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Div, Entity, SharedString, Window, div, px};
use gpui_component::input::{Input, InputState};
use gpui_component::switch::Switch;
use gpui_component::Sizable;
use qaqh_client::{
    ProviderUpsertDto, RingingCommand, SecretWrite, SessionSettingsPatch, ToolsSettingPatch,
};
use qaqh_config_api::{ConfigPatch, ExecPatch, SandboxPatch, SubagentPatch};
use qaqh_profile::{ModelDef, ProfilesDocument, ProviderDef};
use qaqh_types::{CredentialSource, ModelSelection, Wire};

use crate::bridge::BridgeRequest;
use crate::store::{DaemonStore, SettingsEditor};
use crate::theme::Palette;
use crate::ui::{action_button, section_header, segmented_button, setting_row};

type Submit = dyn Fn(&[Field], &mut DaemonStore, &mut Context<DaemonStore>) -> Result<(), String>;

pub struct Form {
    title: String,
    fields: Vec<Field>,
    submit: Box<Submit>,
}

struct Field {
    /// 具名 key:submit 闭包按 key 取字段,避免与构造顺序强耦合的魔法下标。
    key: &'static str,
    label: &'static str,
    control: FieldControl,
}

/// 表单控件:自由文本走组件库 Input(密钥用 masked),布尔走 Switch,
/// 少量互斥固定值走分段按钮(Select 的 delegate 模型对三四个固定选项过重)。
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

    /// 布尔字段:渲染为 Switch;submit 侧仍按「是/否」取值(Field::value 映射)。
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

    /// 按具名 key 取字段;缺 key 是表单构造错误,以 Err 上抛而非静默错位。
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

fn button(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    id: impl Into<String>,
    label: &'static str,
    run: impl Fn(&mut DaemonStore, &mut Window, &mut Context<DaemonStore>) + 'static,
) -> AnyElement {
    action_button(cx, 
        id.into(),
        label,
        t.accent,
        cx.listener(move |daemon, _, window: &mut Window, cx| {
            run(daemon, window, cx);
            cx.notify();
        }),
        t,
    )
    .into_any_element()
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

fn workspaces(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let mut body = div().flex().flex_col().gap_3().p_3()
        .child("工作区目录位于 daemon 所在机器。移除工作区只移除分组。")
        .child(button(cx, t, "create-workspace", "添加工作区", |daemon, window, cx| {
            begin(daemon, "添加工作区", vec![Field::text("path", "daemon 侧绝对目录", "", window, cx)], |fields, daemon, cx| {
                send(daemon, BridgeRequest::Command { target: qaqh_client::CommandTarget::Global,
                    command: RingingCommand::WorkspaceCreate { path: fields[0].required(cx)? } })
            });
        }));
    for workspace in &daemon.workspaces {
        let id = workspace.id.clone();
        let title = workspace.title.clone();
        let rename_id = id.clone();
        body = body.child(div().flex().flex_col().gap_2().p_2().border_1().border_color(t.border)
            .child(format!("{} · {}{}", title, workspace.path, if workspace.missing_dir { " · 目录不存在" } else { "" }))
            .child(div().flex().gap_2()
                .child(button(cx, t, format!("rename-workspace-{id}"), "重命名", move |daemon, window, cx| {
                    let id = rename_id.clone();
                    begin(daemon, "重命名工作区", vec![Field::text("title", "名称", &title, window, cx)], move |fields, daemon, cx| {
                        send(daemon, BridgeRequest::Command { target: qaqh_client::CommandTarget::Workspace { workspace_id: id.clone() },
                            command: RingingCommand::WorkspaceRename { title: fields[0].required(cx)? } })
                    });
                }))
                .child(button(cx, t, format!("remove-workspace-{id}"), "移除分组", move |daemon, _, _| {
                    let result = send(daemon, BridgeRequest::Command { target: qaqh_client::CommandTarget::Workspace { workspace_id: id.clone() }, command: RingingCommand::WorkspaceDelete {} });
                    report(daemon, result);
                }))));
    }
    body
}

/// 账号页:siwc(ChatGPT 套餐)与 dsaccount(DeepSeek 账号)两卡;
/// 全部经 service_call 白名单方法透传,零自建协议。
fn accounts(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let mut body = div()
        .flex()
        .flex_col()
        .gap_3()
        .p_3()
        .child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child("登录走系统浏览器，凭据保存在本机账号管理器，不进入 daemon 配置。"),
        )
        .child(siwc_card(daemon, cx, t))
        .child(dsaccount_card(daemon, cx, t));
    if let Some(line) = &daemon.account_line {
        body = body.child(div().text_xs().text_color(t.warn).child(line.clone()));
    }
    body
}

fn siwc_card(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    use qaqh_client::service_types as st;
    let card = div()
        .flex()
        .flex_col()
        .gap_2()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(t.border);
    let Some(status) = &daemon.siwc else {
        return card.child(div().text_xs().text_color(t.dim).child("ChatGPT 套餐：读取中…"));
    };
    let (state, color) = match status.status {
        st::SiwcConnectionState::Connected if status.sharing => ("已登录，套餐直连可用", t.ok),
        st::SiwcConnectionState::Connected => ("已登录，未授予套餐直连", t.warn),
        st::SiwcConnectionState::Connecting => ("登录流程进行中", t.warn),
        st::SiwcConnectionState::ReauthRequired => ("凭据已失效，需要重新登录", t.bad),
        st::SiwcConnectionState::Disconnected => ("未登录", t.dim),
    };
    let who = status
        .email
        .as_deref()
        .or(status.name.as_deref())
        .unwrap_or_default();
    let mut card = card
        .child(div().text_sm().child("ChatGPT 套餐（SIWC）"))
        .child(
            div()
                .text_xs()
                .text_color(color)
                .child(SharedString::from(if who.is_empty() {
                    state.to_string()
                } else {
                    format!("{state} · {who}")
                })),
        );
    let signing = daemon
        .sign_in
        .as_ref()
        .is_some_and(|flow| flow.service == crate::store::AccountService::Siwc);
    if signing {
        card = card.child(button(cx, t, "siwc-signin-cancel", "取消登录", move |daemon, _, _| {
            daemon.sign_in_cancel();
        }));
    } else {
        match status.status {
            st::SiwcConnectionState::Connected | st::SiwcConnectionState::ReauthRequired => {
                card = card
                    .child(button(cx, t, "siwc-usage", "打开用量页", |daemon, _, cx| {
                        if let Some(status) = &daemon.siwc {
                            cx.open_url(&status.usage_url);
                        }
                    }))
                    .child(button(cx, t, "siwc-signout", "登出", move |daemon, _, _| {
                        daemon.sign_out(crate::store::AccountService::Siwc);
                    }));
                if status.status == st::SiwcConnectionState::ReauthRequired {
                    card = card.child(button(cx, t, "siwc-signin", "重新登录", move |daemon, _, _| {
                        daemon.sign_in_start(crate::store::AccountService::Siwc);
                    }));
                }
            }
            _ => {
                card = card.child(button(cx, t, "siwc-signin", "登录 ChatGPT 套餐", move |daemon, _, _| {
                    daemon.sign_in_start(crate::store::AccountService::Siwc);
                }));
            }
        }
    }
    card
}

fn dsaccount_card(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let card = div()
        .flex()
        .flex_col()
        .gap_2()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(t.border);
    let Some(status) = &daemon.dsaccount else {
        return card.child(div().text_xs().text_color(t.dim).child("DeepSeek 账号：读取中…"));
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
    let mut card = card
        .child(div().text_sm().child("DeepSeek 账号"))
        .child(
            div()
                .text_xs()
                .text_color(color)
                .child(SharedString::from(if who.is_empty() {
                    state.to_string()
                } else {
                    format!("{state} · {who}")
                })),
        );
    if let Some(error) = &status.storage_error {
        card = card.child(
            div()
                .text_xs()
                .text_color(t.bad)
                .child(SharedString::from(format!("本地凭据读取失败:{error}"))),
        );
    }
    let signing = daemon
        .sign_in
        .as_ref()
        .is_some_and(|flow| flow.service == crate::store::AccountService::Dsaccount);
    if signing {
        card = card.child(button(cx, t, "ds-signin-cancel", "取消登录", move |daemon, _, _| {
            daemon.sign_in_cancel();
        }));
    } else if status.signed_in {
        card = card
            .child(button(cx, t, "ds-topup", "充值", |daemon, _, cx| {
                if let Some(status) = &daemon.dsaccount {
                    cx.open_url(&status.top_up_url);
                }
            }))
            .child(button(cx, t, "ds-signout", "登出", move |daemon, _, _| {
                daemon.sign_out(crate::store::AccountService::Dsaccount);
            }));
    } else {
        card = card.child(button(cx, t, "ds-signin", "登录 DeepSeek 账号", move |daemon, _, _| {
            daemon.sign_in_start(crate::store::AccountService::Dsaccount);
        }));
    }
    if let Some(balance) = &daemon.ds_balance {
        for wallet in &balance.recharge {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(t.dim)
                    .child(SharedString::from(format!(
                        "充值 {} {}",
                        wallet.balance, wallet.currency
                    ))),
            );
        }
        for wallet in &balance.bonus {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(t.warn)
                    .child(SharedString::from(format!(
                        "赠金 {} {}",
                        wallet.balance, wallet.currency
                    ))),
            );
        }
        for cost in &balance.total_cost {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(t.dim)
                    .child(SharedString::from(format!(
                        "累计消费 {} {}",
                        cost.amount, cost.currency
                    ))),
            );
        }
        if !balance.has_usable_balance {
            card = card.child(div().text_xs().text_color(t.bad).child("没有可用余额"));
        }
    }
    card
}

fn credential_label(credential: qaqh_types::CredentialSource) -> &'static str {
    match credential {
        qaqh_types::CredentialSource::ApiKey => "API 密钥",
        qaqh_types::CredentialSource::ChatgptPlan => "ChatGPT 套餐",
        qaqh_types::CredentialSource::DeepseekAccount => "DeepSeek 账号",
    }
}

fn report(daemon: &mut DaemonStore, result: Result<(), String>) {
    if let Err(error) = result {
        daemon.lines.push(error);
    }
}

pub fn navigation(active: SettingsEditor, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let tabs = [
        (SettingsEditor::Local, "本机偏好", "local"),
        (SettingsEditor::Config, "全局策略", "global"),
        (SettingsEditor::Catalog, "服务商与模型", "catalog"),
        (SettingsEditor::Accounts, "账号", "accounts"),
        (SettingsEditor::Session, "当前会话", "session"),
        (SettingsEditor::Workspaces, "工作区", "workspaces"),
    ]
    .into_iter()
    .map(|(mode, label, id)| {
        segmented_button(
            format!("daemon-tab-{id}"),
            label,
            active == mode,
            t,
            cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                if daemon.settings_editor != mode {
                    daemon.settings_form = None;
                    daemon.settings_editor = mode;
                    // 账号页每次进入都重拉状态(管理器状态可在窗口外变化)。
                    if mode == SettingsEditor::Accounts {
                        daemon.refresh_accounts();
                    }
                }
                cx.notify();
            }),
        )
    })
    .collect::<Vec<_>>();
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .px(px(24.))
        .py_2()
        .border_b_1()
        .border_color(t.border)
        .children(tabs)
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
        SettingsEditor::Local => div(),
        SettingsEditor::Config => global(daemon, cx, t),
        SettingsEditor::Catalog => catalog(daemon, cx, t),
        SettingsEditor::Accounts => accounts(daemon, cx, t),
        SettingsEditor::Session => session(daemon, cx, t),
        SettingsEditor::Workspaces => workspaces(daemon, cx, t),
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
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(t.border)
        .child(div().text_sm().child(form.title.clone()));
    for (index, field) in form.fields.iter().enumerate() {
        let control = match &field.control {
            FieldControl::Input(state) => {
                div().w(px(300.)).child(Input::new(state).small()).into_any_element()
            }
            FieldControl::Switch(checked) => {
                // Switch 是受控组件:回写进表单字段,submit 仍按「是/否」读值。
                let entity = cx.entity();
                Switch::new(SharedString::from(format!("settings-switch-{index}")))
                    .checked(*checked)
                    .on_change(move |checked: &bool, _, cx: &mut gpui::App| {
                        entity.update(cx, |daemon, cx| {
                            if let Some(form) = &mut daemon.settings_form
                                && let FieldControl::Switch(current) = &mut form.fields[index].control
                            {
                                *current = *checked;
                            }
                            cx.notify();
                        });
                    })
                    .into_any_element()
            }
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
    }
    body.child(
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(button(cx, t, "settings-cancel", "取消", move |daemon, _, _| {
                daemon.settings_form = None
            }))
            .child(button(
                cx,
                t,
                "settings-save",
                if form.fields.is_empty() {
                    "确认"
                } else {
                    "保存"
                },
                |daemon, _, cx| {
                    if let Some(form) = daemon.settings_form.take()
                        && let Err(error) = (form.submit)(&form.fields, daemon, cx)
                    {
                        daemon.settings_form = Some(form);
                        daemon.lines.push(error);
                    }
                },
            )),
    )
}

fn edit_config(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    label: &'static str,
    value: impl ToString,
    patch: impl Fn(&Field, &App) -> Result<ConfigPatch, String> + Clone + 'static,
) -> Div {
    let value = value.to_string();
    let control = button(cx, t, format!("config-{label}"), "编辑", {
        let value = value.clone();
        move |daemon, window, cx| {
            let field = Field::text("value", label, &value, window, cx);
            let patch = patch.clone();
            begin(daemon, label, vec![field], move |fields, daemon, cx| {
                save(daemon, patch(&fields[0], cx)?)
            });
        }
    });
    setting_row(label, &value, control, t)
}

fn toggle_config(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    label: &'static str,
    current: bool,
    patch: impl Fn(bool) -> ConfigPatch + 'static,
) -> Div {
    let controls = [(false, "关闭"), (true, "开启")].map(|(value, title)| {
        let patch = patch(value);
        segmented_button(
            format!("config-{label}-{value}"),
            title,
            current == value,
            t,
            cx.listener(move |daemon: &mut DaemonStore, _, _, _| {
                if current != value {
                    let result = save(daemon, patch.clone());
                    report(daemon, result);
                }
            }),
        )
    });
    setting_row(
        label,
        "",
        div().flex().gap_1().children(controls).into_any_element(),
        t,
    )
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn global(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let Some(config) = &daemon.config else {
        return div().child("正在读取 daemon 配置…");
    };
    macro_rules! number {
        ($label:literal, $field:ident) => {
            edit_config(cx, t, $label, config.$field, |field, cx| {
                Ok(ConfigPatch {
                    $field: Some(field.number(cx)?),
                    ..Default::default()
                })
            })
        };
    }
    let mut body = div()
        .flex()
        .flex_col()
        .child(section_header("运行策略", t))
        .child(crate::ui::permission_row(daemon, cx, t))
        .child(toggle_config(
            cx,
            t,
            "命令沙箱",
            config.sandbox.enabled,
            |enabled| ConfigPatch {
                sandbox: Some(SandboxPatch {
                    enabled: Some(enabled),
                }),
                ..Default::default()
            },
        ))
        .child(toggle_config(
            cx,
            t,
            "内容过滤",
            config.compliance_enabled,
            |enabled| ConfigPatch {
                compliance_enabled: Some(enabled),
                ..Default::default()
            },
        ))
        .child(toggle_config(
            cx,
            t,
            "全局工具白名单",
            config.tool_allowlist_enabled,
            |enabled| ConfigPatch {
                tool_allowlist_enabled: Some(enabled),
                ..Default::default()
            },
        ))
        .child(edit_config(
            cx,
            t,
            "允许的工具（逗号分隔）",
            config.tool_allowlist.join(", "),
            |field, cx| {
                Ok(ConfigPatch {
                    tool_allowlist: Some(list(&field.value(cx))),
                    ..Default::default()
                })
            },
        ))
        .child(number!("自动压缩阈值（0–1）", auto_compact_threshold))
        .child(number!(
            "空闲卸载（秒，0 为关闭）",
            session_idle_unload_secs
        ))
        .child(edit_config(
            cx,
            t,
            "默认 Shell",
            config.exec.default_shell.as_deref().unwrap_or("auto"),
            |field, cx| {
                Ok(ConfigPatch {
                    exec: Some(ExecPatch {
                        default_shell: Some(field.value(cx)),
                    }),
                    ..Default::default()
                })
            },
        ))
        .child(edit_config(
            cx,
            t,
            "Tokenizer 路径（重启后生效）",
            config.tokenizer_path.as_deref().unwrap_or_default(),
            |field, cx| {
                Ok(ConfigPatch {
                    tokenizer_path: Some(field.value(cx)),
                    ..Default::default()
                })
            },
        ))
        .child(section_header("子代理", t));
    macro_rules! subagent {
        ($label:literal, $field:ident, $read:expr, $convert:expr) => {
            edit_config(cx, t, $label, $read, |field, cx| {
                Ok(ConfigPatch {
                    subagent: Some(SubagentPatch {
                        $field: Some(($convert)(field, cx)?),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            })
        };
    }
    body = body
        .child(subagent!(
            "默认模型",
            model,
            &config.subagent.model,
            |f: &Field, cx| Ok::<_, String>(f.value(cx))
        ))
        .child(subagent!(
            "端点 URL",
            base_url,
            &config.subagent.base_url,
            |f: &Field, cx| Ok::<_, String>(f.value(cx))
        ))
        .child(subagent!(
            "输出 Token 上限",
            max_tokens,
            config.subagent.max_tokens,
            Field::number
        ))
        .child(subagent!(
            "超时（秒）",
            timeout_secs,
            config.subagent.timeout_secs,
            Field::number
        ))
        .child(subagent!(
            "最大深度（重启后生效）",
            max_depth,
            config.subagent.max_depth,
            Field::number
        ))
        .child(subagent!(
            "每对会话排队上限",
            message_in_flight_per_pair,
            config.subagent.message_in_flight_per_pair,
            Field::number
        ))
        .child(subagent!(
            "每个发送者消息上限",
            message_outbound_per_sender,
            config.subagent.message_outbound_per_sender,
            Field::number
        ))
        .child(subagent!(
            "默认工具（逗号分隔）",
            default_tools,
            config.subagent.default_tools.join(", "),
            |f: &Field, cx| Ok::<_, String>(list(&f.value(cx)))
        ))
        .child(setting_row(
            "子代理 API 密钥",
            if config.subagent.api_key_set {
                "已配置"
            } else {
                "未配置"
            },
            button(cx, t, "subagent-key", "更换", |daemon, window, cx| {
                let fields = vec![Field::secret("api_key", "新 API 密钥", window, cx)];
                begin(
                    daemon,
                    "更换子代理 API 密钥",
                    fields,
                    |fields, daemon, cx| {
                        save(
                            daemon,
                            ConfigPatch {
                                subagent: Some(SubagentPatch {
                                    api_key: Some(fields[0].required(cx)?),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        )
                    },
                );
            }),
            t,
        ))
        .child(section_header("工具集成 · 只读", t));
    for (name, enabled, servers) in [
        (
            "MCP",
            config.mcp.enabled,
            config
                .mcp
                .servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
        ),
        (
            "LSP",
            config.lsp.enabled,
            config
                .lsp
                .servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
        ),
    ] {
        let status = if enabled { "已启用" } else { "已停用" };
        body = body.child(setting_row(
            name,
            &servers.join("、"),
            div().child(status).into_any_element(),
            t,
        ));
    }
    body
}

fn provider_form(
    daemon: &mut DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    revision: u64,
    existing: Option<(String, ProviderDef)>,
) {
    let provider = existing.as_ref().map(|(_, provider)| provider);
    let wire = provider
        .map(|provider| provider.wire.as_str())
        .unwrap_or("chat");
    let credential = credential_label(
        provider
            .map(|provider| provider.credential)
            .unwrap_or_default(),
    );
    let mut fields = Vec::new();
    if existing.is_none() {
        fields.push(Field::text("provider_id", "服务商 ID", "", window, cx));
    }
    fields.extend([
        Field::text(
            "name",
            "显示名称",
            provider.map(|p| p.name.as_str()).unwrap_or_default(),
            window,
            cx,
        ),
        Field::text(
            "base_url",
            "端点 URL",
            provider.map(|p| p.base_url.as_str()).unwrap_or_default(),
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
        Field::text("context_window", "上下文窗口", model.context_window, window, cx),
        Field::text("max_output_tokens", "输出 Token 上限", model.max_output_tokens, window, cx),
        Field::switch("enabled", "启用", model.enabled),
        Field::switch("tool_calls", "工具调用", model.capabilities.tool_calls),
        Field::switch("image_input", "图片输入", model.capabilities.image_input),
        Field::switch("native_web_search", "原生网页搜索", model.capabilities.native_web_search),
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
        Field::switch("default_native_web_search", "默认启用原生搜索", model.defaults.native_web_search),
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

fn optional(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn confirm_command(
    cx: &mut Context<DaemonStore>,
    t: &Palette,
    id: String,
    title: String,
    revision: u64,
    command: RingingCommand,
) -> AnyElement {
    button(cx, t, id, "删除", move |daemon, _, _| {
        let command = command.clone();
        begin(daemon, title.clone(), vec![], move |_, daemon, _| {
            catalog_command(daemon, revision, command.clone())
        });
    })
}

fn catalog(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let Some(catalog) = daemon.providers.as_deref() else {
        return div().child("正在读取模型目录…");
    };
    let revision = catalog.revision;
    let add = button(
        cx,
        t,
        "provider-add",
        "添加服务商",
        move |daemon, window, cx| provider_form(daemon, window, cx, revision, None),
    );
    let mut body =
        div()
            .flex()
            .flex_col()
            .child(setting_row("服务商目录", "管理模型端点与凭据", add, t));
    for (id, provider) in &catalog.providers {
        let provider_id = id.clone();
        let add_model = button(
            cx,
            t,
            format!("model-add-{id}"),
            "添加模型",
            move |daemon, window, cx| {
                model_form(daemon, window, cx, revision, provider_id.clone(), None);
            },
        );
        let existing = (id.clone(), provider.clone());
        let edit = button(
            cx,
            t,
            format!("provider-edit-{id}"),
            "编辑",
            move |daemon, window, cx| {
                provider_form(daemon, window, cx, revision, Some(existing.clone()));
            },
        );
        let remove = confirm_command(
            cx,
            t,
            format!("provider-delete-{id}"),
            format!("删除服务商「{}」及其模型？", provider.name),
            revision,
            RingingCommand::ProviderDelete { id: id.clone() },
        );
        let desc = format!(
            "{} · 凭据:{}",
            provider.base_url,
            credential_label(provider.credential)
        );
        body = body.child(setting_row(
            &provider.name,
            &desc,
            div()
                .flex()
                .gap_1()
                .child(add_model)
                .child(edit)
                .child(remove)
                .into_any_element(),
            t,
        ));
        for (model_id, model) in &provider.models {
            body = body.child(model_row(catalog, id, model_id, model, cx, t));
        }
    }
    body.child(profiles(daemon, revision, cx, t))
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
    let edit = button(
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
    let remove = confirm_command(
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
    let mut actions = div().flex().gap_1();
    if valid && !is_default {
        actions = actions.child(button(
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
    let provider = provider_id.to_owned();
    let model_id = model_id.to_owned();
    actions = actions.child(button(
        cx,
        t,
        format!("model-profile-{id}"),
        "创建预设",
        move |daemon, window, cx| {
            let fields = vec![
                Field::text("name", "预设名称", "", window, cx),
                Field::secret("api_key", "API 密钥（可选）", window, cx),
            ];
            let provider = provider.clone();
            let model = model_id.clone();
            begin(
                daemon,
                "从模型创建预设",
                fields,
                move |fields, daemon, cx| {
                    catalog_command(
                        daemon,
                        revision,
                        RingingCommand::ProfileCreate {
                            name: fields[0].required(cx)?,
                            model: model.clone(),
                            id: None,
                            provider: Some(provider.clone()),
                            base_url: None,
                            wire: None,
                            reasoning_effort: None,
                            context_length: None,
                            max_tokens: None,
                            api_key: optional(&fields[1].value(cx)),
                        },
                    )
                },
            );
        },
    ));
    setting_row(
        model.display_name.as_deref().unwrap_or(&id),
        if is_default {
            "全局默认模型"
        } else if !model.enabled {
            "已停用"
        } else {
            &id
        },
        actions.child(edit).child(remove).into_any_element(),
        t,
    )
}

fn profiles(
    daemon: &DaemonStore,
    revision: u64,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let mut body = div().flex().flex_col().child(section_header("运行预设", t));
    let Some(config) = &daemon.config else {
        return body;
    };
    body = body.child(setting_row(
        "保存当前配置",
        "保存当前 daemon 配置为命名预设",
        button(
            cx,
            t,
            "profile-save-current",
            "另存为",
            move |daemon, window, cx| {
                let fields = vec![Field::text("name", "预设名称", "", window, cx)];
                begin(
                    daemon,
                    "保存当前配置",
                    fields,
                    move |fields, daemon, cx| {
                        catalog_command(
                            daemon,
                            revision,
                            RingingCommand::ProfileSaveCurrent {
                                name: fields[0].required(cx)?,
                            },
                        )
                    },
                );
            },
        ),
        t,
    ));
    for name in &config.profiles {
        let id = name.clone();
        let apply = button(
            cx,
            t,
            format!("profile-apply-{name}"),
            "设为当前",
            move |daemon, _, _| {
                let result = catalog_command(
                    daemon,
                    revision,
                    RingingCommand::ProfileApply { name: id.clone() },
                );
                report(daemon, result);
            },
        );
        let remove = confirm_command(
            cx,
            t,
            format!("profile-delete-{name}"),
            format!("删除预设「{name}」？"),
            revision,
            RingingCommand::ProfileDelete { name: name.clone() },
        );
        body = body.child(setting_row(
            name,
            if config.active_profile == *name {
                "当前全局预设"
            } else {
                ""
            },
            div()
                .flex()
                .gap_1()
                .child(apply)
                .child(remove)
                .into_any_element(),
            t,
        ));
    }
    body
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
        return div().child("请先选择会话");
    };
    let Some(settings) = &store.settings else {
        return div().child("正在读取会话设置…");
    };
    let meta = settings.meta.clone();
    let mut body = div().flex().flex_col().child(section_header(
        meta.title.as_deref().unwrap_or("当前会话"),
        t,
    ));
    let current_model = meta.model_selection.as_ref();
    if let Some(catalog) = &daemon.providers {
        let target = meta.session_id.clone();
        let selected = current_model
            .map(model_label)
            .unwrap_or_else(|| "未选择".into());
        let model_picker = button(cx, t, "session-model", "选择模型", {
            let selected = selected.clone();
            move |daemon, _, cx| {
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
            }
        });
        body = body.child(setting_row("会话模型", &selected, model_picker, t));
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
            body = body.child(setting_row(
                "推理强度",
                "",
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
    let modes = [("code", "Code", 0), ("plan", "Plan", 1)].map(|(mode, label, id)| {
        let target = meta.session_id.clone();
        let active = (meta.mode == 1) == (id == 1);
        segmented_button(
            format!("session-mode-{mode}"),
            label,
            active,
            t,
            cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                let result = patch_session(
                    daemon,
                    cx,
                    &target,
                    SessionSettingsPatch {
                        mode: Some(mode.into()),
                        ..Default::default()
                    },
                );
                report(daemon, result);
                cx.notify();
            }),
        )
    });
    body = body.child(setting_row(
        "会话模式",
        "",
        div().flex().gap_1().children(modes).into_any_element(),
        t,
    ));
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
    body = body.child(setting_row(
        "工具预设",
        if meta.tool_mode == qaqh_types::CUSTOM {
            "当前使用自定义名单"
        } else {
            ""
        },
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(tools)
            .into_any_element(),
        t,
    ));
    let Some(config) = &daemon.config else {
        return body;
    };
    let profiles = std::iter::once("")
        .chain(config.profiles.iter().map(String::as_str))
        .map(|profile| {
            let target = meta.session_id.clone();
            let value = profile.to_owned();
            segmented_button(
                format!("session-profile-{profile}"),
                if profile.is_empty() {
                    "跟随全局".to_owned()
                } else {
                    profile.to_owned()
                },
                meta.profile.as_deref().unwrap_or_default() == profile,
                t,
                cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                    let result = patch_session(
                        daemon,
                        cx,
                        &target,
                        SessionSettingsPatch {
                            profile: Some(value.clone()),
                            ..Default::default()
                        },
                    );
                    report(daemon, result);
                    cx.notify();
                }),
            )
        });
    body = body.child(setting_row(
        "会话预设",
        "",
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(profiles)
            .into_any_element(),
        t,
    ));
    let target = meta.session_id.clone();
    let names = meta.custom_tools.join(", ");
    let edit = button(cx, t, "session-custom-tools", "编辑", {
        let names = names.clone();
        move |daemon, window, cx| {
            let target = target.clone();
            let fields = vec![Field::text("names", "工具名称（逗号分隔）", &names, window, cx)];
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
        }
    });
    body.child(setting_row("自定义工具", &names, edit, t))
}

pub fn choose_new_model(daemon: &mut DaemonStore, cx: &mut Context<DaemonStore>) {
    let current = daemon
        .new_model
        .as_ref()
        .map(model_label)
        .unwrap_or_default();
    choose_model(
        daemon,
        cx,
        "选择新会话模型",
        &current,
        |model, daemon, _| {
            if daemon
                .new_model
                .as_ref()
                .is_none_or(|current| model_label(current) != model_label(&model))
            {
                daemon.new_model = Some(model);
            }
            Ok(())
        },
    );
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
    let options = crate::store::model_options_from(catalog);
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

fn model_label(model: &ModelSelection) -> String {
    format!("{} / {}", model.provider_id, model.model_id)
}
