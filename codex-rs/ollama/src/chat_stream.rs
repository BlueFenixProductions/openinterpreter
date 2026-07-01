//! NDJSON streaming for Ollama's native `/api/chat` endpoint.
//!
//! Mirrors `client.rs`'s `pull_model_stream`: POST the request, then split the streamed
//! response body into lines with `line_buffer`'s `LineBuffer` (the same NDJSON framing used by
//! `/api/pull`) and parse each complete line as JSON.

use futures::Stream;
use futures::StreamExt;
use std::io;

use crate::chat_types::ChatRequest;
use crate::chat_types::ChatResponseChunk;
use crate::line_buffer::LineBuffer;

/// POST `request` to `{host_root}/api/chat` and stream back parsed NDJSON response chunks in
/// order. The stream ends when the server closes the connection; a non-2xx response or a
/// malformed line surfaces as an `Err` (the latter also ending the stream, since a corrupt line
/// means the framing can no longer be trusted).
pub async fn chat_stream(
    host_root: &str,
    request: &ChatRequest,
) -> io::Result<impl Stream<Item = io::Result<ChatResponseChunk>> + use<>> {
    let url = format!("{}/api/chat", host_root.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .post(url)
        .json(request)
        .send()
        .await
        .map_err(io::Error::other)?;
    if !resp.status().is_success() {
        return Err(io::Error::other(format!(
            "failed to start chat: HTTP {}",
            resp.status()
        )));
    }

    let mut stream = resp.bytes_stream();
    let mut buf = LineBuffer::default();

    let s = async_stream::stream! {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    while let Some(line) = buf.take_line() {
                        if let Ok(text) = std::str::from_utf8(&line) {
                            let text = text.trim();
                            if text.is_empty() { continue; }
                            match serde_json::from_str::<ChatResponseChunk>(text) {
                                Ok(chat_chunk) => yield Ok(chat_chunk),
                                Err(err) => {
                                    yield Err(io::Error::other(err));
                                    return;
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    yield Err(io::Error::other(err));
                    return;
                }
            }
        }
    };

    // Pin+box, matching `pull_model_stream`'s approach: `async_stream::stream!` produces a
    // `!Unpin` type, and callers (including this module's test) need to call `Stream::next()`
    // directly without pinning it themselves first.
    Ok(Box::pin(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_types::ChatMessage;
    use crate::think::ThinkValue;
    use futures::StreamExt;

    #[tokio::test]
    async fn chat_stream_yields_chunks_in_order_and_completes() {
        let server = wiremock::MockServer::start().await;
        let ndjson_body = concat!(
            r#"{"model":"m","message":{"role":"assistant","content":"Hel"},"done":false}"#,
            "\n",
            r#"{"model":"m","message":{"role":"assistant","content":"lo"},"done":false}"#,
            "\n",
            r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":2,"prompt_eval_count":5}"#,
            "\n",
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/chat"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(ndjson_body, "application/x-ndjson"),
            )
            .mount(&server)
            .await;

        let request = ChatRequest {
            model: "m".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some("hi".to_string()),
                thinking: None,
                tool_calls: None,
            }],
            think: ThinkValue::Bool(false),
            stream: true,
            tools: None,
        };

        let mut stream = chat_stream(&server.uri(), &request).await.unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.message.content.as_deref(), Some("Hel"));
        let second = stream.next().await.unwrap().unwrap();
        assert_eq!(second.message.content.as_deref(), Some("lo"));
        let third = stream.next().await.unwrap().unwrap();
        assert!(third.done);
        assert_eq!(third.eval_count, Some(2));
        assert!(stream.next().await.is_none());
    }
}
