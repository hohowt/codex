//! Converts a `ResponsesApiRequest` into a Chat Completions API JSON body.

use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
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
    stream: bool,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    // System message from instructions.
    if !instructions.is_empty() {
        messages.push(json!({
            "role": "system",
            "content": instructions,
        }));
    }

    // Convert each ResponseItem into one or more Chat Completions messages.
    for item in input {
        match item {
            ResponseItem::Message {
                role, content, ..
            } => {
                let text = content_items_to_text(content);
                // Map OpenAI-specific roles to standard Chat Completions roles.
                // "developer" is an OpenAI-only role; most providers only accept
                // "system", "user", "assistant".
                let mapped_role = match role.as_str() {
                    "developer" => "system",
                    other => other,
                };
                if mapped_role == "assistant" {
                    messages.push(json!({
                        "role": "assistant",
                        "content": text,
                    }));
                } else {
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
                if let Some(last) = messages.last_mut()
                    && last.get("role").and_then(Value::as_str) == Some("assistant")
                    && last.get("tool_calls").is_some()
                {
                    last["tool_calls"]
                        .as_array_mut()
                        .unwrap()
                        .push(tool_call);
                } else {
                    messages.push(json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [tool_call],
                    }));
                }
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
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
                if let Some(last) = messages.last_mut()
                    && last.get("role").and_then(Value::as_str) == Some("assistant")
                    && last.get("tool_calls").is_some()
                {
                    last["tool_calls"]
                        .as_array_mut()
                        .unwrap()
                        .push(tool_call);
                } else {
                    messages.push(json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [tool_call],
                    }));
                }
            }
            ResponseItem::Reasoning { summary, .. } => {
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
                }
            }
            // Skip items that have no Chat Completions equivalent.
            _ => {}
        }
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": stream,
    });

    // Convert Responses API tools to Chat Completions tools format.
    let chat_tools = convert_tools(tools);
    if !chat_tools.is_empty() {
        body["tools"] = Value::Array(chat_tools);
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(parallel_tool_calls);
    }

    if stream {
        body["stream_options"] = json!({"include_usage": true});
    }

    body
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
