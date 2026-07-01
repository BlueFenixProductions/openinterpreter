//! Translates one native-Ollama chat chunk (chat_types::ChatResponseChunk) into zero or more
//! codex_api::ResponseEvent — the internal event type the rest of the agent loop
//! (TUI rendering, tool-call handling, token accounting) already consumes for every other wire.

use codex_api::ResponseEvent;
use codex_protocol::protocol::TokenUsage;

use crate::chat_types::ChatResponseChunk;

pub fn chat_chunk_to_events(chunk: ChatResponseChunk) -> Vec<ResponseEvent> {
    let mut events = Vec::new();

    // Reasoning trace, confirmed a genuinely separate field from `content` (live capture against
    // gemma4:26b-a4b-it-qat, think:true — see chat_types.rs's RESPONSE_WITH_THINKING fixture).
    // content_index: 0 — Ollama's native API has no multi-block reasoning index of its own; a
    // single running index is correct until a real multi-segment case is observed.
    if let Some(thinking) = chunk.message.thinking.as_deref() {
        if !thinking.is_empty() {
            events.push(ResponseEvent::ReasoningContentDelta {
                delta: thinking.to_string(),
                content_index: 0,
            });
        }
    }

    if let Some(content) = chunk.message.content.as_deref() {
        if !content.is_empty() {
            events.push(ResponseEvent::OutputTextDelta(content.to_string()));
        }
    }

    if let Some(tool_calls) = &chunk.message.tool_calls {
        for call in tool_calls {
            // Ollama sends the entire tool call in one chunk (confirmed empirically, both
            // stream:false and stream:true — see spec §5), never incremental argument deltas
            // the way OpenAI's streaming tool calls work. Emit it as a single complete delta.
            let delta = serde_json::to_string(&call.function.arguments)
                .unwrap_or_else(|_| "{}".to_string());
            events.push(ResponseEvent::ToolCallInputDelta {
                item_id: call.id.clone(),
                call_id: Some(call.id.clone()),
                delta,
            });
        }
    }

    if chunk.done {
        let token_usage = match (chunk.prompt_eval_count, chunk.eval_count) {
            (Some(input), Some(output)) => Some(TokenUsage {
                input_tokens: input as i64,
                cached_input_tokens: 0,
                output_tokens: output as i64,
                reasoning_output_tokens: 0,
                total_tokens: (input + output) as i64,
            }),
            _ => None,
        };
        events.push(ResponseEvent::Completed {
            response_id: String::new(),
            token_usage,
            end_turn: chunk.done_reason.as_deref().map(|r| r == "stop"),
        });
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_types::ChatMessage;
    use crate::chat_types::ChatResponseChunk;
    use crate::chat_types::ChatToolCall;
    use crate::chat_types::ChatToolCallFunction;

    #[test]
    fn content_chunk_becomes_output_text_delta() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("Hi".to_string()),
                thinking: None,
                tool_calls: None,
            },
            done: false,
            done_reason: None,
            eval_count: None,
            eval_duration: None,
            prompt_eval_count: None,
            prompt_eval_duration: None,
        };
        let events = chat_chunk_to_events(chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ResponseEvent::OutputTextDelta(s) if s == "Hi"));
    }

    #[test]
    fn thinking_field_becomes_reasoning_content_delta_separate_from_text() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("345".to_string()),
                thinking: Some("15 * 23 = 345 via distributive property".to_string()),
                tool_calls: None,
            },
            done: false,
            done_reason: None,
            eval_count: None,
            eval_duration: None,
            prompt_eval_count: None,
            prompt_eval_duration: None,
        };
        let events = chat_chunk_to_events(chunk);
        assert_eq!(
            events.len(),
            2,
            "expected both a reasoning delta and a text delta: {events:?}"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ResponseEvent::ReasoningContentDelta { delta, .. } if delta.contains("distributive")
        )));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ResponseEvent::OutputTextDelta(s) if s == "345"))
        );
    }

    #[test]
    fn final_chunk_becomes_completed_with_token_usage() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("".to_string()),
                thinking: None,
                tool_calls: None,
            },
            done: true,
            done_reason: Some("stop".to_string()),
            eval_count: Some(15),
            eval_duration: Some(560723000),
            prompt_eval_count: Some(69),
            prompt_eval_duration: Some(777192000),
        };
        let events = chat_chunk_to_events(chunk);
        let completed = events
            .iter()
            .find(|e| matches!(e, ResponseEvent::Completed { .. }));
        assert!(
            completed.is_some(),
            "expected a Completed event in {events:?}"
        );
        if let Some(ResponseEvent::Completed {
            end_turn,
            token_usage,
            ..
        }) = completed
        {
            assert_eq!(*end_turn, Some(true));
            let usage = token_usage.as_ref().expect("token_usage must be populated");
            assert_eq!(usage.output_tokens, 15);
            assert_eq!(usage.input_tokens, 69);
        }
    }

    #[test]
    fn tool_call_chunk_becomes_tool_call_input_delta() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("".to_string()),
                thinking: None,
                tool_calls: Some(vec![ChatToolCall {
                    id: "call_1".to_string(),
                    function: ChatToolCallFunction {
                        index: 0,
                        name: "get_weather".to_string(),
                        arguments: serde_json::json!({"city": "Paris"}),
                    },
                }]),
            },
            done: false,
            done_reason: None,
            eval_count: None,
            eval_duration: None,
            prompt_eval_count: None,
            prompt_eval_duration: None,
        };
        let events = chat_chunk_to_events(chunk);
        let tool_event = events
            .iter()
            .find(|e| matches!(e, ResponseEvent::ToolCallInputDelta { .. }));
        assert!(
            tool_event.is_some(),
            "expected a ToolCallInputDelta in {events:?}"
        );
        if let Some(ResponseEvent::ToolCallInputDelta {
            item_id,
            call_id,
            delta,
        }) = tool_event
        {
            assert_eq!(item_id, "call_1");
            assert_eq!(call_id.as_deref(), Some("call_1"));
            assert!(delta.contains("Paris"));
        }
    }
}
