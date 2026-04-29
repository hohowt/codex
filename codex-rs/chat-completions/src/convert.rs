//! Converts a `ResponsesApiRequest` into a Chat Completions API JSON body.

use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;

/// Builds a Chat Completions API request body from a Responses API request.
///
/// `diagnostics` collects warning messages about missing reasoning_content etc.
/// that the caller should display to the user (e.g. via TUI WarningEvent).
pub fn build_chat_request(
    model: &str,
    instructions: &str,
    input: &[ResponseItem],
    tools: &[Value],
    parallel_tool_calls: bool,
    supports_stream_options: bool,
    reasoning_effort: Option<&str>,
    diagnostics: &mut Vec<String>,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let sanitized_input_storage;
    let input = if reasoning_effort.is_some() {
        sanitized_input_storage = sanitize_input_for_thinking_tool_replay(input, diagnostics);
        sanitized_input_storage.as_slice()
    } else {
        input
    };

    // System message from instructions.
    if !instructions.is_empty() {
        messages.push(json!({
            "role": "system",
            "content": instructions,
        }));
    }

    // Track raw reasoning_content from a Reasoning item so it can be
    // attached to a subsequent assistant message with tool_calls. DeepSeek
    // requires reasoning_content on *every* tool-call assistant message in
    // the conversation.  A single model turn may produce multiple groups of
    // tool_calls (e.g. model returns tool_calls, gets results, and continues
    // with another tool_calls).  We use turn-level tracking so that the
    // same reasoning is reused for later tool_call groups when the model
    // does not produce fresh reasoning for the continuation.
    let mut turn_reasoning_content: Option<String> = None;
    let mut last_assistant_message_index: Option<usize> = None;
    let mut current_turn_has_tool_calls = false;

    // Convert each ResponseItem into one or more Chat Completions messages.
    for item in input {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let text = content_items_to_text(content);
                // Map OpenAI-specific roles to standard Chat Completions roles.
                // "developer" is an OpenAI-only role; most providers only accept
                // "system", "user", "assistant".
                let mapped_role = match role.as_str() {
                    "developer" => "system",
                    other => other,
                };
                if mapped_role == "assistant" {
                    if reasoning_effort.is_some()
                        && let Some(index) = last_assistant_message_index
                        && let Some(last) = messages.get_mut(index)
                        && last.get("role").and_then(Value::as_str) == Some("assistant")
                        && last.get("tool_calls").is_none()
                    {
                        let existing = last
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let merged = match (existing.is_empty(), text.is_empty()) {
                            (true, _) => text.clone(),
                            (_, true) => existing.to_string(),
                            (false, false) => format!("{existing}\n{text}"),
                        };
                        last["content"] = json!(merged);
                    } else {
                        messages.push(json!({
                            "role": "assistant",
                            "content": text,
                        }));
                        last_assistant_message_index = Some(messages.len().saturating_sub(1));
                    }
                } else {
                    // Non-assistant messages break the reasoning→tool_call chain.
                    turn_reasoning_content = None;
                    last_assistant_message_index = None;
                    current_turn_has_tool_calls = false;
                    messages.push(json!({
                        "role": mapped_role,
                        "content": text,
                    }));
                }
            }
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                // Append as an assistant message with tool_calls.
                // If the last message is already an assistant with tool_calls, merge.
                let tool_call = json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments,
                    }
                });
                current_turn_has_tool_calls = true;
                if let Some(last) = messages.last_mut()
                    && last.get("role").and_then(Value::as_str) == Some("assistant")
                    && last.get("tool_calls").is_some()
                {
                    last["tool_calls"].as_array_mut().unwrap().push(tool_call);
                    // Attach turn-level reasoning if not already present.
                    if last.get("reasoning_content").is_none()
                        && let Some(rc) = turn_reasoning_content.clone() {
                            last["reasoning_content"] = json!(rc);
                        }
                } else if let Some(index) = last_assistant_message_index
                    && let Some(last) = messages.get_mut(index)
                    && last.get("tool_calls").is_none()
                {
                    last["tool_calls"] = json!([tool_call]);
                    if let Some(rc) = turn_reasoning_content.clone() {
                        last["reasoning_content"] = json!(rc);
                    }
                } else {
                    let mut msg = json!({
                    "role": "assistant",
                    "content": null,
                        "tool_calls": [tool_call],
                    });
                    if let Some(rc) = turn_reasoning_content.clone() {
                        msg["reasoning_content"] = json!(rc);
                    }
                    messages.push(msg);
                }
                last_assistant_message_index = None;
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                // Keep turn_reasoning_content so later tool calls in the same
                // turn can reuse it.  Reset only the per-message tracker.
                last_assistant_message_index = None;
                let text = function_output_to_string(output);
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": text,
                }));
            }
            ResponseItem::LocalShellCall {
                call_id, action, ..
            } => {
                use codex_protocol::models::LocalShellAction;
                let command = match action {
                    LocalShellAction::Exec(exec) => &exec.command,
                };
                let args = json!({
                    "command": command,
                });
                let tool_call = json!({
                    "id": call_id.as_deref().unwrap_or(""),
                    "type": "function",
                    "function": {
                        "name": "shell",
                        "arguments": args.to_string(),
                    }
                });
                current_turn_has_tool_calls = true;
                if let Some(last) = messages.last_mut()
                    && last.get("role").and_then(Value::as_str) == Some("assistant")
                    && last.get("tool_calls").is_some()
                {
                    last["tool_calls"].as_array_mut().unwrap().push(tool_call);
                    // Attach turn-level reasoning if not already present.
                    if last.get("reasoning_content").is_none()
                        && let Some(rc) = turn_reasoning_content.clone() {
                            last["reasoning_content"] = json!(rc);
                        }
                } else if let Some(index) = last_assistant_message_index
                    && let Some(last) = messages.get_mut(index)
                    && last.get("tool_calls").is_none()
                {
                    last["tool_calls"] = json!([tool_call]);
                    if let Some(rc) = turn_reasoning_content.clone() {
                        last["reasoning_content"] = json!(rc);
                    }
                } else {
                    let mut msg = json!({
                    "role": "assistant",
                    "content": null,
                        "tool_calls": [tool_call],
                    });
                    if let Some(rc) = turn_reasoning_content.clone() {
                        msg["reasoning_content"] = json!(rc);
                    }
                    messages.push(msg);
                }
                last_assistant_message_index = None;
            }
            ResponseItem::Reasoning {
                summary, content, ..
            } => {
                // Store raw reasoning content for passing back to the API
                // on a subsequent replay turn (required by DeepSeek thinking mode).
                if let Some(content_items) = content {
                    let raw_text: String = content_items
                        .iter()
                        .filter_map(|c| match c {
                            ReasoningItemContent::ReasoningText { text } => Some(text.as_str()),
                            ReasoningItemContent::Text { text } => Some(text.as_str()),
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !raw_text.is_empty() {
                        if current_turn_has_tool_calls
                            && let Some(index) = last_assistant_message_index
                            && let Some(last) = messages.get_mut(index)
                            && last.get("role").and_then(Value::as_str) == Some("assistant")
                            && last.get("tool_calls").is_none()
                            && last.get("reasoning_content").is_none()
                        {
                            last["reasoning_content"] = json!(raw_text);
                            turn_reasoning_content = Some(raw_text);
                        } else {
                            turn_reasoning_content = Some(raw_text);
                        }
                    }
                }
                // Include reasoning summaries as assistant context.
                use codex_protocol::models::ReasoningItemReasoningSummary;
                let text: String = summary
                    .iter()
                    .map(|s| match s {
                        ReasoningItemReasoningSummary::SummaryText { text } => text.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": format!("<reasoning>\n{text}\n</reasoning>"),
                    }));
                    last_assistant_message_index = Some(messages.len().saturating_sub(1));
                }
            }
            // Skip items that have no Chat Completions equivalent.
            _ => {}
        }
    }

    // Defensive: rewrite any assistant message that has tool_calls but no
    // reasoning_content when thinking mode is active. DeepSeek requires
    // reasoning_content on every tool-call turn in all subsequent requests.
    if reasoning_effort.is_some() {
        let mut rewritten_count = 0usize;
        let mut rewritten_call_ids = HashSet::new();
        for (i, msg) in messages.iter_mut().enumerate() {
            if msg.get("role").and_then(Value::as_str) == Some("assistant")
                && msg.get("tool_calls").is_some()
                && msg.get("reasoning_content").is_none()
            {
                let mut transcript_parts = Vec::new();
                if let Some(content) = msg.get("content").and_then(Value::as_str)
                    && !content.is_empty()
                {
                    transcript_parts.push(content.to_string());
                }

                let tool_count =
                    if let Some(tc_array) = msg.get("tool_calls").and_then(|tc| tc.as_array()) {
                        for tc in tc_array {
                            let call_id = tc.get("id").and_then(Value::as_str);
                            if let Some(id) = call_id {
                                rewritten_call_ids.insert(id.to_string());
                            }
                            let name = tc
                                .get("function")
                                .and_then(|function| function.get("name"))
                                .and_then(Value::as_str)
                                .unwrap_or("tool");
                            let arguments = tc
                                .get("function")
                                .and_then(|function| function.get("arguments"))
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            transcript_parts.push(replayed_tool_call_transcript(
                                name,
                                call_id,
                                "arguments",
                                arguments,
                            ));
                        }
                        tc_array.len()
                    } else {
                        0
                    };

                msg.as_object_mut().unwrap().remove("tool_calls");
                msg["content"] = json!(transcript_parts.join("\n"));
                rewritten_count += 1;
                let msg = format!(
                    "[reasoning_content] build_chat_request 防御性改写: 消息 #{i} 含 {tool_count} 个 tool_call 但无 reasoning_content，已转为 assistant transcript"
                );
                diagnostics.push(msg);
            }
        }

        let mut rewritten_output_count = 0usize;
        for msg in &mut messages {
            if msg.get("role").and_then(Value::as_str) == Some("tool")
                && let Some(tool_call_id) = msg.get("tool_call_id").and_then(Value::as_str)
                && rewritten_call_ids.contains(tool_call_id)
            {
                let content = msg
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                *msg = json!({
                    "role": "assistant",
                    "content": replayed_tool_output_transcript(tool_call_id, None, &content),
                });
                rewritten_output_count += 1;
            }
        }

        if rewritten_count > 0 {
            let msg = format!(
                "[reasoning_content] build_chat_request: 共改写 {rewritten_count} 条无 reasoning 的 tool_call 消息"
            );
            diagnostics.push(msg);
        }
        if rewritten_output_count > 0 {
            let msg = format!(
                "[reasoning_content] build_chat_request: 额外改写 {rewritten_output_count} 条关联 tool output"
            );
            diagnostics.push(msg);
        }
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });

    // Convert Responses API tools to Chat Completions tools format.
    let chat_tools = convert_tools(tools);
    if !chat_tools.is_empty() {
        body["tools"] = Value::Array(chat_tools);
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(parallel_tool_calls);
    }

    if supports_stream_options {
        body["stream_options"] = json!({"include_usage": true});
    }

    if let Some(effort) = reasoning_effort {
        body["thinking"] = json!({"type": "enabled"});
        body["reasoning_effort"] = json!(effort);
    }

    body
}

