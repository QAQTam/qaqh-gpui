//! ASK v1 文本语法解析(与 `qaqh-workspace::ask_user` 同一语法,前端只消费渲染)。
//! ask 工具的 interaction.request 是 `{"input": "ASK v1 …"}` JSON;非法文档
//! 不猜不修,回退自由文本应答。

use qaqh_ringing_contract::fact::ContentValue;

pub struct AskQuestion {
    pub id: String,
    pub question: String,
    pub options: Vec<String>,
    pub allow_custom: bool,
}

/// 收口一个问题块;空选项或禁止自定义却无选项都非法。
fn finish_block(
    questions: &mut Vec<AskQuestion>,
    question: &mut Option<String>,
    options: &mut Vec<String>,
    options_seen: &mut bool,
    allow_custom: &mut bool,
) -> Result<(), String> {
    let Some(text) = question.take() else {
        return Ok(());
    };
    if *options_seen && options.is_empty() {
        return Err("`Options:` requires at least one option".into());
    }
    if options.is_empty() && !*allow_custom {
        return Err("`Allow custom: no` requires at least one option".into());
    }
    questions.push(AskQuestion {
        id: format!("q{}", questions.len() + 1),
        question: text,
        options: std::mem::take(options),
        allow_custom: *allow_custom,
    });
    *options_seen = false;
    *allow_custom = true;
    Ok(())
}

/// 解析完整 ASK v1 文档;语法错误返回 Err(原因)。
pub fn parse_ask_v1(input: &str) -> Result<Vec<AskQuestion>, String> {
    let mut lines = input.lines();
    match lines.next() {
        Some(first) if first.trim() == "ASK v1" => {}
        _ => return Err("expected `ASK v1` header".into()),
    }

    let mut questions = Vec::new();
    let mut question: Option<String> = None;
    let mut options = Vec::new();
    let mut options_seen = false;
    let mut allow_custom = true;
    let mut allow_custom_seen = false;

    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            finish_block(
                &mut questions,
                &mut question,
                &mut options,
                &mut options_seen,
                &mut allow_custom,
            )?;
            continue;
        }
        if let Some(text) = line.strip_prefix("Question: ") {
            if question.is_some() {
                return Err("separate question blocks with a blank line".into());
            }
            let text = text.trim();
            if text.is_empty() {
                return Err("question text is required".into());
            }
            question = Some(text.to_owned());
            continue;
        }
        if question.is_none() {
            return Err("expected `Question: ...`".into());
        }
        if line == "Options:" {
            if options_seen || allow_custom_seen {
                return Err("`Options:` must appear once before `Allow custom:`".into());
            }
            options_seen = true;
            continue;
        }
        if let Some(text) = line.strip_prefix("- ") {
            if !options_seen || allow_custom_seen {
                return Err("options must follow `Options:` and precede `Allow custom:`".into());
            }
            let text = text.trim();
            if text.is_empty() {
                return Err("option text is required".into());
            }
            options.push(text.to_owned());
            continue;
        }
        if let Some(value) = line.strip_prefix("Allow custom: ") {
            if allow_custom_seen {
                return Err("`Allow custom:` may appear once".into());
            }
            allow_custom = match value {
                "yes" => true,
                "no" => false,
                _ => {
                    return Err(
                        "expected exactly `Allow custom: yes` or `Allow custom: no`".into(),
                    )
                }
            };
            allow_custom_seen = true;
            continue;
        }
        return Err(
            "unknown line; use `Question:`, `Options:`, `- option`, or `Allow custom:`".into(),
        );
    }
    finish_block(
        &mut questions,
        &mut question,
        &mut options,
        &mut options_seen,
        &mut allow_custom,
    )?;
    if questions.is_empty() {
        return Err("at least one question is required".into());
    }
    Ok(questions)
}

/// 从 interaction.request(`{"input": "ASK v1 …"}`)提取问题;解析失败返回 Err。
pub fn parse_request(request: &ContentValue) -> Result<Vec<AskQuestion>, String> {
    let ContentValue::Inline { text } = request else {
        return Err("request content unavailable".into());
    };
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("request is not json: {error}"))?;
    let input = value
        .get("input")
        .and_then(serde_json::Value::as_str)
        .ok_or("expected string field `input`")?;
    parse_ask_v1(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_question_parses_with_ids_and_defaults() {
        let questions =
            parse_ask_v1("ASK v1\nQuestion: Choose A or B?\nOptions:\n- A\n- B\nAllow custom: no")
                .expect("parse");
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].id, "q1");
        assert_eq!(questions[0].options, vec!["A".to_owned(), "B".to_owned()]);
        assert!(!questions[0].allow_custom);
    }

    #[test]
    fn trailing_block_without_blank_line_is_kept() {
        let questions = parse_ask_v1(
            "ASK v1\nQuestion: Which?\nOptions:\n- A\n\nQuestion: Constraints?",
        )
        .expect("parse");
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].id, "q1");
        assert_eq!(questions[1].id, "q2");
        assert!(questions[0].allow_custom);
        assert!(questions[1].options.is_empty());
    }

    #[test]
    fn malformed_fails() {
        assert!(parse_ask_v1("Question: no header").is_err());
        assert!(parse_ask_v1("ASK v1\nQuestion: Q?\nUnknown: value").is_err());
        assert!(parse_ask_v1("ASK v1\nQuestion: Q?\nOptions:").is_err());
    }
}
