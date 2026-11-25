//! ASK 交互请求解析:后端 `session_loop::run_ask` 已通过 `normalize_ask_user`
//! 将模型输入规范化并随 `InteractionView.request` 下发 `NormalizedAsk` JSON
//! (`{"mode":"single"|"batch","questions":[...]}`),前端直接反序列化即可。
//! 非法/空载荷与未就绪引用不猜不修,回退自由文本应答。

use qaqh_ringing_contract::fact::ContentValue;
use serde::Deserialize;

fn default_allow_custom() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AskQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default = "default_allow_custom")]
    pub allow_custom: bool,
}

#[derive(Debug, Deserialize)]
struct AskPayload {
    questions: Vec<AskQuestion>,
}

/// 从 `interaction.request` (`NormalizedAsk` JSON) 提取问题列表;解析失败或无问题返回 `Err`。
pub fn parse_request(request: &ContentValue) -> Result<Vec<AskQuestion>, String> {
    let ContentValue::Inline { text } = request else {
        return Err("request content unavailable".into());
    };
    let payload: AskPayload =
        serde_json::from_str(text).map_err(|error| format!("request is not valid ask json: {error}"))?;
    if payload.questions.is_empty() {
        return Err("ask payload has no questions".into());
    }
    Ok(payload.questions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_mode_payload_parses_with_allow_custom_false() {
        let req = ContentValue::Inline {
            text: r#"{"mode":"single","questions":[{"id":"q1","question":"Choose A or B?","options":["A","B"],"allow_custom":false}]}"#.into(),
        };
        let questions = parse_request(&req).expect("parse single");
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].id, "q1");
        assert_eq!(questions[0].question, "Choose A or B?");
        assert_eq!(questions[0].options, vec!["A".to_owned(), "B".to_owned()]);
        assert!(!questions[0].allow_custom);
    }

    #[test]
    fn batch_mode_payload_parses_and_defaults_allow_custom() {
        let req = ContentValue::Inline {
            text: r#"{"mode":"batch","questions":[{"id":"q1","question":"如何处理 `FORBIDDEN_DIDS`?","options":["保持原样","改成 sha256","挪进 gitignore"],"allow_custom":true},{"id":"q2","question":"要不要另做备份?"}]}"#.into(),
        };
        let questions = parse_request(&req).expect("parse batch");
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].id, "q1");
        assert_eq!(questions[0].options.len(), 3);
        assert!(questions[0].allow_custom);
        assert_eq!(questions[1].id, "q2");
        assert!(questions[1].options.is_empty());
        assert!(questions[1].allow_custom);
    }

    #[test]
    fn invalid_empty_or_ref_payload_returns_err() {
        assert!(parse_request(&ContentValue::Inline { text: "not json".into() }).is_err());
        assert!(parse_request(&ContentValue::Inline {
            text: r#"{"mode":"batch","questions":[]}"#.into(),
        }).is_err());
        assert!(parse_request(&ContentValue::Ref {
            content_ref: qaqh_ringing_contract::fact::ContentRef::new(
                qaqh_ringing_contract::fact::ContentHash::new("cnt_1"),
            ),
        }).is_err());
    }
}
