//! Request/response types for Ollama's native `/api/chat` endpoint (NOT the OpenAI-compat
//! `/v1/chat/completions` surface). Shapes verified against a live Ollama instance, 2026-07-01 —
//! see docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md §5 for the captured
//! fixtures these types are built from.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::think::ThinkValue;

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub think: ThinkValue,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Reasoning trace, separate from `content` — confirmed real via a live capture (a
    /// think:true request against gemma4:26b-a4b-it-qat): the model's `<think>`-equivalent
    /// output arrives here, not mixed into `content`. Absent on non-reasoning responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    pub function: ChatToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCallFunction {
    pub index: u32,
    pub name: String,
    pub arguments: Value,
}

/// One line of Ollama's native streamed (or non-streamed, when `stream:false`) chat response.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponseChunk {
    pub model: String,
    pub message: ChatMessage,
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    #[serde(default)]
    pub eval_count: Option<u64>,
    #[serde(default)]
    pub eval_duration: Option<u64>,
    #[serde(default)]
    pub prompt_eval_count: Option<u64>,
    #[serde(default)]
    pub prompt_eval_duration: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::think::ThinkValue;

    // Captured live from Rikudo's Ollama (gemma4:26b-a4b-it-qat), stream:false, 2026-07-01 —
    // see docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md §5.
    const NON_STREAMING_CHAT_RESPONSE: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:22:52.304048354Z",
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_og2056ag",
                    "function": {
                        "index": 0,
                        "name": "get_weather",
                        "arguments": {"city": "Paris"}
                    }
                }
            ]
        },
        "done": true,
        "done_reason": "stop",
        "total_duration": 1757438142,
        "load_duration": 328879050,
        "prompt_eval_count": 69,
        "prompt_eval_duration": 777192000,
        "eval_count": 15,
        "eval_duration": 560723000
    }"#;

    // Captured live, stream:true, mid-stream content chunk (done:false, no tool_calls).
    const STREAMING_CONTENT_CHUNK: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:23:11.856682501Z",
        "message": {"role": "assistant", "content": "Hello"},
        "done": false
    }"#;

    // Captured live, stream:true, final chunk (done:true, no message content, usage stats present).
    const STREAMING_FINAL_CHUNK: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:23:11.896534616Z",
        "message": {"role": "assistant", "content": ""},
        "done": true,
        "done_reason": "stop",
        "total_duration": 1012876548,
        "load_duration": 310477301,
        "prompt_eval_count": 69,
        "prompt_eval_duration": 142060000,
        "eval_count": 15,
        "eval_duration": 558562000
    }"#;

    // Captured live, think:true, stream:false, gemma4:26b-a4b-it-qat asked to reason through
    // "What is 15 * 23?" — confirms `message.thinking` is a real, separate field alongside
    // `message.content` (resolves the spec's flagged uncertainty about this field's existence).
    // Truncated here to the parts the test actually checks; the field is a much longer string live.
    const RESPONSE_WITH_THINKING: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:33:59.695247273Z",
        "message": {
            "role": "assistant",
            "content": "15 x 23 = 345",
            "thinking": "The user wants the product of 15 and 23. Method: distributive property."
        },
        "done": true,
        "done_reason": "stop",
        "prompt_eval_count": 30,
        "eval_count": 778
    }"#;

    #[test]
    fn parses_non_streaming_response_with_tool_call() {
        let chunk: ChatResponseChunk = serde_json::from_str(NON_STREAMING_CHAT_RESPONSE).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.as_deref(), Some("stop"));
        assert_eq!(chunk.eval_count, Some(15));
        assert_eq!(chunk.prompt_eval_count, Some(69));
        let tool_calls = chunk.message.tool_calls.expect("tool_calls must parse");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_og2056ag");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(
            tool_calls[0]
                .function
                .arguments
                .get("city")
                .and_then(|v| v.as_str()),
            Some("Paris")
        );
    }

    #[test]
    fn parses_streaming_content_chunk() {
        let chunk: ChatResponseChunk = serde_json::from_str(STREAMING_CONTENT_CHUNK).unwrap();
        assert!(!chunk.done);
        assert_eq!(chunk.message.content.as_deref(), Some("Hello"));
        assert!(chunk.message.tool_calls.is_none());
    }

    #[test]
    fn parses_response_with_thinking_field_separate_from_content() {
        let chunk: ChatResponseChunk = serde_json::from_str(RESPONSE_WITH_THINKING).unwrap();
        assert_eq!(chunk.message.content.as_deref(), Some("15 x 23 = 345"));
        assert!(
            chunk
                .message
                .thinking
                .as_deref()
                .unwrap()
                .contains("distributive property")
        );
    }

    #[test]
    fn parses_streaming_final_chunk_with_usage() {
        let chunk: ChatResponseChunk = serde_json::from_str(STREAMING_FINAL_CHUNK).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.eval_count, Some(15));
        assert_eq!(chunk.prompt_eval_count, Some(69));
    }

    #[test]
    fn chat_request_serializes_with_think_bool() {
        let req = ChatRequest {
            model: "gemma4:26b-a4b-it-qat".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                thinking: None,
                tool_calls: None,
            }],
            think: ThinkValue::Bool(false),
            stream: true,
            tools: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["think"], serde_json::json!(false));
        assert_eq!(json["stream"], serde_json::json!(true));
    }

    #[test]
    fn chat_request_serializes_with_think_effort_string() {
        let req = ChatRequest {
            model: "gpt-oss:120b".to_string(),
            messages: vec![],
            think: ThinkValue::Effort("low".to_string()),
            stream: true,
            tools: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["think"], serde_json::json!("low"));
    }
}
