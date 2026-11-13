//! `SessionEvent` → `SessionStore` 的 reducer(纯函数)。
//!
//! 同步规则(契约 D1/D6):快照 seq 是互斥上界,`seq < store.seq` 的帧是
//! 残余重叠,按 id 幂等吸收;resync 只丢暂态(streaming/arg_stream/尾窗),
//! 已提交状态由快照重灌。

use std::collections::HashSet;

use gpui::Context;
use qaqh_ringing_contract::events::SessionEvent;
use qaqh_ringing_contract::events::SessionEventFrame;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::fact::TurnId;
use qaqh_ringing_contract::state::SessionState;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallState;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_ringing_contract::state::TurnView;

use crate::store::{ArgStream, HandoffSeal, SessionStore};

pub fn apply_snapshot(store: &mut SessionStore, state: SessionState) {
    store.seq = state.seq;
    store.activity = Some(state.activity);
    store.current_turn_id = state.current_turn_id;
    store.title = state.title;
    store.turns = state.turns;
    store.entries = state.timeline;
    store.timeline_list.reset(store.entries.len() + 1);
    store.tools = state.tools;
    store.interactions = state.interactions;
    // 快照替换了交互列表:清掉孤儿 ask 草稿与提示,避免引用悬空的 interaction。
    store
        .ask_drafts
        .retain(|(id, _)| store.interactions.iter().any(|view| &view.interaction_id == id));
    store.ask_hint = None;
    store.subagents = state.subagents;
    store.exec_sessions = state.exec_sessions;
    store.goal = state.goal;
    store.driver = state.driver;
    clear_transient(store);
}

/// resync:清暂态,已提交状态等待快照重灌。
pub fn clear_transient(store: &mut SessionStore) {
    store.streaming.clear();
    store.arg_stream.clear();
    store.output_tail.clear();
    store.exec_output_tail.clear();
    store.md_cache.clear();
}

pub fn apply_frame(store: &mut SessionStore, frame: SessionEventFrame, cx: &mut Context<SessionStore>) {
    if frame.seq < store.seq {
        return;
    }
    store.seq = frame.seq + 1;
    apply_event(store, frame.event, cx);
    for (_, text) in &mut store.output_tail { trim_tail(text, store.output_budget); }
    for (_, _, text) in &mut store.exec_output_tail { trim_tail(text, store.output_budget); }
}

fn trim_tail(text: &mut String, budget: usize) {
    if text.len() <= budget { return; }
    let mut start = text.len() - budget;
    while !text.is_char_boundary(start) { start += 1; }
    text.drain(..start);
}

