//! Pure projection of canonical [`ChannelOutputEvent`]s onto the web wire.

use serde_json::{Value, json};
use tinychannels_bus::channel::OutboundContentPart;
use tinychannels_bus::module_progressive::{ChannelOutputEvent, SubagentOutput};
use tinychannels_bus::web::{
    WebChannelEvent, WebCostSource, WebEventContext, WebSubagentProgressDetail,
    WebTurnUsagePayload, cap_web_args, cap_web_output,
};

/// Stateful projector for one turn.
///
/// It stamps every emitted event with the turn's addressing context and a
/// strictly increasing `seq`, which is what lets a web client order events that
/// arrive over more than one transport.
#[derive(Clone, Debug)]
pub struct WebTurnProjector {
    context: WebEventContext,
    next_seq: u64,
}

impl WebTurnProjector {
    /// Start a turn. `seq` begins at 1.
    pub fn new(client_id: &str, thread_id: &str, request_id: &str) -> Self {
        Self {
            context: WebEventContext {
                client_id: client_id.to_owned(),
                thread_id: thread_id.to_owned(),
                request_id: request_id.to_owned(),
                turn_request_id: None,
                seq: None,
                ts: None,
            },
            next_seq: 1,
        }
    }

    /// Attach the originating turn id for events of a follow-up turn.
    pub fn with_turn_request_id(mut self, turn_request_id: impl Into<String>) -> Self {
        self.context.turn_request_id = Some(turn_request_id.into());
        self
    }

    /// The addressing context without a sequence stamp.
    pub fn context(&self) -> &WebEventContext {
        &self.context
    }

    /// Project one output event. The result is empty when the web wire has no
    /// representation for it (for example `Native` provider data), and every
    /// returned event carries the next `seq` and `ts`.
    pub fn project(&mut self, event: ChannelOutputEvent, ts_ms: u64) -> Vec<WebChannelEvent> {
        let Some(mut projected) = project_event(&self.context, event) else {
            tracing::trace!(
                thread_id = %self.context.thread_id,
                request_id = %self.context.request_id,
                "web projector: output event has no web representation"
            );
            return Vec::new();
        };
        for event in &mut projected {
            let seq = self.next_seq;
            self.next_seq += 1;
            let context = event.context_mut();
            context.seq = Some(seq);
            context.ts = Some(ts_ms);
        }
        projected
    }
}

/// Mutable access to the shared addressing fields of any projected event.
pub fn context_mut(event: &mut WebChannelEvent) -> &mut WebEventContext {
    event.context_mut()
}

