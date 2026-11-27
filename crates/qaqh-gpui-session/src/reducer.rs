//! `SessionEvent` → `SessionStore` 的 reducer(纯函数)。
//!
//! 同步规则(ADR-0042):快照 `seq` 是**下一个将分配**的互斥上界,所以
//! `store.seq` 记「已收到的最后序号」(`snapshot.seq - 1`),重复帧按
//! `meta.seq <= store.seq` 丢弃;resync 只丢暂态(streaming/arg_stream/尾窗),
//! 已提交状态由快照重灌——**绝不拼接**(拼出来的时间线是错位的假历史)。

use std::collections::HashSet;

use gpui_kit::Context;
use qaqh_core_vocab::{ActivityState, BlockId, ToolCallId, TurnId, TurnTerminal};
use qaqh_ringing_v3::{BlockKind, BlockView, SessionEvent, SessionEventFrame, SessionSnapshot, ToolCallView, TurnSummary};

use crate::store::{ArgStream, HandoffSeal, SessionStore};

pub fn apply_snapshot(store: &mut SessionStore, snapshot: SessionSnapshot) {
    store.seq = snapshot.seq.saturating_sub(1);
    store.activity = snapshot.activity;
    store.current_turn_id = None;
    store.title = snapshot.title.clone();
    store.timeline_epoch = snapshot.timeline_epoch;
    store.turns = snapshot.turns;
    store.entries = snapshot.tail_blocks;
    store.entries.sort_by_key(|block| block.ord);
    store.transcript_before_ord = store.entries.first().map(|block| block.ord);
    store.transcript_loading = false;
    store.pinned_compaction = snapshot.pinned_compaction;
    store.tool_mode = snapshot.tool_mode;
    store.handoff_from_session_id = snapshot.handoff_from.clone();
    store.session_created_at_ms = snapshot.created_at_ms;
    store.driver_client_instance_id = snapshot.driver_client_instance_id.clone();
    store.driver_ordinal = snapshot.driver_ordinal;
    store.expanded_groups.clear();
    store.expanded_traces.clear();
    store.collapsed_traces.clear();
    store.trace_scroll_handles.clear();
    store.trace_started_at.clear();
    store.trace_elapsed_ms.clear();
    store.turn_started_at.clear();
    store.turn_elapsed_ms.clear();
    store.tools = snapshot.tool_calls;
    store.interactions = snapshot.pending_interactions;
    // 快照替换了交互列表:清掉孤儿 ask 草稿与提示,避免引用悬空的 interaction。
    store
        .ask_drafts
        .retain(|(id, _)| store.interactions.iter().any(|view| &view.interaction_id == id));
    store.ask_hint = None;
    store.subagents = snapshot.subagents;
    store.exec_sessions = snapshot.exec_sessions;
    store.goal = snapshot.goal;
    if let Some(settings) = store.settings.as_mut() {
        settings.title = snapshot.title;
        settings.workspace_id = snapshot.workspace_id;
        settings.cwd = snapshot.cwd;
        settings.archived = snapshot.archived;
        settings.settings_revision = snapshot.settings_revision;
        settings.model_selection = snapshot.model_selection;
        settings.activity = snapshot.activity;
        settings.tool_mode = snapshot.tool_mode;
        settings.custom_tools = snapshot.custom_tools;
        settings.handoff_from = snapshot.handoff_from;
    }
    clear_transient(store);
    store.timeline_list.reset(store.entries.len() + 1);
    store.prefetch_blobs();
}