fn apply_event(store: &mut SessionStore, event: SessionEvent, cx: &mut Context<SessionStore>) {
    match event {
        SessionEvent::ActivityChanged {
            activity,
            current_turn_id,
        } => {
            store.activity = Some(activity);
            store.current_turn_id = current_turn_id;
        }
        SessionEvent::TurnStarted { turn_id, input_id } => {
            upsert_turn(store, turn_id, |turn| turn.input_id = input_id)
        }
        SessionEvent::TurnPhaseChanged { turn_id, phase } => {
            upsert_turn(store, turn_id, |turn| turn.phase = Some(phase));
        }
        SessionEvent::TurnEnded { turn_id, outcome } => {
            upsert_turn(store, turn_id, |turn| {
                turn.outcome = Some(outcome);
                turn.phase = None;
            });
        }
        SessionEvent::MessageDelta {
            turn_id: _,
            block_id,
            block_kind,
            text,
        } => {
            // tok/s 采样:Answer 流的净增字符入 3s 滑窗(替换语义取差值)。
            if block_kind == qaqh_ringing_contract::fact::AssistantBlockKind::Answer {
                let delta = match store.streaming.iter().find(|slot| slot.0 == block_id) {
                    Some(slot) => text.len() as i64 - slot.2.len() as i64,
                    None => text.len() as i64,
                };
                store.note_stream_chars(delta);
            }
            match store.streaming.iter_mut().find(|slot| slot.0 == block_id) {
                Some(slot) => slot.2 = text,
                None => store.streaming.push((block_id, block_kind, text)),
            }
        }
        SessionEvent::ToolArgumentsDelta {
            call_id,
            turn_id: _,
            tool_name,
            text,
            lines_added,
            lines_removed,
            target_files,
            args_complete,
        } => match store.arg_stream.iter_mut().find(|slot| slot.call_id == call_id) {
            Some(slot) => {
                slot.tool_name = tool_name;
                slot.text = text;
                slot.lines = lines_added.zip(lines_removed);
                slot.target_files = target_files;
                slot.args_complete = args_complete;
            }
            None => store.arg_stream.push(ArgStream {
                call_id,
                tool_name,
                text,
                lines: lines_added.zip(lines_removed),
                target_files,
                args_complete,
            }),
        },
        SessionEvent::ToolDeclared {
            call_id,
            turn_id,
            tool_name,
            args,
        } => {
            store.arg_stream.retain(|slot| slot.call_id != call_id);
            let updated = upsert_tool(store, &call_id, |existing| {
                existing.turn_id = Some(turn_id.clone());
                existing.tool_name = Some(tool_name.clone());
                existing.args = args.clone();
                existing.state = ToolCallState::Declared;
            });
            if !updated {
                store.tools.push(ToolCallView {
                    call_id,
                    turn_id: Some(turn_id),
                    tool_name: Some(tool_name),
                    args,
                    state: ToolCallState::Declared,
                    exec_session_id: None,
                    output: None,
                    output_truncated: false,
                    metrics: None,
                    code_delta: None,
                    effects: Vec::new(),
                });
            }
        }
        SessionEvent::ToolStateChanged {
            call_id,
            turn_id,
            state,
        } => {
            let updated = upsert_tool(store, &call_id, |existing| {
                existing.turn_id = Some(turn_id.clone());
                existing.state = state.clone();
            });
            if !updated {
                store.tools.push(ToolCallView {
                    call_id,
                    turn_id: Some(turn_id),
                    tool_name: None,
                    args: None,
                    state,
                    exec_session_id: None,
                    output: None,
                    output_truncated: false,
                    metrics: None,
                    code_delta: None,
                    effects: Vec::new(),
                });
            }
        }
        SessionEvent::ToolOutputDelta {
            call_id,
            stream: _,
            text,
        } => match store.output_tail.iter_mut().find(|slot| slot.0 == call_id) {
            Some(slot) => slot.1.push_str(&text),
            None => store.output_tail.push((call_id, text)),
        },
        SessionEvent::ToolFinished {
            call_id,
            outcome,
            exec_session_id,
            output,
            output_truncated,
            metrics,
            code_delta,
            effects,
        } => {
            store.output_tail.retain(|slot| slot.0 != call_id);
            store.arg_stream.retain(|slot| slot.call_id != call_id);
            let updated = upsert_tool(store, &call_id, |existing| {
                existing.exec_session_id = exec_session_id.clone();
                existing.output = output.clone();
                existing.output_truncated = output_truncated;
                existing.metrics = metrics.clone();
                existing.code_delta = code_delta.clone();
                existing.effects = effects.clone();
                existing.state = ToolCallState::Finished { outcome: outcome.clone() };
            });
            if !updated {
                store.tools.push(ToolCallView {
                    call_id,
                    turn_id: None,
                    tool_name: None,
                    args: None,
                    state: ToolCallState::Finished { outcome },
                    exec_session_id,
                    output,
                    output_truncated,
                    metrics,
                    code_delta,
                    effects,
                });
            }
        }
        SessionEvent::ExecSessionStarted { session } => upsert_exec(store, session),
        SessionEvent::ExecSessionStateChanged { session } => upsert_exec(store, session),
        SessionEvent::ExecSessionFinished { session } => {
            store.exec_output_tail.retain(|slot| slot.0 != session.id);
            upsert_exec(store, session);
        }
        SessionEvent::ExecSessionOutputDelta {
            exec_session_id,
            start_offset,
            end_offset,
            source: _,
            text,
        } => match store
            .exec_output_tail
            .iter_mut()
            .find(|slot| slot.0 == exec_session_id)
        {
            Some(slot) => {
                if start_offset >= slot.1 {
                    slot.1 = end_offset;
                    slot.2.push_str(&text);
                }
            }
            None => store
                .exec_output_tail
                .push((exec_session_id, end_offset, text)),
        },
        SessionEvent::TimelineAppended { entry } => {
            if let TimelineEntry::AssistantBlock { block_id, .. } = &entry {
                store.streaming.retain(|slot| &slot.0 != block_id);
            }
            upsert_entry(store, entry);
        }
        SessionEvent::TimelineTruncated {
            from_turn_id: _,
            removed_turn_ids,
        } => {
            // 权威集合整枝:回合(输入/正文/工具卡/交互)一并消失;副作用
            // (journal/exec/子代理)不动。无 turn 归属的条目无法判定,保守保留。
            let removed: std::collections::HashSet<TurnId> = removed_turn_ids.into_iter().collect();
            let removed_calls: HashSet<ToolCallId> = store
                .tools
                .iter()
                .filter(|tool| tool.turn_id.as_ref().is_some_and(|turn_id| removed.contains(turn_id)))
                .map(|tool| tool.call_id.clone())
                .collect();
            store.turns.retain(|turn| !removed.contains(&turn.turn_id));
            store
                .tools
                .retain(|tool| !tool.turn_id.as_ref().is_some_and(|turn_id| removed.contains(turn_id)));
            store.interactions.retain(|interaction| {
                !interaction
                    .turn_id
                    .as_ref()
                    .is_some_and(|turn_id| removed.contains(turn_id))
            });
            store.arg_stream.retain(|slot| !removed_calls.contains(&slot.call_id));
            store.output_tail.retain(|(call_id, _)| !removed_calls.contains(call_id));
            store.entries.retain(|entry| match entry {
                TimelineEntry::Input { turn_id, .. }
                | TimelineEntry::ToolCall { turn_id, .. }
                | TimelineEntry::ToolResult { turn_id, .. } => {
                    turn_id.as_ref().is_none_or(|turn_id| !removed.contains(turn_id))
                }
                TimelineEntry::AssistantBlock { turn_id, .. } => !removed.contains(turn_id),
                TimelineEntry::Compaction { .. } => true,
            });
            store.timeline_list.reset(store.entries.len() + 1);
        }
        SessionEvent::InteractionRequested { interaction } => {
            match store
                .interactions
                .iter_mut()
                .find(|slot| slot.interaction_id == interaction.interaction_id)
            {
                Some(slot) => *slot = interaction,
                None => store.interactions.push(interaction),
            }
        }
        SessionEvent::InteractionResolved {
            interaction_id, ..
        } => {
            store.ask_drafts.retain(|(id, _)| id != &interaction_id);
            store.ask_hint = None;
            store
                .interactions
                .retain(|slot| slot.interaction_id != interaction_id);
        }
        SessionEvent::SubagentUpdated { subagent } => {
            match store
                .subagents
                .iter_mut()
                .find(|slot| slot.child_session_id == subagent.child_session_id)
            {
                Some(slot) => *slot = subagent,
                None => store.subagents.push(subagent),
            }
        }
        SessionEvent::SessionMetaChanged { title } => {
            // 重命名草稿与权威标题一致才收口;草稿值经实体读取。
            if store.renaming.as_ref().is_some_and(|draft| Some(draft.read(cx).value().trim()) == title.as_deref()) { store.renaming = None; }
            store.title = title;
        }
        SessionEvent::DriverChanged { driver } => {
            store.driver = driver;
        }
        SessionEvent::GoalUpdated { goal } => {
            // 整字段替换;None = 已清除。链根与当前会话双流可能都发,幂等。
            store.goal = goal;
        }
        SessionEvent::HandoffSealed {
            sealed_session_id,
            successor_session_id,
            work_summary,
            ..
        } => {
            // ADR-0016 纯信息面:fork 本体不依赖本事件。仅封箱线本线需要标记,
            // 后继线经 session.list 正常渲染。双发幂等:整字段替换(同 GoalUpdated 口径)。
            if sealed_session_id == store.session_id {
                store.handoff_seal = Some(HandoffSeal { successor_session_id, work_summary });
            }
        }
    }
}

