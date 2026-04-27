//! Converts a `ResponsesApiRequest` into a Chat Completions API JSON body.

use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use serde_json::Value;
use serde_json::json;

/// Builds a Chat Completions API request body from a Responses API request.
pub fn build_chat_request(
    model: &str,
    instructions: &str,
    input: &[ResponseItem],
    tools: &[Value],
    parallel_tool_calls: bool,
    supports_stream_options: bool,
    reasoning_effort: Option<&str>,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let sanitized_input_storage;
    let input = if reasoning_effort.is_some() {
        sanitized_input_storage = sanitize_input_for_thinking_tool_replay(input);
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
    // attached to a subsequent assistant message with tool_calls.
    let mut pending_reasoning_content: Option<String> = None;
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
                    pending_reasoning_content = None;
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
                    // Attach reasoning_content if not already present.
                    if last.get("reasoning_content").is_none() {
                        if let Some(rc) = pending_reasoning_content.take() {
                            last["reasoning_content"] = json!(rc);
                        }
                    }
                } else if let Some(index) = last_assistant_message_index
                    && let Some(last) = messages.get_mut(index)
                    && last.get("tool_calls").is_none()
                {
                    last["tool_calls"] = json!([tool_call]);
                    if let Some(rc) = pending_reasoning_content.take() {
                        last["reasoning_content"] = json!(rc);
                    }
                } else {
                    let mut msg = json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [tool_call],
                    });
                    if let Some(rc) = pending_reasoning_content.take() {
                        msg["reasoning_content"] = json!(rc);
                    }
                    messages.push(msg);
                }
                last_assistant_message_index = None;
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                // Tool output breaks the reasoning→tool_call chain.
                pending_reasoning_content = None;
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
                    // Attach reasoning_content if not already present.
                    if last.get("reasoning_content").is_none() {
                        if let Some(rc) = pending_reasoning_content.take() {
                            last["reasoning_content"] = json!(rc);
                        }
                    }
                } else if let Some(index) = last_assistant_message_index
                    && let Some(last) = messages.get_mut(index)
                    && last.get("tool_calls").is_none()
                {
                    last["tool_calls"] = json!([tool_call]);
                    if let Some(rc) = pending_reasoning_content.take() {
                        last["reasoning_content"] = json!(rc);
                    }
                } else {
                    let mut msg = json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [tool_call],
                    });
                    if let Some(rc) = pending_reasoning_content.take() {
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
                        } else {
                            pending_reasoning_content = Some(raw_text);
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

fn sanitize_input_for_thinking_tool_replay(input: &[ResponseItem]) -> Vec<ResponseItem> {
    let mut sanitized = Vec::new();
    let mut model_turn_segment = Vec::new();

    for item in input {
        if is_non_assistant_message(item) {
            sanitized.extend(sanitize_model_turn_segment(&model_turn_segment));
            model_turn_segment.clear();
            sanitized.push(item.clone());
        } else {
            model_turn_segment.push(item.clone());
        }
    }

    sanitized.extend(sanitize_model_turn_segment(&model_turn_segment));
    sanitized
}

fn sanitize_model_turn_segment(segment: &[ResponseItem]) -> Vec<ResponseItem> {
    let mut saw_reasoning_content = false;
    let mut saw_tool_call_without_reasoning = false;
    let mut last_tool_activity_index = None;

    for (index, item) in segment.iter().enumerate() {
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
            }
            ResponseItem::FunctionCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::LocalShellCall { .. } => {
                last_tool_activity_index = Some(index);
                if !saw_reasoning_content {
                    saw_tool_call_without_reasoning = true;
                }
            }
            ResponseItem::FunctionCallOutput { .. }
            | ResponseItem::CustomToolCallOutput { .. }
            | ResponseItem::ToolSearchOutput { .. } => {
                last_tool_activity_index = Some(index);
            }
            _ => {}
        }
    }

    if !saw_tool_call_without_reasoning {
        return segment.to_vec();
    }

    let Some(last_tool_activity_index) = last_tool_activity_index else {
        return segment.to_vec();
    };

    let trailing_assistant_messages: Vec<ResponseItem> = segment[last_tool_activity_index + 1..]
        .iter()
        .filter(|item| matches!(item, ResponseItem::Message { role, .. } if role == "assistant"))
        .cloned()
        .collect();

    if trailing_assistant_messages.is_empty() {
        // No trailing assistant messages to keep as context for this turn.
        // Strip tool calls and outputs that lack reasoning_content to avoid
        // DeepSeek 400: "reasoning_content must be passed back to the API".
        segment
            .iter()
            .filter(|item| {
                !matches!(
                    item,
                    ResponseItem::FunctionCall { .. }
                        | ResponseItem::CustomToolCall { .. }
                        | ResponseItem::LocalShellCall { .. }
                        | ResponseItem::FunctionCallOutput { .. }
                        | ResponseItem::CustomToolCallOutput { .. }
                        | ResponseItem::ToolSearchOutput { .. }
                )
            })
            .cloned()
            .collect()
    } else {
        trailing_assistant_messages
    }
}

fn is_non_assistant_message(item: &ResponseItem) -> bool {
    matches!(item, ResponseItem::Message { role, .. } if role != "assistant")
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
    fn collapses_completed_tool_turn_without_reasoning_content() {
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
    fn strips_tool_calls_without_reasoning_when_no_trailing_assistant() {
        // When old history has tool calls without reasoning_content
        // AND no trailing assistant message, tool calls must be stripped
        // to avoid DeepSeek 400: "reasoning_content must be passed back".
        let body = build_chat_request(
            "deepseek-v4-pro",
            "",
            &[
                user_message("question"),
                assistant_message("Let me check."),
                // No reasoning item here – simulates old history before the fix.
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
        );

        // Tool calls without reasoning_content are stripped;
        // only the assistant text and user messages remain.
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
                },
                {
                    "role": "user",
                    "content": "next question",
                }
            ])
        );
    }
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