fn sanitize_input_for_thinking_tool_replay(
    input: &[ResponseItem],
    diagnostics: &mut Vec<String>,
) -> Vec<ResponseItem> {
    let mut sanitized = Vec::new();
    let mut model_turn_segment = Vec::new();

    for item in input {
        if is_non_assistant_message(item) {
            sanitized.extend(sanitize_model_turn_segment(
                &model_turn_segment,
                diagnostics,
            ));
            model_turn_segment.clear();
            sanitized.push(item.clone());
        } else {
            model_turn_segment.push(item.clone());
        }
    }

    sanitized.extend(sanitize_model_turn_segment(
        &model_turn_segment,
        diagnostics,
    ));
    sanitized
}

fn sanitize_model_turn_segment(
    segment: &[ResponseItem],
    diagnostics: &mut Vec<String>,
) -> Vec<ResponseItem> {
    let mut sanitized = Vec::new();
    let mut saw_reasoning_content = false;
    let mut rewritten_tool_calls = 0usize;
    let mut rewritten_tool_outputs = 0usize;
    let mut rewritten_call_ids = HashSet::new();

    for item in segment {
        match item {
            ResponseItem::Reasoning { content, .. } => {
                if content.as_ref().is_some_and(|items| {
                    items.iter().any(|item| match item {
                        ReasoningItemContent::ReasoningText { text }
                        | ReasoningItemContent::Text { text } => !text.is_empty(),
                    })
                }) {
                    saw_reasoning_content = true;
                }
                sanitized.push(item.clone());
            }
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                if saw_reasoning_content {
                    sanitized.push(item.clone());
                } else {
                    rewritten_call_ids.insert(call_id.clone());
                    rewritten_tool_calls += 1;
                    sanitized.push(assistant_transcript_message(replayed_tool_call_transcript(
                        name,
                        Some(call_id),
                        "arguments",
                        arguments,
                    )));
                }
            }
            ResponseItem::CustomToolCall {
                call_id,
                name,
                input,
                ..
            } => {
                if saw_reasoning_content {
                    sanitized.push(item.clone());
                } else {
                    rewritten_call_ids.insert(call_id.clone());
                    rewritten_tool_calls += 1;
                    sanitized.push(assistant_transcript_message(replayed_tool_call_transcript(
                        name,
                        Some(call_id),
                        "input",
                        input,
                    )));
                }
            }
            ResponseItem::LocalShellCall {
                call_id, action, ..
            } => {
                if saw_reasoning_content {
                    sanitized.push(item.clone());
                } else {
                    use codex_protocol::models::LocalShellAction;
                    let command = match action {
                        LocalShellAction::Exec(exec) => &exec.command,
                    };
                    let arguments = json!({
                        "command": command,
                    })
                    .to_string();
                    if let Some(call_id) = call_id {
                        rewritten_call_ids.insert(call_id.clone());
                    }
                    rewritten_tool_calls += 1;
                    sanitized.push(assistant_transcript_message(replayed_tool_call_transcript(
                        "shell",
                        call_id.as_deref(),
                        "arguments",
                        &arguments,
                    )));
                }
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                if rewritten_call_ids.contains(call_id) {
                    rewritten_tool_outputs += 1;
                    sanitized.push(assistant_transcript_message(
                        replayed_tool_output_transcript(call_id, None, &output.to_string()),
                    ));
                } else {
                    sanitized.push(item.clone());
                }
                // Do NOT reset saw_reasoning_content here — main conversion loop
                // keeps turn_reasoning_content for the entire turn so subsequent
                // tool calls reuse it. Resetting would cause the next FunctionCall
                // to be rewritten as text, losing reasoning_content that DeepSeek
                // thinking mode requires.
            }
            ResponseItem::CustomToolCallOutput {
                call_id,
                name,
                output,
            } => {
                if rewritten_call_ids.contains(call_id) {
                    rewritten_tool_outputs += 1;
                    sanitized.push(assistant_transcript_message(
                        replayed_tool_output_transcript(
                            call_id,
                            name.as_deref(),
                            &output.to_string(),
                        ),
                    ));
                } else {
                    sanitized.push(item.clone());
                }
                // Do NOT reset saw_reasoning_content here — same reasoning as above.
            }
            ResponseItem::ToolSearchOutput {
                call_id,
                status,
                execution,
                tools,
            } => {
                if let Some(call_id) = call_id
                    && rewritten_call_ids.contains(call_id)
                {
                    rewritten_tool_outputs += 1;
                    let output = format!(
                        "status: {status}\nexecution: {execution}\ntools:\n{}",
                        serde_json::to_string(tools).unwrap_or_default()
                    );
                    sanitized.push(assistant_transcript_message(
                        replayed_tool_output_transcript(call_id, Some("tool_search"), &output),
                    ));
                } else {
                    sanitized.push(item.clone());
                }
                // After tool outputs, a new tool_call group in the turn
                // needs its own reasoning.  Reset so continuation tool
                // calls without reasoning are detected for self-healing.
                saw_reasoning_content = false;
            }
            _ => sanitized.push(item.clone()),
        }
    }

    if rewritten_tool_calls > 0 || rewritten_tool_outputs > 0 {
        let msg = format!(
            "[reasoning_content] sanitize_model_turn_segment: 将 {rewritten_tool_calls} 个无 reasoning 的 tool_call 与 {rewritten_tool_outputs} 个关联 output 改写为 assistant transcript"
        );
        diagnostics.push(msg);
    }

    sanitized
}

