use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FocusHandle, KeyDownEvent, Window, div, px};
use qaqh_client::{
    ProviderUpsertDto, RingingCommand, SecretWrite, SessionSettingsPatch, ToolsSettingPatch,
};
use qaqh_config_api::{ConfigPatch, ExecPatch, SandboxPatch, SubagentPatch};
use qaqh_profile::{ModelDef, ProfilesDocument, ProviderDef};
use qaqh_types::{CredentialSource, ModelSelection, Wire};

use crate::bridge::BridgeRequest;
use crate::store::{DaemonStore, SettingsEditor};
use crate::text_input::{InputElement, TextField};
use crate::theme::Palette;
use crate::ui::{
    action_button, basic_field_key, field_box, section_header, segmented_button, setting_row,
};

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
    input: TextField,
    focus: FocusHandle,
    secret: bool,
    choices: Vec<String>,
}

impl Form {
    pub fn focused_field(&self, window: &Window) -> Option<&TextField> {
        self.fields
            .iter()
            .find(|field| field.focus.is_focused(window))
            .map(|field| &field.input)
    }

    pub fn focused_field_mut(&mut self, window: &Window) -> Option<&mut TextField> {
        self.fields
            .iter_mut()
            .find(|field| field.focus.is_focused(window))
            .map(|field| &mut field.input)
    }
}

impl Field {
    fn text(
        key: &'static str,
        label: &'static str,
        value: impl ToString,
        cx: &mut Context<DaemonStore>,
    ) -> Self {
        Self {
            key,
            label,
            input: TextField::with_text(&value.to_string()),
            focus: cx.focus_handle(),
            secret: false,
            choices: Vec::new(),
        }
    }

    fn secret(key: &'static str, label: &'static str, cx: &mut Context<DaemonStore>) -> Self {
        Self {
            secret: true,
            ..Self::text(key, label, "", cx)
        }
    }

    fn choice(
        key: &'static str,
        label: &'static str,
        value: &str,
        choices: impl IntoIterator<Item = impl Into<String>>,
        cx: &mut Context<DaemonStore>,
    ) -> Self {
        Self {
            choices: choices.into_iter().map(Into::into).collect(),
            ..Self::text(key, label, value, cx)
        }
    }