fn project_event(ctx: &WebEventContext, event: ChannelOutputEvent) -> Option<Vec<WebChannelEvent>> {
    use ChannelOutputEvent as O;
    let context = ctx.clone();
    let one = |event| Some(vec![event]);
    match event {
        O::TextDelta { text, iteration } => one(WebChannelEvent::TextDelta {
            context,
            delta: Some(text),
            delta_kind: Some("text".into()),
            round: iteration,
        }),
        O::ThinkingDelta { text, iteration } => one(WebChannelEvent::ThinkingDelta {
            context,
            delta: Some(text),
            delta_kind: Some("thinking".into()),
            round: iteration,
        }),
        O::FinalMessage { text, parts, .. } => one(WebChannelEvent::ChatDone {
            context,
            full_response: Some(text),
            citations: None,
            usage: None,
            timing: None,
            parts: non_empty(parts),
        }),
        O::ToolProgress { label, message } => one(WebChannelEvent::ToolProgress {
            context,
            tool_call_id: None,
            message: Some(message.unwrap_or(label)),
        }),
        O::ToolCallStarted {
            call_id,
            tool_name,
            arguments,
            iteration,
            display_label,
            display_detail,
        } => one(WebChannelEvent::ToolCall {
            context,
            tool_name: Some(tool_name),
            skill_id: None,
            args: Some(arguments),
            round: Some(iteration),
            tool_call_id: Some(call_id),
            tool_display_label: display_label,
            tool_display_detail: display_detail,
        }),
        O::ToolCallArgsDelta {
            call_id,
            tool_name,
            delta,
            iteration,
        } => one(WebChannelEvent::ToolArgsDelta {
            context,
            delta: Some(delta),
            delta_kind: Some("tool_args".into()),
            tool_call_id: Some(call_id),
            tool_name: Some(tool_name),
            skill_id: None,
            round: Some(iteration),
        }),
        O::ToolCallProgress { call_id, message } => one(WebChannelEvent::ToolProgress {
            context,
            tool_call_id: Some(call_id),
            message: Some(message),
        }),
        O::ToolCallCompleted {
            call_id,
            tool_name,
            success,
            output,
            arguments,
            elapsed_ms,
            iteration,
            failure,
            display_label,
            display_detail,
            structured,
            ..
        } => one(WebChannelEvent::ToolResult {
            context,
            tool_name: Some(tool_name),
            skill_id: None,
            output: Some(cap_web_output(output)),
            args: arguments,
            success: Some(success),
            round: Some(iteration),
            tool_call_id: Some(call_id),
            failure: failure.and_then(|failure| serde_json::to_value(failure).ok()),
            elapsed_ms: Some(elapsed_ms),
            tool_display_label: display_label,
            tool_display_detail: display_detail,
            structured,
        }),
        O::ApprovalRequest {
            id,
            prompt,
            choices,
        } => one(WebChannelEvent::ApprovalRequest {
            context,
            tool_name: None,
            message: Some(prompt),
            args: Some(json!({ "choices": choices })),
            tool_call_id: Some(id),
            expires_at: None,
            detached: None,
        }),
        O::Clarification { prompt } => one(WebChannelEvent::ChatInterim {
            context,
            full_response: Some(prompt),
            round: None,
        }),
        O::Media { text, parts } => one(WebChannelEvent::ChatMessage {
            context,
            message: None,
            full_response: text,
            parts: non_empty(
                parts
                    .into_iter()
                    .map(|part| OutboundContentPart::Content { part })
                    .collect(),
            ),
        }),
        O::VoiceNote { text, voice } => one(WebChannelEvent::ChatMessage {
            context,
            message: None,
            full_response: text,
            parts: Some(vec![OutboundContentPart::Content {
                part: tinychannels_bus::channel::ContentPart::VoiceNote { voice_note: voice },
            }]),
        }),
        O::Subagent(output) => project_subagent(context, *output).map(|event| vec![event]),
        O::ModelUsage {
            subagent_task_id: Some(_),
            ..
        } => None,
        O::ModelUsage {
            iteration,
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cost_usd,
            ..
        } => one(turn_cost(
            context,
            Some(iteration),
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cost_usd,
        )),
        O::Usage {
            iteration,
            input_tokens,
            output_tokens,
            cached_input_tokens,
            total_usd,
            ..
        } => one(turn_cost(
            context,
            iteration,
            input_tokens,
            output_tokens,
            cached_input_tokens,
            total_usd,
        )),
        O::Artifact {
            operation,
            artifact,
        } => {
            let args = Some(artifact);
            match operation.as_str() {
                "ready" => one(WebChannelEvent::ArtifactReady {
                    context,
                    tool_call_id: None,
                    args,
                }),
                "failed" => one(WebChannelEvent::ArtifactFailed {
                    context,
                    tool_call_id: None,
                    args,
                }),
                "pending" => one(WebChannelEvent::ArtifactPending {
                    context,
                    tool_call_id: None,
                    args,
                }),
                _ => None,
            }
        }
        O::Cancellation { reason } => one(WebChannelEvent::ChatCancelled {
            context,
            cancel_reason: reason,
            superseded_by: None,
        }),
        O::Lifecycle {
            stage,
            message,
            iteration,
            ..
        } => match stage.as_str() {
            "inference_start" => one(WebChannelEvent::InferenceStart { context }),
            "inference_heartbeat" => one(WebChannelEvent::InferenceHeartbeat { context }),
            "iteration_start" => one(WebChannelEvent::IterationStart {
                context,
                message,
                round: iteration,
            }),
            "context_compacting" => one(WebChannelEvent::ContextCompacting {
                context,
                round: iteration,
            }),
            _ => None,
        },
        O::Native { .. } => None,
    }
}

fn non_empty(parts: Vec<OutboundContentPart>) -> Option<Vec<OutboundContentPart>> {
    (!parts.is_empty()).then_some(parts)
}

fn turn_cost(
    context: WebEventContext,
    round: Option<u32>,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    cost_usd: Option<f64>,
) -> WebChannelEvent {
    WebChannelEvent::TurnCost {
        context,
        round,
        usage: Some(WebTurnUsagePayload {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cost_usd,
            cost_source: if cost_usd.is_some() {
                WebCostSource::Charged
            } else {
                WebCostSource::Unknown
            },
            ..Default::default()
        }),
    }
}

