//! 配置文档（`providers.toml` JSON）与 V3 词表的接缝。
//!
//! `qaqh_profile::ProviderDef` 的 `wire` / `credential` 字段用的是后端 V2 时代
//! 的类型 crate 里的同名枚举——本仓不依赖那个 crate（`qaqh-profile` 也没有
//! re-export 这两个枚举）。配置文档是磁盘 JSON，两个字段在盘上就是下面这几个稳定字符串
//! （daemon 的 `ListProviderPresets` 也按同一组字符串构造 V3 词表），因此这里按
//! **磁盘字符串**投影到 [`qaqh_core_vocab`] 的词表，前端全局只用 V3 枚举。
//!
//! 这不是协议转换层：只处理配置文件里的两个纯值字段。

use qaqh_core_vocab::{CredentialSource, Wire};
use qaqh_profile::ProviderDef;

/// 磁盘字符串 → 凭据类别（未知值按 `ApiKey`：BYOK 是配置文档的缺省语义）。
fn credential_of(raw: &str) -> CredentialSource {
    match raw {
        "chatgpt_plan" => CredentialSource::ChatgptPlan,
        "deepseek_account" => CredentialSource::DeepseekAccount,
        _ => CredentialSource::ApiKey,
    }
}

/// 磁盘字符串 → 线缆协议类别（未知值按 `Chat`，与 `ProviderDef` 的 `#[default]` 同口径）。
fn wire_of(raw: &str) -> Wire {
    match raw {
        "responses" => Wire::Responses,
        "messages" => Wire::Messages,
        "gemini" => Wire::Gemini,
        _ => Wire::Chat,
    }
}

fn field_str(provider: &ProviderDef, field: &str) -> String {
    serde_json::to_value(provider)
        .ok()
        .and_then(|value| value.get(field).and_then(|field| field.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// 服务商的凭据类别（V3 词表）。
pub fn credential_kind(provider: &ProviderDef) -> CredentialSource {
    credential_of(&field_str(provider, "credential"))
}

/// 服务商的线缆协议类别（V3 词表）。
pub fn wire_kind(provider: &ProviderDef) -> Wire {
    wire_of(&field_str(provider, "wire"))
}

/// V3 线缆协议 → 磁盘短名（表单回填与 `config.upsertProvider` 载荷同源）。
pub fn wire_label(wire: Wire) -> &'static str {
    match wire {
        Wire::Chat => "chat",
        Wire::Responses => "responses",
        Wire::Messages => "messages",
        Wire::Gemini => "gemini",
    }
}

/// 磁盘短名 → V3 线缆协议（未知值拒绝：表单不该把垃圾写给 daemon）。
pub fn wire_from_label(raw: &str) -> Option<Wire> {
    match raw {
        "chat" => Some(Wire::Chat),
        "responses" => Some(Wire::Responses),
        "messages" => Some(Wire::Messages),
        "gemini" => Some(Wire::Gemini),
        _ => None,
    }
}

/// `ProviderDef` → 磁盘线缆短名（表格展示/表单回填）。
pub fn wire_label_of(provider: &ProviderDef) -> &'static str {
    wire_label(wire_kind(provider))
}

/// V3 凭据类别 → 磁盘短名（`config.upsertProvider` 载荷）。
pub fn credential_label(credential: CredentialSource) -> &'static str {
    match credential {
        CredentialSource::ApiKey => "api_key",
        CredentialSource::ChatgptPlan => "chatgpt_plan",
        CredentialSource::DeepseekAccount => "deepseek_account",
    }
}