    /// 按具名 key 取字段;缺 key 是表单构造错误,以 Err 上抛而非静默错位。
    fn find<'a>(fields: &'a [Field], key: &str) -> Result<&'a Field, String> {
        fields
            .iter()
            .find(|field| field.key == key)
            .ok_or_else(|| format!("内部错误：表单缺少字段 {key}"))
    }

    fn value(&self) -> &str {
        self.input.text.trim()
    }

    fn number<T: std::str::FromStr>(&self) -> Result<T, String> {
        self.value()
            .parse()
            .map_err(|_| format!("{}的数字格式无效", self.label))
    }

    fn required(&self) -> Result<String, String> {
        if self.value().is_empty() {
            return Err(format!("请填写{}", self.label));
        }
        Ok(self.value().to_owned())
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
    run: impl Fn(&mut DaemonStore, &mut Context<DaemonStore>) + 'static,
) -> AnyElement {
    action_button(
        id.into(),
        label,
        t.accent,
        cx.listener(move |daemon, _, _, cx| {
            run(daemon, cx);
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
        .child(button(cx, t, "create-workspace", "添加工作区", |daemon, cx| {
            begin(daemon, "添加工作区", vec![Field::text("path", "daemon 侧绝对目录", "", cx)], |fields, daemon, _| {
                send(daemon, BridgeRequest::Command { target: qaqh_client::CommandTarget::Global,
                    command: RingingCommand::WorkspaceCreate { path: fields[0].required()? } })
            });
        }));
    for workspace in &daemon.workspaces {
        let id = workspace.id.clone();
        let title = workspace.title.clone();
        let rename_id = id.clone();
        body = body.child(div().flex().flex_col().gap_2().p_2().border_1().border_color(t.border)
            .child(format!("{} · {}{}", title, workspace.path, if workspace.missing_dir { " · 目录不存在" } else { "" }))
            .child(div().flex().gap_2()
                .child(button(cx, t, format!("rename-workspace-{id}"), "重命名", move |daemon, cx| {
                    let id = rename_id.clone();
                    begin(daemon, "重命名工作区", vec![Field::text("title", "名称", &title, cx)], move |fields, daemon, _| {
                        send(daemon, BridgeRequest::Command { target: qaqh_client::CommandTarget::Workspace { workspace_id: id.clone() },
                            command: RingingCommand::WorkspaceRename { title: fields[0].required()? } })
                    });
                }))
                .child(button(cx, t, format!("remove-workspace-{id}"), "移除分组", move |daemon, _| {
                    let result = send(daemon, BridgeRequest::Command { target: qaqh_client::CommandTarget::Workspace { workspace_id: id.clone() }, command: RingingCommand::WorkspaceDelete {} });
                    report(daemon, result);
                }))));
    }
    body
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
    window: &Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    if let Some(form) = &daemon.settings_form {
        return render_form(form, window, cx, t);
    }
    match daemon.settings_editor {
        SettingsEditor::Local => div(),
        SettingsEditor::Config => global(daemon, cx, t),
        SettingsEditor::Catalog => catalog(daemon, cx, t),
        SettingsEditor::Session => session(daemon, cx, t),
        SettingsEditor::Workspaces => workspaces(daemon, cx, t),
    }
}

pub(crate) fn render_form(
    form: &Form,
    window: &Window,
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
        let focused = field.focus.is_focused(window);
        let control = if field.choices.is_empty() {
            let content = if field.secret {
                div().child("•".repeat(field.input.text.chars().count()))
            } else {
                div().child(field.input.display(focused, window))
            };
            let input = field_box(
                format!("settings-field-{index}"),
                &field.focus,
                if focused { t.accent } else { t.border },
                cx.listener(move |daemon: &mut DaemonStore, _, window, cx| {
                    if let Some(form) = &daemon.settings_form {
                        window.focus(&form.fields[index].focus, cx);
                    }
                }),
                cx.listener(
                    move |daemon: &mut DaemonStore, event: &KeyDownEvent, _, cx| {
                        if let Some(form) = &mut daemon.settings_form {
                            basic_field_key(&mut form.fields[index].input, event, cx);
                        }
                    },
                ),
            )
            .w(px(300.))
            .child(content);
            InputElement::new(cx.entity(), field.focus.clone(), input).into_any_element()
        } else {
            let choices = field.choices.iter().map(|choice| {
                segmented_button(
                    format!("settings-choice-{index}-{choice}"),
                    choice.clone(),
                    field.value() == choice,
                    t,
                    {
                        let choice = choice.clone();
                        cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                            if let Some(form) = &mut daemon.settings_form {
                                form.fields[index].input = TextField::with_text(&choice);
                                cx.notify();
                            }
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
        };
        body = body.child(setting_row(field.label, "", control, t));
    }
    body.child(
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(button(cx, t, "settings-cancel", "取消", |daemon, _| {
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
                |daemon, cx| {
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
    patch: impl Fn(&Field) -> Result<ConfigPatch, String> + Clone + 'static,
) -> Div {
    let value = value.to_string();
    let control = button(cx, t, format!("config-{label}"), "编辑", {
        let value = value.clone();
        move |daemon, cx| {
            let field = Field::text("value", label, &value, cx);
            let patch = patch.clone();
            begin(daemon, label, vec![field], move |fields, daemon, _| {
                save(daemon, patch(&fields[0])?)
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
            edit_config(cx, t, $label, config.$field, |field| {
                Ok(ConfigPatch {
                    $field: Some(field.number()?),
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
            |field| {
                Ok(ConfigPatch {
                    tool_allowlist: Some(list(field.value())),
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
            |field| {
                Ok(ConfigPatch {
                    exec: Some(ExecPatch {
                        default_shell: Some(field.value().to_owned()),
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
            |field| {
                Ok(ConfigPatch {
                    tokenizer_path: Some(field.value().to_owned()),
                    ..Default::default()
                })
            },
        ))
        .child(section_header("子代理", t));
    macro_rules! subagent {
        ($label:literal, $field:ident, $read:expr, $convert:expr) => {
            edit_config(cx, t, $label, $read, |field| {
                Ok(ConfigPatch {
                    subagent: Some(SubagentPatch {
                        $field: Some(($convert)(field)?),
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
            |f: &Field| Ok::<_, String>(f.value().to_owned())
        ))
        .child(subagent!(
            "端点 URL",
            base_url,
            &config.subagent.base_url,
            |f: &Field| Ok::<_, String>(f.value().to_owned())
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
            |f: &Field| Ok::<_, String>(list(f.value()))
        ))
        .child(setting_row(
            "子代理 API 密钥",
            if config.subagent.api_key_set {
                "已配置"
            } else {
                "未配置"
            },
            button(cx, t, "subagent-key", "更换", |daemon, cx| {
                let fields = vec![Field::secret("api_key", "新 API 密钥", cx)];
                begin(
                    daemon,
                    "更换子代理 API 密钥",
                    fields,
                    |fields, daemon, _| {
                        save(
                            daemon,
                            ConfigPatch {
                                subagent: Some(SubagentPatch {
                                    api_key: Some(fields[0].required()?),
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
    cx: &mut Context<DaemonStore>,
    revision: u64,
    existing: Option<(String, ProviderDef)>,
) {
    let provider = existing.as_ref().map(|(_, provider)| provider);
    let wire = provider
        .map(|provider| provider.wire.as_str())
        .unwrap_or("chat");
    let credential = match provider
        .map(|provider| provider.credential)
        .unwrap_or_default()
    {
        CredentialSource::ApiKey => "API 密钥",
        CredentialSource::ChatgptPlan => "ChatGPT 套餐",
        CredentialSource::DeepseekAccount => "DeepSeek 账号",
    };
    let mut fields = Vec::new();
    if existing.is_none() {
        fields.push(Field::text("provider_id", "服务商 ID", "", cx));
    }
    fields.extend([
        Field::text(
            "name",
            "显示名称",
            provider.map(|p| p.name.as_str()).unwrap_or_default(),
            cx,
        ),
        Field::text(
            "base_url",
            "端点 URL",
            provider.map(|p| p.base_url.as_str()).unwrap_or_default(),
            cx,
        ),
        Field::choice(
            "wire",
            "接口协议",
            wire,
            ["chat", "responses", "messages", "gemini"],
            cx,
        ),
        Field::choice(
            "credential",
            "凭据来源",
            credential,
            ["API 密钥", "ChatGPT 套餐", "DeepSeek 账号"],
            cx,
        ),
        Field::secret("api_key", "新 API 密钥（留空保留）", cx),
        Field::choice("clear_key", "清除已存密钥", "否", ["否", "是"], cx),
    ]);
    let title = existing
        .as_ref()
        .map(|(id, _)| format!("编辑服务商 · {id}"))
        .unwrap_or_else(|| "添加服务商".into());
    begin(daemon, title, fields, move |fields, daemon, _| {
        let id = match &existing {
            Some((id, _)) => id.clone(),
            None => Field::find(fields, "provider_id")?.required()?,
        };
        let provider = existing.as_ref().map(|(_, p)| p);
        let api_key = match (
            Field::find(fields, "api_key")?.value(),
            Field::find(fields, "clear_key")?.value(),
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
                    name: Field::find(fields, "name")?.required()?,
                    base_url: Field::find(fields, "base_url")?.required()?,
                    wire: Wire::parse(Field::find(fields, "wire")?.value())
                        .ok_or("请选择接口协议")?,
                    credential: match Field::find(fields, "credential")?.value() {
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
        fields.push(Field::text("model_id", "模型 ID", "", cx));
    }
    let choice = |value| if value { "是" } else { "否" };
    fields.extend([
        Field::text(
            "display_name",
            "显示名称",
            model.display_name.as_deref().unwrap_or_default(),
            cx,
        ),
        Field::text("context_window", "上下文窗口", model.context_window, cx),
        Field::text("max_output_tokens", "输出 Token 上限", model.max_output_tokens, cx),
        Field::choice("enabled", "启用", choice(model.enabled), ["是", "否"], cx),
        Field::choice(
            "tool_calls",
            "工具调用",
            choice(model.capabilities.tool_calls),
            ["是", "否"],
            cx,
        ),
        Field::choice(
            "image_input",
            "图片输入",
            choice(model.capabilities.image_input),
            ["是", "否"],
            cx,
        ),
        Field::choice(
            "native_web_search",
            "原生网页搜索",
            choice(model.capabilities.native_web_search),
            ["是", "否"],
            cx,
        ),
        Field::choice(
            "reasoning",
            "推理",
            choice(model.capabilities.reasoning),
            ["是", "否"],
            cx,
        ),
        Field::text(
            "reasoning_levels",
            "推理档位（逗号分隔）",
            model.reasoning_levels.join(", "),
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
            cx,
        ),
        Field::choice(
            "default_native_web_search",
            "默认启用原生搜索",
            choice(model.defaults.native_web_search),
            ["是", "否"],
            cx,
        ),
    ]);
    let title = existing
        .as_ref()
        .map(|(id, _)| format!("编辑模型 · {id}"))
        .unwrap_or_else(|| format!("添加模型 · {provider_id}"));
    begin(daemon, title, fields, move |fields, daemon, _| {
        let model_id = match &existing {
            Some((id, _)) => id.clone(),
            None => Field::find(fields, "model_id")?.required()?,
        };
        let mut model = model.clone();
        model.display_name = optional(Field::find(fields, "display_name")?.value());
        model.context_window = Field::find(fields, "context_window")?.number()?;
        model.max_output_tokens = Field::find(fields, "max_output_tokens")?.number()?;
        model.enabled = Field::find(fields, "enabled")?.value() == "是";
        model.capabilities.tool_calls = Field::find(fields, "tool_calls")?.value() == "是";
        model.capabilities.image_input = Field::find(fields, "image_input")?.value() == "是";
        model.capabilities.native_web_search =
            Field::find(fields, "native_web_search")?.value() == "是";
        model.capabilities.reasoning = Field::find(fields, "reasoning")?.value() == "是";
        model.reasoning_levels = list(Field::find(fields, "reasoning_levels")?.value());
        model.defaults.reasoning_effort =
            optional(Field::find(fields, "default_reasoning_effort")?.value());
        let default_tokens = Field::find(fields, "default_max_output_tokens")?;
        model.defaults.max_output_tokens = optional(default_tokens.value())
            .map(|_| default_tokens.number())
            .transpose()?;
        model.defaults.native_web_search =
            Field::find(fields, "default_native_web_search")?.value() == "是";
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
    button(cx, t, id, "删除", move |daemon, _| {
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
        move |daemon, cx| provider_form(daemon, cx, revision, None),
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
            move |daemon, cx| {
                model_form(daemon, cx, revision, provider_id.clone(), None);
            },
        );
        let existing = (id.clone(), provider.clone());
        let edit = button(
            cx,
            t,
            format!("provider-edit-{id}"),
            "编辑",
            move |daemon, cx| {
                provider_form(daemon, cx, revision, Some(existing.clone()));
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
        body = body.child(setting_row(
            &provider.name,
            &provider.base_url,
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
        move |daemon, cx| {
            model_form(
                daemon,
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
            move |daemon, _| {
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
        move |daemon, cx| {
            let fields = vec![
                Field::text("name", "预设名称", "", cx),
                Field::secret("api_key", "API 密钥（可选）", cx),
            ];
            let provider = provider.clone();
            let model = model_id.clone();
            begin(
                daemon,
                "从模型创建预设",
                fields,
                move |fields, daemon, _| {
                    catalog_command(
                        daemon,
                        revision,
                        RingingCommand::ProfileCreate {
                            name: fields[0].required()?,
                            model: model.clone(),
                            id: None,
                            provider: Some(provider.clone()),
                            base_url: None,
                            wire: None,
                            reasoning_effort: None,
                            context_length: None,
                            max_tokens: None,
                            api_key: optional(fields[1].value()),
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
            move |daemon, cx| {
                let fields = vec![Field::text("name", "预设名称", "", cx)];
                begin(
                    daemon,
                    "保存当前配置",
                    fields,
                    move |fields, daemon, _| {
                        catalog_command(
                            daemon,
                            revision,
                            RingingCommand::ProfileSaveCurrent {
                                name: fields[0].required()?,
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
            move |daemon, _| {
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
            move |daemon, cx| {
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
        move |daemon, cx| {
            let target = target.clone();
            let fields = vec![Field::text("names", "工具名称（逗号分隔）", &names, cx)];
            begin(
                daemon,
                "自定义会话工具",
                fields,
                move |fields, daemon, cx| {
                    let names = list(fields[0].value());
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
    cx: &mut Context<DaemonStore>,
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
        cx,
    )];
    begin(daemon, title, fields, move |fields, daemon, cx| {
        let model = options
            .iter()
            .find(|model| model_label(model) == fields[0].value())
            .cloned()
            .ok_or("请选择可用模型")?;
        select(model, daemon, cx)
    });
}

fn model_label(model: &ModelSelection) -> String {
    format!("{} / {}", model.provider_id, model.model_id)
}
