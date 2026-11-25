//! `SessionEvent` → `SessionStore` 的 reducer(纯函数)。
//!
//! 同步规则(契约 D1/D6):快照 seq 是互斥上界,`seq < store.seq` 的帧是
//! 残余重叠,按 id 幂等吸收;resync 只丢暂态(streaming/arg_stream/尾窗),
//! 已提交状态由快照重灌。

use std::collections::HashSet;

use gpui_kit::Context;
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
    store.expanded_groups.clear();
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
    clear_transient(store);
}

/// 快照重灌后同步交接投影(A 侧倒计时/新鲜封箱仪式 + B 侧新鲜开箱仪式)。
/// 与 `apply_snapshot` 分离以保持纯快照单测无需 `Context`。
pub fn sync_handoff_after_snapshot(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    let latest_facts = store
        .tools
        .iter()
        .rev()
        .find(|view| view.tool_name.as_deref() == Some("handoff"))
        .and_then(crate::store::handoff_facts);

    if let Some(facts) = latest_facts {
        if !facts.auto_continue && !store.handoff_eager {
            store.handoff_stay = true;
        }
        if facts.sealed {
            let fresh = facts
                .sealed_at_ms
                .is_some_and(|t0| crate::store::now_ms().saturating_sub(t0) < 12_000);
            if fresh
                && !store.handoff_ritual_done.contains(&facts.call_id)
                && !store.handoff_seal_rituals.contains_key(&facts.call_id)
            {
                store
                    .handoff_seal_rituals
                    .insert(facts.call_id.clone(), std::time::Instant::now());
            }
        }
        store.handoff_open = (facts.sealed && facts.planned_open_at_ms.is_some()).then_some(facts);
        arm_handoff_tick(store, cx);
    }

    if let Some(first_input) = store
        .entries
        .iter()
        .position(|entry| matches!(entry, TimelineEntry::Input { .. }))
        && let Some(source) = crate::handoff::detect_open_source(store, first_input)
        && source.at_ms > 0
        && crate::store::now_ms().saturating_sub(source.at_ms) < 12_000
        && !store.handoff_open_ritual_done
        && store.handoff_open_ritual.is_none()
    {
        store.handoff_open_ritual = Some(std::time::Instant::now());
    }
}

/// resync:清暂态,已提交状态等待快照重灌。
pub fn clear_transient(store: &mut SessionStore) {
    store.streaming.clear();
    store.arg_stream.clear();
    store.output_tail.clear();
    store.exec_output_tail.clear();
    store.provider_retry = None;
}