/// 按 call_id 更新既有工具视图;不存在返回 false。
fn upsert_tool(
    store: &mut SessionStore,
    call_id: &ToolCallId,
    update: impl FnOnce(&mut ToolCallView),
) -> bool {
    match store.tools.iter_mut().find(|tool| &tool.call_id == call_id) {
        Some(tool) => {
            update(tool);
            true
        }
        None => false,
    }
}

fn upsert_turn(store: &mut SessionStore, turn_id: TurnId, update: impl FnOnce(&mut TurnView)) {
    if let Some(turn) = store.turns.iter_mut().find(|turn| turn.turn_id == turn_id) {
        update(turn);
    } else {
        store.turns.push(TurnView {
            turn_id,
            input_id: None,
            phase: None,
            outcome: None,
        });
        update(store.turns.last_mut().expect("just pushed"));
    }
}

fn upsert_exec(
    store: &mut SessionStore,
    session: qaqh_ringing_contract::state::ExecSessionView,
) {
    match store
        .exec_sessions
        .iter_mut()
        .find(|slot| slot.id == session.id)
    {
        Some(slot) => *slot = session,
        None => store.exec_sessions.push(session),
    }
}

fn upsert_entry(store: &mut SessionStore, entry: TimelineEntry) {
    let Some(identity) = entry_identity(&entry) else {
        store.entries.push(entry);
        return;
    };
    match store
        .entries
        .iter()
        .position(|slot| entry_identity(slot).as_deref() == Some(identity.as_str()))
    {
        Some(position) => store.entries[position] = entry,
        None => store.entries.push(entry),
    }
}

/// 幂等身份:AssistantBlock/Input 按 block_id,ToolCall/ToolResult 按 call_id,
/// Compaction 无身份(追加)。
fn entry_identity(entry: &TimelineEntry) -> Option<String> {
    match entry {
        TimelineEntry::Input { block_id, .. } => Some(format!("input:{}", block_id.0)),
        TimelineEntry::AssistantBlock { block_id, .. } => {
            Some(format!("assistant:{}", block_id.0))
        }
        TimelineEntry::ToolCall { call_id, .. } => Some(format!("call:{}", call_id.0)),
        TimelineEntry::ToolResult { call_id, .. } => Some(format!("result:{}", call_id.0)),
        TimelineEntry::Compaction { .. } => None,
    }
}
