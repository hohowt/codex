//! Streams a Chat Completions request and converts SSE chunks into `ResponseEvent`s.

use crate::convert::build_chat_request;
use codex_api::ApiError;
use codex_api::AuthProvider;
use codex_api::Provider;
use codex_api::ReqwestTransport;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::trace;
use tracing::warn;

/// Streams a Chat Completions request, translating from Responses API types.
///
/// Returns a `ResponseStream` that yields `ResponseEvent`s, matching the same
/// interface that the Responses API path uses.
pub async fn stream_chat_completions<A: AuthProvider>(
    _transport: ReqwestTransport,
    provider: Provider,
    auth: &A,
    model: &str,
    instructions: &str,
    input: &[ResponseItem],
    tools: &[Value],
    parallel_tool_calls: bool,
    idle_timeout: Duration,
    reasoning_effort: Option<String>,
) -> Result<ResponseStream, ApiError> {
    // MiniMax does not support stream_options; DeepSeek and other OpenAI-compatible providers do.
    let supports_stream_options = !is_minimax_provider(&provider.name, &provider.base_url);
    // Only DeepSeek V4+ supports the thinking/reasoning_effort parameters.
    let supports_thinking = is_deepseek_provider(&provider.name, &provider.base_url);
    let reasoning_effort = if supports_thinking {
        reasoning_effort
    } else {
        None
    };
    let body = build_chat_request(
        model,
        instructions,
        input,
        tools,
        parallel_tool_calls,
        supports_stream_options,
        reasoning_effort.as_deref(),
    );

    let url = provider.url_for_path("chat/completions");
    let mut headers = provider.headers.clone();
    headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        http::header::ACCEPT,
        http::HeaderValue::from_static("text/event-stream"),
    );

    // Add auth headers.
    if let Some(token) = auth.bearer_token() {
        if let Ok(val) = http::HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(http::header::AUTHORIZATION, val);
        }
    }

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .headers(reqwest_headers_from_http(headers))
        .json(&body)
        .send()
        .await
        .map_err(|e| ApiError::Stream(format!("chat completions request failed: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(ApiError::Stream(format!(
            "chat completions returned {status}: {body_text}"
        )));
    }

    let byte_stream = response
        .bytes_stream()
        .map(|r| r.map_err(|e| codex_client::TransportError::Network(e.to_string())));

    let (tx, rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);

    tokio::spawn(process_chat_sse(Box::pin(byte_stream), tx, idle_timeout));

    Ok(ResponseStream { rx_event: rx })
}

fn is_deepseek_provider(name: &str, base_url: &str) -> bool {
    name.eq_ignore_ascii_case("DeepSeek") || base_url.to_ascii_lowercase().contains("deepseek.com")
}

fn is_minimax_provider(name: &str, base_url: &str) -> bool {
    name.eq_ignore_ascii_case("MiniMax") || base_url.to_ascii_lowercase().contains("minimax")
}

