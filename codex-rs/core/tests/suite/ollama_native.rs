//! End-to-end coverage for `StreamTransportRoute::OllamaNativeChat`: a turn against a mocked
//! Ollama native `/api/chat` endpoint (NDJSON, not SSE) should produce the same
//! `EventMsg::AgentMessage` shape as every other wire.

use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ollama_native_chat_turn_completes_with_mock_response() {
    skip_if_no_network!();

    let server = MockServer::start().await;

    let ndjson_body = concat!(
        r#"{"model":"m","message":{"role":"assistant","content":"Hel"},"done":false}"#,
        "\n",
        r#"{"model":"m","message":{"role":"assistant","content":"lo"},"done":false}"#,
        "\n",
        r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":2,"prompt_eval_count":5}"#,
        "\n",
    );

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ndjson_body, "application/x-ndjson"))
        .expect(1)
        .mount(&server)
        .await;

    let provider = ModelProviderInfo {
        name: "ollama-native".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::OllamaNative,
        ollama_think: None,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(2_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .unwrap();

    let final_message = wait_for_event(
        &codex,
        |event| matches!(event, EventMsg::AgentMessage(message) if message.message == "Hello"),
    )
    .await;
    assert!(matches!(final_message, EventMsg::AgentMessage(_)));

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
}

/// End-to-end coverage for tool-calling over `StreamTransportRoute::OllamaNativeChat`: when
/// Ollama's native NDJSON response includes a `message.tool_calls` array, `client.rs`'s
/// `OllamaStreamState` bracketing logic must emit `OutputItemDone(ResponseItem::FunctionCall)`
/// so the turn processor (`core/src/session/turn.rs`) dispatches it like any other wire. Mirrors
/// `abort_tasks::interrupt_long_running_tool_emits_turn_aborted`, which proves the same thing for
/// the Responses SSE wire via a `shell_command` function call: wait for `ExecCommandBegin` to
/// confirm the tool call was parsed and surfaced, then interrupt rather than depend on a second
/// mocked round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ollama_native_chat_turn_handles_tool_call() {
    skip_if_no_network!();

    let server = MockServer::start().await;

    // Real captured shape (per the native-Ollama-backend design spec, §5): a tool-call chunk
    // carrying `message.tool_calls` with `done:false`, followed by a `done:true` closing chunk.
    let ndjson_body = concat!(
        r#"{"model":"m","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","function":{"index":0,"name":"shell_command","arguments":{"command":"sleep 60","timeout_ms":60000}}}]},"done":false}"#,
        "\n",
        r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":2,"prompt_eval_count":5}"#,
        "\n",
    );

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ndjson_body, "application/x-ndjson"))
        .expect(1)
        .mount(&server)
        .await;

    let provider = ModelProviderInfo {
        name: "ollama-native".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::OllamaNative,
        ollama_think: None,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(2_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "start sleep".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .unwrap();

    // Proves the tool call was correctly parsed out of the NDJSON `tool_calls` array and
    // surfaced to the turn processor as a real function call (not silently dropped), the same
    // way `interrupt_long_running_tool_emits_turn_aborted` proves it for the Responses wire.
    wait_for_event(&codex, |ev| matches!(ev, EventMsg::ExecCommandBegin(_))).await;

    codex.submit(Op::Interrupt).await.unwrap();

    wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnAborted(_))).await;
}