fn is_non_assistant_message(item: &ResponseItem) -> bool {
    matches!(item, ResponseItem::Message { role, .. } if role != "assistant")
}

fn assistant_transcript_message(text: String) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText { text }],
        end_turn: Some(true),
        phase: None,
    }
}

fn replayed_tool_call_transcript(
    name: &str,
    call_id: Option<&str>,
    payload_label: &str,
    payload: &str,
) -> String {
    let call_id = call_id.unwrap_or_default();
    format!(
        "<replayed_tool_call>\nname: {name}\ncall_id: {call_id}\n{payload_label}:\n{payload}\n</replayed_tool_call>"
    )
}

fn replayed_tool_output_transcript(call_id: &str, name: Option<&str>, output: &str) -> String {
    let tool_name = name
        .map(|name| format!("name: {name}\n"))
        .unwrap_or_default();
    format!(
        "<replayed_tool_output>\n{tool_name}call_id: {call_id}\noutput:\n{output}\n</replayed_tool_output>"
    )
}

fn content_items_to_text(items: &[ContentItem]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } => Some(text.as_str()),
            ContentItem::OutputText { text } => Some(text.as_str()),
            ContentItem::InputImage { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn function_output_to_string(output: &FunctionCallOutputPayload) -> String {
    // FunctionCallOutputPayload serializes to either a string or structured content.
    serde_json::to_string(output).unwrap_or_default()
}

/// Converts Responses API tool definitions to Chat Completions format.
///
/// Responses API tools look like: `{"type": "function", "name": "...", "parameters": {...}, "description": "..."}`
/// Chat Completions tools look like: `{"type": "function", "function": {"name": "...", "parameters": {...}, "description": "..."}}`
fn convert_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| {
            let tool_type = tool.get("type")?.as_str()?;
            if tool_type == "function" {
                let name = tool.get("name")?;
                let mut func = json!({ "name": name });
                if let Some(desc) = tool.get("description") {
                    func["description"] = desc.clone();
                }
                if let Some(params) = tool.get("parameters") {
                    func["parameters"] = params.clone();
                }
                if let Some(strict) = tool.get("strict") {
                    func["strict"] = strict.clone();
                }
                Some(json!({
                    "type": "function",
                    "function": func,
                }))
            } else {
                // Skip non-function tools (web_search, etc.) — not supported by Chat Completions.
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ReasoningItemContent;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn assistant_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            end_turn: Some(true),
            phase: None,
        }
    }

    fn user_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            end_turn: None,
            phase: None,
        }
    }

    fn reasoning_item(text: &str) -> ResponseItem {
        ResponseItem::Reasoning {
            id: String::new(),
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: text.to_string(),
            }]),
            encrypted_content: None,
        }
    }

    #[test]
    fn does_not_attach_reasoning_content_to_plain_assistant_history() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                assistant_message("done"),
                reasoning_item("chain-of-thought"),
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([{
                "role": "assistant",
                "content": "done",
            }])
        );
    }

    #[test]
    fn attaches_reasoning_content_to_tool_call_history() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                reasoning_item("need-tool"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"pwd\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([{
                "role": "assistant",
                "content": null,
                "reasoning_content": "need-tool",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "shell",
                        "arguments": "{\"command\":\"pwd\"}",
                    }
                }]
            }])
        );
    }

    #[test]
    fn merges_assistant_content_and_tool_call_into_single_message() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                assistant_message("Let me check."),
                reasoning_item("need-tool"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"pwd\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([{
                "role": "assistant",
                "content": "Let me check.",
                "reasoning_content": "need-tool",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "shell",
                        "arguments": "{\"command\":\"pwd\"}",
                    }
                }]
            }])
        );
    }

    #[test]
    fn preserves_completed_tool_turn_without_reasoning_content_as_transcript() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                user_message("question"),
                assistant_message("Let me check."),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"pwd\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
                assistant_message("Done."),
                user_message("next question"),
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([
                {
                    "role": "user",
                    "content": "question",
                },
                {
                    "role": "assistant",
                    "content": concat!(
                        "Let me check.\n",
                        "<replayed_tool_call>\n",
                        "name: shell\n",
                        "call_id: call_1\n",
                        "arguments:\n",
                        "{\"command\":\"pwd\"}\n",
                        "</replayed_tool_call>\n",
                        "<replayed_tool_output>\n",
                        "call_id: call_1\n",
                        "output:\n",
                        "ok\n",
                        "</replayed_tool_output>\n",
                        "Done."
                    ),
                },
                {
                    "role": "user",
                    "content": "next question",
                }
            ])
        );
    }

    #[test]
    fn preserves_tool_turn_when_reasoning_content_exists() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                user_message("question"),
                assistant_message("Let me check."),
                reasoning_item("need-tool"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"pwd\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
                assistant_message("Done."),
                user_message("next question"),
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([
                {
                    "role": "user",
                    "content": "question",
                },
                {
                    "role": "assistant",
                    "content": "Let me check.",
                    "reasoning_content": "need-tool",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}",
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "\"ok\"",
                },
                {
                    "role": "assistant",
                    "content": "Done.",
                },
                {
                    "role": "user",
                    "content": "next question",
                }
            ])
        );
    }

    #[test]
    fn merges_consecutive_assistant_messages_before_tool_call_in_thinking_mode() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                user_message("question"),
                assistant_message("First step."),
                assistant_message("Second step."),
                reasoning_item("need-tool"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"pwd\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([
                {
                    "role": "user",
                    "content": "question",
                },
                {
                    "role": "assistant",
                    "content": "First step.\nSecond step.",
                    "reasoning_content": "need-tool",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}",
                        }
                    }]
                }
            ])
        );
    }

    #[test]
    fn preserves_reasoning_content_for_final_assistant_after_tool_turn() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                user_message("question"),
                assistant_message("Let me check."),
                reasoning_item("need-tool"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"pwd\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
                assistant_message("Done."),
                reasoning_item("final-thought"),
                user_message("next question"),
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([
                {
                    "role": "user",
                    "content": "question",
                },
                {
                    "role": "assistant",
                    "content": "Let me check.",
                    "reasoning_content": "need-tool",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}",
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "\"ok\"",
                },
                {
                    "role": "assistant",
                    "content": "Done.",
                    "reasoning_content": "final-thought",
                },
                {
                    "role": "user",
                    "content": "next question",
                }
            ])
        );
    }

    #[test]
    fn preserves_tool_calls_without_reasoning_as_transcript_without_trailing_assistant() {
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                user_message("question"),
                assistant_message("Let me check."),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"ls\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("file.txt".to_string()),
                },
                // No trailing assistant message.
                user_message("next question"),
            ],
            &[],
            false,
            true,
            Some("high"),
            &mut Vec::new(),
        );

        assert_eq!(
            body["messages"],
            json!([
                {
                    "role": "user",
                    "content": "question",
                },
                {
                    "role": "assistant",
                    "content": concat!(
                        "Let me check.\n",
                        "<replayed_tool_call>\n",
                        "name: shell\n",
                        "call_id: call_1\n",
                        "arguments:\n",
                        "{\"command\":\"ls\"}\n",
                        "</replayed_tool_call>\n",
                        "<replayed_tool_output>\n",
                        "call_id: call_1\n",
                        "output:\n",
                        "file.txt\n",
                        "</replayed_tool_output>"
                    ),
                },
                {
                    "role": "user",
                    "content": "next question",
                }
            ])
        );
    }
}
