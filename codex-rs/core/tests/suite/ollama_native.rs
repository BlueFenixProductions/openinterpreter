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