pub fn apply_frame(store: &mut SessionStore, frame: SessionEventFrame, cx: &mut Context<SessionStore>) {
    if frame.seq < store.seq {
        return;
    }
    store.seq = frame.seq + 1;
    // 重试态是瞬态展示:任何权威帧(含下一条重试帧之外的一切)到达即清,
    // 重试帧自身只刷新计数/原因。
    if !matches!(&frame.event, SessionEvent::ProviderRetrying { .. }) {
        store.provider_retry = None;
    }
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
                    diff: None,
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
                    diff: None,
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
            diff,
            metrics,
            code_delta,
            effects,
        } => {
            store.output_tail.retain(|slot| slot.0 != call_id);
            store.arg_stream.retain(|slot| slot.call_id != call_id);
            let finished_call_id = call_id.clone();
            let updated = upsert_tool(store, &call_id, |existing| {
                existing.exec_session_id = exec_session_id.clone();
                existing.output = output.clone();
                existing.output_truncated = output_truncated;
                existing.diff = diff.clone();
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
                    diff,
                    metrics,
                    code_delta,
                    effects,
                });
            }
            // ADR-016 开箱机器面:handoff 工具终态成功即解析 T0/计划开箱并武装
            // 倒计时(纯展示,§决策 3);若刚封箱(12s 内)则触发一次封箱大盒仪式。
            if updated
                && let Some(view) = store.tools.iter().find(|view| view.call_id == finished_call_id)
                && view.tool_name.as_deref() == Some("handoff")
                && let Some(facts) = crate::store::handoff_facts(view)
            {
                if !facts.auto_continue && !store.handoff_eager {
                    store.handoff_stay = true;
                }
                if facts.sealed
                    && !store.handoff_ritual_done.contains(&finished_call_id)
                    && !store.handoff_seal_rituals.contains_key(&finished_call_id)
                    && facts
                        .sealed_at_ms
                        .is_none_or(|t0| crate::store::now_ms().saturating_sub(t0) < 12_000)
                {
                    store
                        .handoff_seal_rituals
                        .insert(finished_call_id.clone(), std::time::Instant::now());
                }
                store.handoff_open =
                    (facts.sealed && facts.planned_open_at_ms.is_some()).then_some(facts);
                arm_handoff_tick(store, cx);
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
        // Provider 重试(turn 级运行态陈述):仅展示,任何权威帧到达即清。
        SessionEvent::ProviderRetrying {
            attempt,
            max_attempts,
            delay_secs,
            error,
            ..
        } => {
            // 仅展示(不落库、不进事实);attempt 从 1 计,delay/error 原样透传。
            store.provider_retry = Some(crate::store::ProviderRetry {
                attempt,
                max_attempts,
                delay_secs,
                error,
            });
        }
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
            store.expanded_groups.clear();
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
            store.title = title;
        }
        SessionEvent::GoalUpdated { goal } => {
            // 整字段替换;None = 已清除。链根与当前会话双流可能都发,幂等。
            store.goal = goal;
        }
        SessionEvent::HandoffSealed {
            sealed_session_id,
            successor_session_id,
            work_summary,
            turn_id,
            at_ms,
            ..
        } => {
            // ADR-0016 纯信息面:双发到封箱线与后继线。
            if sealed_session_id == store.session_id {
                let model_auto = store
                    .tools
                    .iter()
                    .find(|tool| {
                        tool.tool_name.as_deref() == Some("handoff")
                            && tool.turn_id.as_ref() == Some(&turn_id)
                    })
                    .and_then(crate::store::handoff_facts)
                    .map(|facts| facts.auto_continue)
                    .unwrap_or(true);
                store.handoff_seal = Some(HandoffSeal {
                    sealed_session_id,
                    successor_session_id: successor_session_id.clone(),
                    work_summary,
                    at_ms,
                });
                let should_jump = store.handoff_eager || (!store.handoff_stay && model_auto);
                store.handoff_auto_jump = should_jump.then_some(successor_session_id);
            } else if successor_session_id == store.session_id {
                store.handoff_incoming = Some(crate::handoff::HandoffOpenSource {
                    parent_session_id: sealed_session_id,
                    work_summary,
                    at_ms,
                });
                if at_ms > 0
                    && crate::store::now_ms().saturating_sub(at_ms) < 12_000
                    && !store.handoff_open_ritual_done
                    && store.handoff_open_ritual.is_none()
                {
                    store.handoff_open_ritual = Some(std::time::Instant::now());
                }
            }
        }
        SessionEvent::PromptBlocked { reason } => {
            store.operation_error = Some(format!("输入被 Hook 拦截：{reason}"));
        }
        SessionEvent::HookStarted { .. } | SessionEvent::HookFinished { .. } => {}
    }
}

/// 倒计时 ticker:纯展示驱动(ADR-0016 §决策 3)——到点停表,不自做任何事;
/// 自动跳转只由 HandoffSealed 权威事件触发,不看本地钟。250ms 节拍,
/// 封箱/到点即停,不常驻。
fn arm_handoff_tick(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    let Some(facts) = &store.handoff_open else { return };
    if facts.planned_open_at_ms.unwrap_or(0) <= crate::store::now_ms() {
        return;
    }
    if store.handoff_ticking {
        return;
    }
    store.handoff_ticking = true;
    cx.spawn(async move |entity, cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let continue_tick = entity
                .update(cx, |store, cx| {
                    let done = store.handoff_seal.is_some()
                        || store
                            .handoff_open
                            .as_ref()
                            .is_none_or(|facts| facts.planned_open_at_ms.unwrap_or(0) <= crate::store::now_ms());
                    if done {
                        store.handoff_ticking = false;
                    }
                    cx.notify();
                    !done
                })
                .unwrap_or(false);
            if !continue_tick {
                break;
            }
        }
    })
    .detach();
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