/// 快照重灌后同步交接投影(A 侧倒计时/新鲜封箱仪式 + B 侧新鲜开箱仪式)。
/// 与 `apply_snapshot` 分离以保持纯快照单测无需 `Context`。
pub fn sync_handoff_after_snapshot(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    let latest_facts = store
        .tools
        .iter()
        .rev()
        .filter(|view| view.tool_name == "handoff")
        .find_map(|view| {
            crate::store::handoff_facts(view, |content| store.content_text(content))
        });

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

    if store.is_running() {
        arm_trace_tick(store, cx);
    }

    let first_input = store
        .entries
        .iter()
        .position(|block| block.kind == BlockKind::User);
    if let Some(first_input) = first_input
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

/// 向上翻页结果合并:更早的块插到时间线前方,工具视图补齐,游标前移。
///
/// 代际失配(`timeline_epoch`)时直接丢弃:服务端已判 `StaleCursor`,壳层会重取快照。
pub fn apply_transcript(store: &mut SessionStore, page: qaqh_ringing_v3::TranscriptPage) {
    store.transcript_loading = false;
    if page.timeline_epoch != store.timeline_epoch {
        return;
    }
    for block in page.blocks {
        upsert_block(store, block);
    }
    for call in page.tool_calls {
        upsert_tool_view(store, call);
    }
    store.transcript_before_ord = page
        .next_before_ord
        .or_else(|| store.entries.first().map(|block| block.ord));
    store.timeline_list.reset(store.entries.len() + 1);
    store.prefetch_blobs();
}

pub fn apply_frame(
    store: &mut SessionStore,
    frame: SessionEventFrame,
    cx: &mut Context<SessionStore>,
) {
    if frame.meta.seq <= store.seq {
        return;
    }
    store.seq = frame.meta.seq;
    // 重试态是瞬态展示:任何权威帧(重试帧自身除外)到达即清。
    if !matches!(&frame.event, SessionEvent::ProviderRetrying { .. }) {
        store.provider_retry = None;
    }
    apply_event(store, frame.event, cx);
    for (_, text) in &mut store.output_tail {
        trim_tail(text, store.output_budget);
    }
    for (_, _, text) in &mut store.exec_output_tail {
        trim_tail(text, store.output_budget);
    }
    store.prefetch_blobs();
}

fn trim_tail(text: &mut String, budget: usize) {
    if text.len() <= budget {
        return;
    }
    let mut start = text.len() - budget;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.drain(..start);
}

fn apply_event(store: &mut SessionStore, event: SessionEvent, cx: &mut Context<SessionStore>) {
    match event {
        SessionEvent::ActivityChanged {
            activity,
            current_turn_id,
            ..
        } => {
            store.activity = activity;
            store.current_turn_id = current_turn_id.clone();
            if activity == ActivityState::Running {
                if let Some(turn_id) = current_turn_id {
                    store
                        .turn_started_at
                        .entry(turn_id)
                        .or_insert_with(std::time::Instant::now);
                }
                arm_trace_tick(store, cx);
            }
        }
        SessionEvent::TurnStarted {
            turn_id,
            input_id,
            started_at_ms,
            ..
        } => {
            store
                .turn_started_at
                .entry(turn_id.clone())
                .or_insert_with(std::time::Instant::now);
            upsert_turn(store, turn_id, started_at_ms, |turn| turn.input_id = input_id);
            arm_trace_tick(store, cx);
        }
        SessionEvent::TurnPhaseChanged { turn_id, phase, .. } => {
            if let Some(turn) = store.turns.iter_mut().find(|turn| turn.turn_id == turn_id) {
                turn.phase = Some(phase);
            }
        }
        SessionEvent::TurnFinished {
            turn_id,
            terminal,
            usage,
            ..
        } => {
            if let Some(started) = store.turn_started_at.remove(&turn_id) {
                store
                    .turn_elapsed_ms
                    .insert(turn_id.clone(), started.elapsed().as_millis() as u64);
            }
            let started_at_ms = store
                .turns
                .iter()
                .find(|turn| turn.turn_id == turn_id)
                .map(|turn| turn.started_at_ms)
                .unwrap_or_default();
            upsert_turn(store, turn_id, started_at_ms, |turn| {
                turn.terminal = Some(terminal);
                turn.phase = None;
                turn.usage = Some(usage);
            });
        }
        SessionEvent::AnswerDelta { block_id, text, .. } => {
            store.note_stream_chars(text.len() as i64);
            append_stream(store, block_id, BlockKind::Answer, &text);
            arm_trace_tick(store, cx);
        }
        SessionEvent::ReasoningDelta { block_id, text, .. } => {
            append_stream(store, block_id, BlockKind::Reasoning, &text);
            arm_trace_tick(store, cx);
        }
        SessionEvent::BlockFinal { block_id, content, .. } => {
            store.streaming.retain(|slot| slot.0 != block_id);
            if let Some(block) = store
                .entries
                .iter_mut()
                .find(|block| block.block_id == block_id)
            {
                block.content = content;
            }
        }
        SessionEvent::ToolArgumentsDelta {
            tool_call_id,
            tool_name,
            delta,
            lines_added,
            lines_removed,
            target_files,
            args_complete,
            ..
        } => {
            arm_trace_tick(store, cx);
            match store
                .arg_stream
                .iter_mut()
                .find(|slot| slot.call_id == tool_call_id)
            {
                Some(slot) => {
                    slot.tool_name = tool_name;
                    slot.text.push_str(&delta);
                    slot.lines = lines_added.zip(lines_removed);
                    slot.target_files = target_files;
                    slot.args_complete = args_complete;
                }
                None => store.arg_stream.push(ArgStream {
                    call_id: tool_call_id,
                    tool_name,
                    text: delta,
                    lines: lines_added.zip(lines_removed),
                    target_files,
                    args_complete,
                }),
            }
        }
        SessionEvent::ToolCallUpdated { call, .. } => {
            arm_trace_tick(store, cx);
            // V3 保证每次都带完整视图:整条替换,不做逐字段合并。
            store.arg_stream.retain(|slot| slot.call_id != call.tool_call_id);
            upsert_tool_view(store, call);
        }
        SessionEvent::ToolOutputDelta {
            tool_call_id, text, ..
        } => match store
            .output_tail
            .iter_mut()
            .find(|slot| slot.0 == tool_call_id)
        {
            Some(slot) => slot.1.push_str(&text),
            None => store.output_tail.push((tool_call_id, text)),
        },
        SessionEvent::TimelineAppended { block, .. } => {
            if matches!(block.kind, BlockKind::Answer | BlockKind::Reasoning) {
                store.streaming.retain(|slot| slot.0 != block.block_id);
            }
            upsert_block(store, block);
        }
        SessionEvent::TimelineTruncated {
            removed_turn_ids,
            timeline_epoch,
            ..
        } => {
            // 权威集合整枝:回合(输入/正文/工具卡/交互)一并消失;副作用
            // (journal/exec/子代理)不动。无 turn 归属的块无法判定,保守保留。
            store.timeline_epoch = timeline_epoch;
            let removed: HashSet<TurnId> = removed_turn_ids.into_iter().collect();
            let removed_calls: HashSet<ToolCallId> = store
                .tools
                .iter()
                .filter(|tool| tool.turn_id.as_ref().is_some_and(|turn_id| removed.contains(turn_id)))
                .map(|tool| tool.tool_call_id.clone())
                .collect();
            store.turns.retain(|turn| !removed.contains(&turn.turn_id));
            store.tools.retain(|tool| {
                !tool
                    .turn_id
                    .as_ref()
                    .is_some_and(|turn_id| removed.contains(turn_id))
            });
            store.interactions.retain(|interaction| {
                !interaction
                    .turn_id
                    .as_ref()
                    .is_some_and(|turn_id| removed.contains(turn_id))
            });
            store
                .arg_stream
                .retain(|slot| !removed_calls.contains(&slot.call_id));
            store
                .output_tail
                .retain(|(call_id, _)| !removed_calls.contains(call_id));
            store.entries.retain(|block| {
                block
                    .turn_id
                    .as_ref()
                    .is_none_or(|turn_id| !removed.contains(turn_id))
            });
            store.expanded_groups.clear();
            store.expanded_traces.clear();
            store.collapsed_traces.clear();
            store.trace_scroll_handles.clear();
            store.trace_started_at.clear();
            store.trace_elapsed_ms.clear();
            store.timeline_list.reset(store.entries.len() + 1);
        }
        SessionEvent::InteractionRequested { interaction, .. } => {
            match store
                .interactions
                .iter_mut()
                .find(|slot| slot.interaction_id == interaction.interaction_id)
            {
                Some(slot) => *slot = interaction,
                None => store.interactions.push(interaction),
            }
        }
        SessionEvent::InteractionResolved { interaction_id, .. } => {
            store.ask_drafts.retain(|(id, _)| id != &interaction_id);
            store.ask_hint = None;
            store
                .interactions
                .retain(|slot| slot.interaction_id != interaction_id);
        }
        SessionEvent::ExecSessionUpdated { exec, .. } => {
            if exec.finished {
                store
                    .exec_output_tail
                    .retain(|slot| slot.0 != exec.exec_session_id);
            }
            match store
                .exec_sessions
                .iter_mut()
                .find(|slot| slot.exec_session_id == exec.exec_session_id)
            {
                Some(slot) => *slot = exec,
                None => store.exec_sessions.push(exec),
            }
        }
        SessionEvent::ExecOutputDelta {
            exec_session_id,
            start_offset,
            end_offset,
            text,
            ..
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
            delay_ms,
            reason,
            ..
        } => {
            store.provider_retry = Some(crate::store::ProviderRetry {
                attempt,
                max_attempts,
                delay_secs: delay_ms / 1000,
                error: reason,
            });
        }
        SessionEvent::SubagentUpdated { subagent, .. } => {
            match store
                .subagents
                .iter_mut()
                .find(|slot| slot.session_id == subagent.session_id)
            {
                Some(slot) => *slot = subagent,
                None => store.subagents.push(subagent),
            }
        }
        SessionEvent::SessionMetaChanged { title, .. } => {
            store.title = title.clone();
            if let Some(settings) = store.settings.as_mut() {
                settings.title = title;
            }
        }
        SessionEvent::GoalUpdated { goal, .. } => {
            // 整字段替换;None = 已清除。链根与当前会话双流可能都发,幂等。
            store.goal = goal;
        }
        SessionEvent::PinnedCompactionChanged { summary, .. } => {
            store.pinned_compaction = summary;
        }
        SessionEvent::DriverChanged {
            driver_client_instance_id,
            ordinal,
            ..
        } => {
            store.driver_client_instance_id = Some(driver_client_instance_id);
            store.driver_ordinal = ordinal;
        }
        SessionEvent::CatalogChanged { .. } => {
            let _ = store.requests.unbounded_send(qaqh_gpui_wire::bridge::BridgeRequest::Config(
                qaqh_ringing_v3::ConfigRequest::ListProviders,
            ));
        }
        SessionEvent::HandoffSealed {
            sealed_session_id,
            successor_session_id,
            turn_id,
            summary,
            at_ms,
            ..
        } => {
            let work_summary = store.content_text(&summary);
            // ADR-0016 纯信息面:双发到封箱线与后继线。
            if sealed_session_id == store.session_id {
                let model_auto = store
                    .tools
                    .iter()
                    .find(|tool| {
                        tool.tool_name == "handoff" && tool.turn_id.as_ref() == Some(&turn_id)
                    })
                    .and_then(|tool| {
                        crate::store::handoff_facts(tool, |content| store.content_text(content))
                    })
                    .map(|facts| facts.auto_continue)
                    .unwrap_or(true);
                store.handoff_seal = Some(HandoffSeal {
                    sealed_session_id: sealed_session_id.as_str().to_owned(),
                    successor_session_id: successor_session_id.as_str().to_owned(),
                    work_summary: work_summary.clone(),
                    at_ms,
                });
                let should_jump = store.handoff_eager || (!store.handoff_stay && model_auto);
                store.handoff_auto_jump =
                    should_jump.then(|| successor_session_id.as_str().to_owned());
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
        SessionEvent::PromptBlocked { reason, .. } => {
            store.operation_error = Some(format!("输入被 Hook 拦截：{reason}"));
        }
        SessionEvent::SessionRemoved { session_id } => {
            store.operation_error = Some("会话已被删除".into());
            cx.emit(crate::store::SessionNavEvent::SessionRemoved { session_id });
        }
        SessionEvent::ResyncRequired { reason, .. } => {
            // 两类真缺口才走这里:清暂态,整份重取快照(禁止拼接)。
            store.snapshot_pending = true;
            clear_transient(store);
            let _ = store
                .requests
                .unbounded_send(qaqh_gpui_wire::bridge::BridgeRequest::Refill(
                    store.session_id.clone(),
                ));
            store.operation_error = Some(format!("时间线需要重新同步（{reason:?}）"));
        }
    }
}

/// 过程实时计时 ticker:仅在 `store.is_running()` 时每秒刷新一次界面以驱动「1分22秒」走表,
/// 会话离开 Running 即自动停表退出,空闲零 CPU 消耗。
fn arm_trace_tick(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    if !store.is_running() || store.trace_ticking {
        return;
    }
    store.trace_ticking = true;
    cx.spawn(async move |entity, cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            let keep_going = entity
                .update(cx, |store, cx| {
                    let active = store.is_running();
                    if !active {
                        store.trace_ticking = false;
                    }
                    cx.notify();
                    active
                })
                .unwrap_or(false);
            if !keep_going {
                break;
            }
        }
    })
    .detach();
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

/// 工具视图整条替换(V3 `ToolCallUpdated` 每次都给完整视图)。
fn upsert_tool_view(store: &mut SessionStore, call: ToolCallView) {
    match store
        .tools
        .iter_mut()
        .find(|tool| tool.tool_call_id == call.tool_call_id)
    {
        Some(slot) => *slot = call,
        None => store.tools.push(call),
    }
}

fn upsert_turn(
    store: &mut SessionStore,
    turn_id: TurnId,
    started_at_ms: u64,
    update: impl FnOnce(&mut TurnSummary),
) {
    if let Some(turn) = store.turns.iter_mut().find(|turn| turn.turn_id == turn_id) {
        update(turn);
    } else {
        store.turns.push(TurnSummary {
            turn_id,
            input_id: None,
            ordinal: store.turns.len() as u64,
            phase: None,
            terminal: None,
            usage: None,
            started_at_ms,
        });
        update(store.turns.last_mut().expect("just pushed"));
    }
}

/// 块按 `ord` 排序插入(增量到达序不可信);同 `block_id` 幂等替换。
fn upsert_block(store: &mut SessionStore, block: BlockView) {
    if let Some(slot) = store
        .entries
        .iter_mut()
        .find(|slot| slot.block_id == block.block_id)
    {
        *slot = block;
        return;
    }
    let position = store.entries.partition_point(|slot| slot.ord < block.ord);
    store.entries.insert(position, block);
}

/// 流式增量:**追加**(V3 `AnswerDelta`/`ReasoningDelta` 是真增量)。
fn append_stream(store: &mut SessionStore, block_id: BlockId, kind: BlockKind, delta: &str) {
    match store
        .streaming
        .iter_mut()
        .find(|slot| slot.0 == block_id)
    {
        Some(slot) => slot.2.push_str(delta),
        None => store
            .streaming
            .push((block_id, kind, delta.to_owned())),
    }
}

/// 终态提示(供 shell 侧渲染复用)。
pub fn terminal_label(terminal: &TurnTerminal) -> String {
    match terminal {
        TurnTerminal::Completed => "已完成".into(),
        TurnTerminal::Failed { code, message } => format!("失败（{code}）：{message}"),
        TurnTerminal::Cancelled { code } => format!("已取消（{code}）"),
        TurnTerminal::Interrupted { code } => format!("已中断（{code}）"),
    }
}

