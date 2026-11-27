//! GitHub PR/Issue 行内胶囊元数据懒加载与缓存:
//! - 仅在鼠标悬停到对应链接胶囊时按需触发本地 `gh api` 请求;
//! - 面向弱网环境设置 10s 超时与退避冷却,未确认拿到完整数据前绝不弹出空卡片或骨架卡片;
//! - 通过全局缓存 + 短 TTL(45s) 兼顾状态实时性(如 Open -> Merged)与防抖。

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gpui_kit::{App, Global, WeakEntity};
use serde_json::Value;

use crate::store::SessionStore;

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const CACHE_TTL: Duration = Duration::from_secs(45);
const FAILURE_COOLDOWN: Duration = Duration::from_secs(12);
const SUMMARY_MAX_CHARS: usize = 220;
const SUMMARY_MAX_LINES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GitHubKind {
    Pull,
    Issue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRef {
    pub kind: GitHubKind,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
}

impl GitHubRef {
    /// 从 GitHub URL 解析 PR 或 Issue 引用(`https://github.com/<owner>/<repo>/pull/<n>` 或 `/issues/<n>`)。
    pub fn parse_url(raw: &str) -> Option<Self> {
        let rest = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))?;
        let without_query = rest.split(['?', '#']).next().unwrap_or(rest);
        let mut parts = without_query.split('/');
        let host = parts.next()?.to_ascii_lowercase();
        if host != "github.com" && host != "www.github.com" {
            return None;
        }
        let owner = parts.next()?.trim();
        let repo = parts.next()?.trim();
        let kind_seg = parts.next()?.trim();
        let num_seg = parts.next()?.trim();
        if owner.is_empty() || repo.is_empty() {
            return None;
        }
        let kind = match kind_seg {
            "pull" => GitHubKind::Pull,
            "issues" => GitHubKind::Issue,
            _ => return None,
        };
        let number: u64 = num_seg.parse().ok().filter(|&n| n > 0)?;
        Some(Self {
            kind,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
            url: raw.to_owned(),
        })
    }

    pub fn short_label(&self) -> String {
        format!("{}#{}", self.repo, self.number)
    }

    pub fn full_label(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }

    fn key(&self) -> GitHubCacheKey {
        GitHubCacheKey {
            kind: self.kind,
            owner: self.owner.to_ascii_lowercase(),
            repo: self.repo.to_ascii_lowercase(),
            number: self.number,
        }
    }

    fn api_endpoint(&self) -> String {
        match self.kind {
            GitHubKind::Pull => format!("repos/{}/{}/pulls/{}", self.owner, self.repo, self.number),
            GitHubKind::Issue => format!("repos/{}/{}/issues/{}", self.owner, self.repo, self.number),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GitHubCacheKey {
    kind: GitHubKind,
    owner: String,
    repo: String,
    number: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubItemState {
    Open,
    Merged,
    Closed,
    Draft,
}

impl GitHubItemState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Merged => "Merged",
            Self::Closed => "Closed",
            Self::Draft => "Draft",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitHubItemMeta {
    pub reference: GitHubRef,
    pub state: GitHubItemState,
    pub title: String,
    pub branch: Option<(String, String)>,
    pub author: Option<String>,
    pub summary: Option<String>,
}

enum CacheEntry {
    Loading,
    Ready {
        meta: GitHubItemMeta,
        fetched_at: Instant,
    },
    Failed {
        failed_at: Instant,
    },
}

#[derive(Default)]
pub struct GitHubPreviewCache {
    entries: HashMap<GitHubCacheKey, CacheEntry>,
}

impl Global for GitHubPreviewCache {}

/// 读取当前缓存快照:`(确认已加载且未过期的新鲜数据, 是否正在加载)`。
pub fn snapshot(cx: &App, reference: &GitHubRef) -> (Option<GitHubItemMeta>, bool) {
    let Some(cache) = cx.try_global::<GitHubPreviewCache>() else {
        return (None, false);
    };
    let now = Instant::now();
    match cache.entries.get(&reference.key()) {
        Some(CacheEntry::Loading) => (None, true),
        Some(CacheEntry::Ready { meta, fetched_at }) => {
            if now.saturating_duration_since(*fetched_at) <= CACHE_TTL {
                (Some(meta.clone()), false)
            } else {
                (None, false)
            }
        }
        Some(CacheEntry::Failed { .. }) | None => (None, false),
    }
}

/// 鼠标悬停触发懒加载;命中新鲜缓存、正在加载或失败退避期时不重复请求。
pub fn request_on_hover(
    reference: GitHubRef,
    session: WeakEntity<SessionStore>,
    cx: &mut App,
) {
    let key = reference.key();
    let now = Instant::now();
    let should_fetch = {
        let cache = cx.default_global::<GitHubPreviewCache>();
        match cache.entries.get(&key) {
            Some(CacheEntry::Loading) => false,
            Some(CacheEntry::Ready { fetched_at, .. }) => {
                now.saturating_duration_since(*fetched_at) > CACHE_TTL
            }
            Some(CacheEntry::Failed { failed_at }) => {
                now.saturating_duration_since(*failed_at) > FAILURE_COOLDOWN
            }
            None => true,
        }
    };
    if !should_fetch {
        return;
    }

    cx.default_global::<GitHubPreviewCache>()
        .entries
        .insert(key.clone(), CacheEntry::Loading);
    let _ = session.update(cx, |_, cx| cx.notify());

    let reference_for_fetch = reference.clone();
    cx.spawn(async move |cx| {
        let fetched = fetch_via_gh_with_timeout(&reference_for_fetch, cx).await;
        let _ = cx.update(|cx| {
            let entry = match fetched {
                Ok(meta) => CacheEntry::Ready {
                    meta,
                    fetched_at: Instant::now(),
                },
                Err(()) => CacheEntry::Failed {
                    failed_at: Instant::now(),
                },
            };
            cx.default_global::<GitHubPreviewCache>()
                .entries
                .insert(key, entry);
            let _ = session.update(cx, |_, cx| cx.notify());
            cx.refresh_windows();
        });
    })
    .detach();
}

async fn fetch_via_gh_with_timeout(
    reference: &GitHubRef,
    cx: &gpui_kit::AsyncApp,
) -> Result<GitHubItemMeta, ()> {
    let endpoint = reference.api_endpoint();
    let mut cmd = Command::new("gh");
    cmd.args(["api", &endpoint])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd.spawn().map_err(|_| ())?;
    let stdout = child.stdout.take().ok_or(())?;
    let read_task = cx.background_executor().spawn(async move {
        use std::io::Read;
        let mut buf = Vec::new();
        let mut reader = stdout;
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + FETCH_TIMEOUT;
    let exited_ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(());
                }
                cx.background_executor()
                    .timer(Duration::from_millis(45))
                    .await;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
        }
    };
    if !exited_ok {
        return Err(());
    }
    let bytes = read_task.await;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    parse_gh_payload(reference, &value)
}

fn parse_gh_payload(reference: &GitHubRef, value: &Value) -> Result<GitHubItemMeta, ()> {
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(())?
        .to_owned();

    let state = if value.get("merged").and_then(Value::as_bool) == Some(true) {
        GitHubItemState::Merged
    } else if value.get("state").and_then(Value::as_str) == Some("closed") {
        GitHubItemState::Closed
    } else if value.get("draft").and_then(Value::as_bool) == Some(true) {
        GitHubItemState::Draft
    } else {
        GitHubItemState::Open
    };

    let branch = match (
        value
            .get("head")
            .and_then(|head| head.get("ref"))
            .and_then(Value::as_str),
        value
            .get("base")
            .and_then(|base| base.get("ref"))
            .and_then(Value::as_str),
    ) {
        (Some(head), Some(base)) if !head.is_empty() && !base.is_empty() => {
            Some((head.to_owned(), base.to_owned()))
        }
        _ => None,
    };

    let author = value
        .get("user")
        .and_then(|user| user.get("login"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let summary = value
        .get("body")
        .and_then(Value::as_str)
        .and_then(summarize_body);

    Ok(GitHubItemMeta {
        reference: reference.clone(),
        state,
        title,
        branch,
        author,
        summary,
    })
}

fn summarize_body(body: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut total_chars = 0usize;
    let mut truncated = false;

    for raw in body.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("<!--") {
            continue;
        }
        if lines.len() >= SUMMARY_MAX_LINES {
            truncated = true;
            break;
        }
        let remaining = SUMMARY_MAX_CHARS.saturating_sub(total_chars);
        if remaining == 0 {
            truncated = true;
            break;
        }
        let char_count = trimmed.chars().count();
        if char_count > remaining {
            let cut: String = trimmed.chars().take(remaining).collect();
            lines.push(cut);
            truncated = true;
            break;
        }
        total_chars += char_count;
        lines.push(trimmed.to_owned());
    }

    if lines.is_empty() {
        return None;
    }
    let mut joined = lines.join("\n");
    if truncated {
        joined.push('…');
    }
    Some(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_pull_and_issue_urls() {
        let pr = GitHubRef::parse_url("https://github.com/QAQTam/qaqh-backend/pull/47").unwrap();
        assert_eq!(pr.kind, GitHubKind::Pull);
        assert_eq!(pr.owner, "QAQTam");
        assert_eq!(pr.repo, "qaqh-backend");
        assert_eq!(pr.number, 47);
        assert_eq!(pr.short_label(), "qaqh-backend#47");

        let pr_files =
            GitHubRef::parse_url("https://github.com/QAQTam/qaqh-backend/pull/47/files#diff-1")
                .unwrap();
        assert_eq!(pr_files.number, 47);

        let issue = GitHubRef::parse_url("https://github.com/rust-lang/rust/issues/123").unwrap();
        assert_eq!(issue.kind, GitHubKind::Issue);
        assert_eq!(issue.number, 123);

        assert!(GitHubRef::parse_url("https://github.com/QAQTam/qaqh-backend").is_none());
        assert!(GitHubRef::parse_url("https://example.com/a/b/pull/1").is_none());
    }

    #[test]
    fn parses_gh_pr_json_payload() {
        let pr = GitHubRef::parse_url("https://github.com/QAQTam/qaqh-backend/pull/47").unwrap();
        let json = serde_json::json!({
            "title": "[v4.6 P3-0] 修复问题",
            "state": "closed",
            "merged": true,
            "draft": false,
            "head": { "ref": "fix/p3" },
            "base": { "ref": "main" },
            "user": { "login": "QAQTam" },
            "body": "\n## 改动摘要\n\n- 第一条\n"
        });
        let meta = parse_gh_payload(&pr, &json).unwrap();
        assert_eq!(meta.state, GitHubItemState::Merged);
        assert_eq!(meta.branch, Some(("fix/p3".into(), "main".into())));
        assert_eq!(meta.author.as_deref(), Some("QAQTam"));
        assert_eq!(meta.summary.as_deref(), Some("## 改动摘要\n- 第一条"));
    }
}
