//! `SessionEvent` → `SessionStore` 的 reducer(纯函数)。
//!
//! 同步规则(契约 D1/D6):快照 seq 是互斥上界,`seq < store.seq` 的帧是
//! 残余重叠,按 id 幂等吸收;resync 只丢暂态(streaming/arg_stream/尾窗),
//! 已提交状态由快照重灌。

use qaqh_ringing_contract::events::SessionEvent;
use qaqh_ringing_contract::events::SessionEventFrame;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::fact::TurnId;
use qaqh_ringing_contract::state::SessionState;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallState;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_ringing_contract::state::TurnView;

use crate::store::SessionStore;

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
    store.subagents = state.subagents;
    store.exec_sessions = state.exec_sessions;
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

pub fn apply_frame(store: &mut SessionStore, frame: SessionEventFrame) {
    if frame.seq < store.seq {
        return;
    }
    store.seq = frame.seq + 1;
    apply_event(store, frame.event);
}

fn apply_event(store: &mut SessionStore, event: SessionEvent) {
    match event {
        SessionEvent::ActivityChanged {
            activity,
            current_turn_id,
        } => {
            store.activity = Some(activity);
            store.current_turn_id = current_turn_id;
        }
        SessionEvent::TurnStarted {
            turn_id,
            input_id,
            mode,
        } => upsert_turn(store, turn_id, |turn| {
            turn.input_id = input_id;
            turn.mode = mode;
        }),
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
        } => match store.streaming.iter_mut().find(|slot| slot.0 == block_id) {
            Some(slot) => slot.2 = text,
            None => store.streaming.push((block_id, block_kind, text)),
        },
        SessionEvent::ToolArgumentsDelta {
            call_id,
            turn_id: _,
            tool_name,
            text,
        } => match store.arg_stream.iter_mut().find(|slot| slot.0 == call_id) {
            Some(slot) => {
                slot.1 = tool_name;
                slot.2 = text;
            }
            None => store.arg_stream.push((call_id, tool_name, text)),
        },
        SessionEvent::ToolDeclared {
            call_id,
            turn_id,
            tool_name,
            args,
        } => {
            store.arg_stream.retain(|slot| slot.0 != call_id);
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
        SessionEvent::ExecSessionFinished { session } => upsert_exec(store, session),
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
            store.title = title;
        }
        SessionEvent::DriverChanged { driver } => {
            store.driver = driver;
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
            mode: None,
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