fn reqwest_headers_from_http(headers: http::HeaderMap) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    for (name, value) in headers.iter() {
        if let Ok(n) = reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()) {
            if let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
                out.insert(n, v);
            }
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    id: Option<String>,
    choices: Option<Vec<ChunkChoice>>,
    usage: Option<ChunkUsage>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    delta: Option<ChunkDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkDelta {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ChunkToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChunkToolCall {
    index: Option<usize>,
    id: Option<String>,
    function: Option<ChunkFunction>,
}

#[derive(Debug, Deserialize)]
struct ChunkFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkUsage {
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    total_tokens: Option<i64>,
    prompt_tokens_details: Option<ChunkPromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ChunkPromptTokensDetails {
    cached_tokens: Option<i64>,
}

/// Accumulated state for a single tool call being streamed.
#[derive(Debug, Default, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

async fn process_chat_sse(
    stream: codex_client::ByteStream,
    tx: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
) {
    let mut sse_stream = stream.eventsource();
    let mut sent_created = false;
    let mut accumulated_text = String::new();
    let mut accumulated_reasoning = String::new();
    let mut pending_tool_calls: Vec<PendingToolCall> = Vec::new();
    let mut last_chunk_id = String::new();
    let mut final_usage: Option<TokenUsage> = None;

    loop {
        let next = tokio::time::timeout(idle_timeout, sse_stream.next()).await;
        match next {
            Ok(Some(Ok(event))) => {
                let data = event.data.trim().to_string();
                if data == "[DONE]" {
                    break;
                }

                let chunk: ChatCompletionChunk = match serde_json::from_str(&data) {
                    Ok(c) => c,
                    Err(e) => {
                        trace!("failed to parse chat chunk: {e}: {data}");
                        continue;
                    }
                };

                if let Some(id) = &chunk.id {
                    last_chunk_id = id.clone();
                }

                // Emit Created on first chunk.
                if !sent_created {
                    let _ = tx.send(Ok(ResponseEvent::Created)).await;
                    sent_created = true;
                }

                // Process usage (usually in the last chunk).
                if let Some(usage) = chunk.usage {
                    final_usage = Some(TokenUsage {
                        input_tokens: usage.prompt_tokens.unwrap_or(0),
                        cached_input_tokens: usage
                            .prompt_tokens_details
                            .and_then(|d| d.cached_tokens)
                            .unwrap_or(0),
                        output_tokens: usage.completion_tokens.unwrap_or(0),
                        reasoning_output_tokens: 0,
                        total_tokens: usage.total_tokens.unwrap_or(0),
                    });
                }

                let Some(choices) = chunk.choices else {
                    continue;
                };

                for choice in &choices {
                    let Some(delta) = &choice.delta else {
                        continue;
                    };

                    // Text content delta.
                    if let Some(content) = &delta.content {
                        if !content.is_empty() {
                            accumulated_text.push_str(content);
                            let _ = tx
                                .send(Ok(ResponseEvent::OutputTextDelta(content.clone())))
                                .await;
                        }
                    }

                    // Reasoning content (DeepSeek style).
                    if let Some(reasoning) = &delta.reasoning_content {
                        if !reasoning.is_empty() {
                            accumulated_reasoning.push_str(reasoning);
                            let _ = tx
                                .send(Ok(ResponseEvent::ReasoningContentDelta {
                                    delta: reasoning.clone(),
                                    content_index: 0,
                                }))
                                .await;
                        }
                    }

                    // Tool call deltas.
                    if let Some(tool_calls) = &delta.tool_calls {
                        for tc in tool_calls {
                            let idx = tc.index.unwrap_or(0);
                            // Grow the pending list if needed.
                            while pending_tool_calls.len() <= idx {
                                pending_tool_calls.push(PendingToolCall::default());
                            }
                            if let Some(id) = &tc.id {
                                pending_tool_calls[idx].id = id.clone();
                            }
                            if let Some(func) = &tc.function {
                                if let Some(name) = &func.name {
                                    pending_tool_calls[idx].name = name.clone();
                                }
                                if let Some(args) = &func.arguments {
                                    pending_tool_calls[idx].arguments.push_str(args);
                                }
                            }
                        }
                    }

                    // On finish, emit completed items.
                    if choice.finish_reason.is_some() {
                        // Emit accumulated text as a message.
                        if !accumulated_text.is_empty() {
                            let item = ResponseItem::Message {
                                id: None,
                                role: "assistant".to_string(),
                                content: vec![ContentItem::OutputText {
                                    text: accumulated_text.clone(),
                                }],
                                end_turn: Some(true),
                                phase: None,
                            };
                            let _ = tx.send(Ok(ResponseEvent::OutputItemDone(item))).await;
                        }

                        // Emit accumulated reasoning content so it can be
                        // passed back to the API on subsequent tool-call turns.
                        if !accumulated_reasoning.is_empty() {
                            let item = ResponseItem::Reasoning {
                                id: String::new(),
                                summary: Vec::new(),
                                content: Some(vec![ReasoningItemContent::ReasoningText {
                                    text: accumulated_reasoning.clone(),
                                }]),
                                encrypted_content: None,
                            };
                            let _ = tx.send(Ok(ResponseEvent::OutputItemDone(item))).await;
                        }

                        // Emit tool calls.
                        if !pending_tool_calls.is_empty() && accumulated_reasoning.is_empty() {
                            let names: Vec<&str> =
                                pending_tool_calls.iter().map(|tc| tc.name.as_str()).collect();
                            warn!(
                                "[reasoning_content] process_chat_sse: 模型返回了 {} 个 tool_call ({names:?})，但 reasoning_content 为空",
                                pending_tool_calls.len()
                            );
                        }
                        for tc in &pending_tool_calls {
                            let item = ResponseItem::FunctionCall {
                                id: None,
                                name: tc.name.clone(),
                                namespace: None,
                                arguments: tc.arguments.clone(),
                                call_id: tc.id.clone(),
                            };
                            let _ = tx.send(Ok(ResponseEvent::OutputItemDone(item))).await;
                        }
                    }
                }
            }
            Ok(Some(Err(e))) => {
                debug!("Chat SSE error: {e:#}");
                let _ = tx.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                break;
            }
            Err(_) => {
                let _ = tx
                    .send(Err(ApiError::Stream(
                        "idle timeout waiting for chat SSE".into(),
                    )))
                    .await;
                return;
            }
        }
    }

    // Emit Completed.
    let _ = tx
        .send(Ok(ResponseEvent::Completed {
            response_id: last_chunk_id,
            token_usage: final_usage,
        }))
        .await;
}