/// Map a subagent progress record by its `state` to the matching
/// `subagent_*` web event. Unknown states have no web representation.
fn project_subagent(context: WebEventContext, out: SubagentOutput) -> Option<WebChannelEvent> {
    let detail = Some(subagent_detail(&out));
    let skill_id = Some(out.task_id.clone());
    let round = out.iteration;
    let message = out.message.clone();
    let tool_name = out.tool_name.clone();
    let success = out.success;
    let tool_call_id = out.call_id.clone();
    let output = out.output.clone().map(cap_web_output);
    let args = cap_web_args(out.arguments.clone());
    let failure: Option<Value> = out
        .failure
        .as_ref()
        .and_then(|failure| serde_json::to_value(failure).ok());
    Some(match out.state.as_str() {
        "spawned" => WebChannelEvent::SubagentSpawned {
            context,
            message,
            tool_name,
            skill_id,
            round,
            subagent: detail,
        },
        "completed" => WebChannelEvent::SubagentCompleted {
            context,
            message,
            tool_name,
            skill_id,
            success,
            round,
            subagent: detail,
        },
        "failed" => WebChannelEvent::SubagentFailed {
            context,
            message,
            tool_name,
            skill_id,
            success,
            subagent: detail,
            round,
        },
        "awaiting_user" => WebChannelEvent::SubagentAwaitingUser {
            context,
            message,
            tool_name,
            skill_id,
            success,
            round,
            subagent: detail,
        },
        "iteration_start" => WebChannelEvent::SubagentIterationStart {
            context,
            message,
            tool_name,
            skill_id,
            round,
            subagent: detail,
        },
        "tool_call" => WebChannelEvent::SubagentToolCall {
            context,
            tool_name,
            skill_id,
            args,
            round,
            tool_call_id,
            tool_display_label: out.display_label,
            tool_display_detail: out.display_detail,
            subagent: detail,
        },
        "tool_result" => WebChannelEvent::SubagentToolResult {
            context,
            tool_name,
            skill_id,
            success,
            round,
            tool_call_id,
            output,
            args,
            elapsed_ms: out.elapsed_ms,
            tool_display_label: out.display_label,
            tool_display_detail: out.display_detail,
            structured: out.structured,
            failure,
            subagent: detail,
        },
        "text_delta" => WebChannelEvent::SubagentTextDelta {
            context,
            delta: message,
            delta_kind: Some("text".into()),
            round,
            skill_id,
            subagent: detail,
        },
        "thinking_delta" => WebChannelEvent::SubagentThinkingDelta {
            context,
            delta: message,
            delta_kind: Some("thinking".into()),
            round,
            skill_id,
            subagent: detail,
        },
        _ => return None,
    })
}

fn subagent_detail(out: &SubagentOutput) -> WebSubagentProgressDetail {
    let usage = out.usage.as_ref();
    // A finished child reports its total iteration count and final text on the
    // detail; every other state carries neither.
    let completed = out.state == "completed";
    WebSubagentProgressDetail {
        mode: out.mode.clone(),
        dedicated_thread: out.dedicated_thread,
        prompt_chars: out.prompt_chars.map(|n| n as u64),
        child_iteration: out.iteration,
        child_max_iterations: out.max_iterations,
        agent_id: Some(out.agent_id.clone()),
        task_id: Some(out.task_id.clone()),
        elapsed_ms: out.elapsed_ms,
        input_tokens: usage.map(|u| u.input_tokens),
        output_tokens: usage.map(|u| u.output_tokens),
        cached_input_tokens: usage.map(|u| u.cached_input_tokens),
        cost_usd: usage.map(|u| u.charged_amount_usd),
        iterations: if completed { out.iteration } else { None },
        output_chars: out.output_chars.map(|n| n as u64),
        worker_thread_id: out.worker_thread_id.clone(),
        display_name: out.display_name.clone(),
        worktree_path: out.worktree_path.clone(),
        changed_files: (!out.changed_files.is_empty()).then(|| out.changed_files.clone()),
        dirty_status: out.dirty_status,
        parent_call_id: out.parent_call_id.clone(),
        output: if completed {
            out.output.clone().map(cap_web_output)
        } else {
            None
        },
    }
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
