use super::*;
use crate::session::tests::build_world_state_from_turn_context;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use pretty_assertions::assert_eq;
use std::sync::Arc;

async fn process_compacted_history_with_test_session(
    compacted_history: Vec<ResponseItem>,
    previous_turn_settings: Option<&PreviousTurnSettings>,
) -> (Vec<ResponseItem>, Vec<ResponseItem>) {
    let (session, turn_context) = crate::session::tests::make_session_and_context().await;
    let turn_context = Arc::new(turn_context);
    session
        .set_previous_turn_settings(previous_turn_settings.cloned())
        .await;
    let world_state = Arc::new(build_world_state_from_turn_context(&session, &turn_context).await);
    let initial_context = session
        .build_initial_context_with_world_state(&turn_context, world_state.as_ref())
        .await;
    let initial_context_injection = InitialContextInjection::BeforeLastUserMessage(world_state);
    let (refreshed, _) = crate::compact_remote::process_compacted_history(
        &session,
        &turn_context,
        compacted_history,
        &initial_context_injection,
    )
    .await;
    (refreshed, initial_context)
}

fn user_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call(name: &str, call_id: &str, arguments: serde_json::Value) -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: name.to_string(),
        namespace: None,
        arguments: arguments.to_string(),
        call_id: call_id.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call_output(call_id: &str, output: &str) -> ResponseItem {
    ResponseItem::FunctionCallOutput {
        id: None,
        call_id: call_id.to_string(),
        output: FunctionCallOutputPayload::from_text(output.to_string()),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn compacted_user_message(text: &str) -> CompactedUserMessage {
    CompactedUserMessage {
        message: text.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn content_items_to_text_joins_non_empty_segments() {
    let items = vec![
        ContentItem::InputText {
            text: "hello".to_string(),
        },
        ContentItem::OutputText {
            text: String::new(),
        },
        ContentItem::OutputText {
            text: "world".to_string(),
        },
    ];

    let joined = content_items_to_text(&items);

    assert_eq!(Some("hello\nworld".to_string()), joined);
}

#[test]
fn content_items_to_text_ignores_image_only_content() {
    let items = vec![ContentItem::InputImage {
        image_url: "file://image.png".to_string(),
        detail: Some(DEFAULT_IMAGE_DETAIL),
    }];

    let joined = content_items_to_text(&items);

    assert_eq!(None, joined);
}

#[test]
fn collect_user_messages_extracts_user_text_only() {
    let items = vec![
        ResponseItem::Message {
            id: Some("assistant".to_string()),
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "ignored".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: Some("user".to_string()),
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "first".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Other,
    ];

    let collected = collect_user_messages(&items);

    assert_eq!(vec![compacted_user_message("first")], collected);
}

#[test]
fn zcode_retained_compacted_user_messages_keeps_read_tool_reminders_only() {
    let read_reminder = "<system-reminder>\nCalled the Read tool with the following input: {}\nResult of calling the Read tool:\n1\talpha\n</system-reminder>";
    let retained = zcode_retained_compacted_user_messages(&[
        user_message("ordinary prompt"),
        user_message(read_reminder),
        user_message("<subagent_notification>{}</subagent_notification>"),
    ]);

    assert_eq!(retained.len(), 1);
    let ResponseItem::Message { content, .. } = &retained[0] else {
        panic!("expected message");
    };
    assert_eq!(
        content,
        &vec![ContentItem::InputText {
            text: read_reminder.to_string(),
        }]
    );
}

#[test]
fn zcode_retained_compacted_user_messages_synthesizes_recent_whole_file_read_reminders() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let notes_path = temp_dir.path().join("research-notes.md");
    std::fs::write(
        &notes_path,
        "# Research Notes\nCanvas and usability notes.\n",
    )
    .expect("write notes");
    let notes_path = notes_path.to_string_lossy().to_string();
    let retained = zcode_retained_compacted_user_messages(&[
        function_call(
            "Read",
            "read-notes-1",
            serde_json::json!({"file_path":notes_path}),
        ),
        function_call_output(
            "read-notes-1",
            "Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead.",
        ),
        function_call(
            "Read",
            "read-game-1",
            serde_json::json!({"file_path":"/workspace/game.js"}),
        ),
        function_call_output("read-game-1", &"1\tconst x = 1;\n".repeat(7000)),
        function_call(
            "Read",
            "read-notes-2",
            serde_json::json!({"file_path":notes_path}),
        ),
        function_call_output(
            "read-notes-2",
            "Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead.",
        ),
        function_call(
            "Read",
            "read-ranged-1",
            serde_json::json!({"file_path":"/workspace/index.html","offset":1,"limit":5}),
        ),
        function_call_output("read-ranged-1", "1\t<html>"),
        function_call(
            "Read",
            "read-level-1",
            serde_json::json!({"file_path":"/workspace/generated-levels/level-042.json"}),
        ),
        function_call_output(
            "read-level-1",
            "<open-interpreter-harness-no-truncate>\n1\t{\"marker\":\"ZCODE_WEB_GAME_LEVEL\"}",
        ),
    ]);

    let texts = retained
        .iter()
        .map(|item| match item {
            ResponseItem::Message { content, .. } => content_items_to_text(content).unwrap(),
            other => panic!("expected message, found {other:?}"),
        })
        .collect::<Vec<_>>();

    assert_eq!(texts.len(), 2);
    assert!(texts[0].contains("/workspace/generated-levels/level-042.json"));
    assert!(texts[0].contains("ZCODE_WEB_GAME_LEVEL"));
    assert!(texts[0].contains("Result of calling the Read tool:\n1\t{"));
    assert!(!texts[0].contains("<open-interpreter-harness-no-truncate>"));
    assert!(texts[1].contains("research-notes.md"));
    assert!(texts[1].contains("# Research Notes"));
    assert!(!texts.iter().any(|text| text.contains("/workspace/game.js")));
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("/workspace/index.html"))
    );
    assert!(!texts.iter().any(|text| text.contains("Wasted call")));
}

#[test]
fn collect_user_messages_filters_session_prefix_entries() {
    let items = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: r#"# AGENTS.md instructions for project

<INSTRUCTIONS>
do things
</INSTRUCTIONS>"#
                    .to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "<ENVIRONMENT_CONTEXT>cwd=/tmp</ENVIRONMENT_CONTEXT>".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "real user message".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let collected = collect_user_messages(&items);

    assert_eq!(vec![compacted_user_message("real user message")], collected);
}

#[test]
fn collect_user_messages_filters_legacy_warnings() {
    let items = vec![
        user_message(
            "Warning: The maximum number of unified exec processes you can keep open is 60 and you currently have 61 processes open. Reuse older processes or close them to prevent automatic pruning of old processes",
        ),
        user_message(
            "Warning: apply_patch was requested via exec_command. Use the apply_patch tool instead of exec_command.",
        ),
        user_message(
            "Warning: Your account was flagged for potentially high-risk cyber activity and this request was routed to gpt-5.2 as a fallback. To regain access to gpt-5.3-codex, apply for trusted access: https://chatgpt.com/cyber or learn more: https://developers.openai.com/codex/concepts/cyber-safety",
        ),
        user_message("real user message"),
    ];

    let collected = collect_user_messages(&items);

    assert_eq!(vec![compacted_user_message("real user message")], collected);
}

#[test]
fn build_token_limited_compacted_history_truncates_overlong_user_messages() {
    // Use a small truncation limit so the test remains fast while still validating
    // that oversized user content is truncated.
    let max_tokens = 16;
    let big = "word ".repeat(200);
    let user_message = compacted_user_message(&big);
    let history = super::build_compacted_history_with_limit(
        Vec::new(),
        std::slice::from_ref(&user_message),
        "SUMMARY",
        max_tokens,
    );
    assert_eq!(history.len(), 2);

    let truncated_message = &history[0];
    let summary_message = &history[1];

    let truncated_text = match truncated_message {
        ResponseItem::Message { role, content, .. } if role == "user" => {
            content_items_to_text(content).unwrap_or_default()
        }
        other => panic!("unexpected item in history: {other:?}"),
    };

    assert!(
        truncated_text.contains("tokens truncated"),
        "expected truncation marker in truncated user message"
    );
    assert!(
        !truncated_text.contains(&big),
        "truncated user message should not include the full oversized user text"
    );

    let summary_text = match summary_message {
        ResponseItem::Message { role, content, .. } if role == "user" => {
            content_items_to_text(content).unwrap_or_default()
        }
        other => panic!("unexpected item in history: {other:?}"),
    };
    assert_eq!(summary_text, "SUMMARY");
}

#[test]
fn build_token_limited_compacted_history_appends_summary_message() {
    let initial_context: Vec<ResponseItem> = Vec::new();
    let user_messages = vec![compacted_user_message("first user message")];
    let summary_text = "summary text";

    let history = build_compacted_history(initial_context, &user_messages, summary_text);
    assert!(
        !history.is_empty(),
        "expected compacted history to include summary"
    );

    let last = history.last().expect("history should have a summary entry");
    let summary = match last {
        ResponseItem::Message { role, content, .. } if role == "user" => {
            content_items_to_text(content).unwrap_or_default()
        }
        other => panic!("expected summary message, found {other:?}"),
    };
    assert_eq!(summary, summary_text);
}

#[test]
fn build_compacted_history_preserves_user_message_passthrough_metadata() {
    let history = build_compacted_history(
        Vec::new(),
        &[CompactedUserMessage {
            message: "first user message".to_string(),
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    turn_id: Some("turn-1".to_string()),
                },
            ),
        }],
        "summary text",
    );

    assert_eq!(history[0].turn_id(), Some("turn-1"));
    assert_eq!(history[1].turn_id(), None);
}

#[test]
fn should_use_remote_compact_task_for_azure_provider() {
    let provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://example.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        ollama_think: None,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };

    assert!(should_use_remote_compact_task(&provider));
}

#[tokio::test]
async fn zcode_compact_failures_advance_window_without_retrying() {
    let (_session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    let mut config = turn_context.config.as_ref().clone();
    config.harness = Some("zcode".to_string());
    turn_context.config = std::sync::Arc::new(config);

    assert!(!should_retry_failed_compact(&turn_context));
    assert!(should_advance_window_after_failed_compact(&turn_context));
}

#[tokio::test]
async fn default_compact_failures_keep_retry_policy() {
    let (_session, turn_context) = crate::session::tests::make_session_and_context().await;

    assert!(should_retry_failed_compact(&turn_context));
    assert!(!should_advance_window_after_failed_compact(&turn_context));
}

#[tokio::test]
async fn zcode_manual_compact_skips_user_only_history() {
    let (session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    let mut config = turn_context.config.as_ref().clone();
    config.harness = Some("zcode".to_string());
    turn_context.config = std::sync::Arc::new(config);
    session
        .record_conversation_items(&turn_context, &[user_message("prompt")])
        .await;

    assert!(should_skip_zcode_manual_compact(&session, &turn_context).await);
}

#[tokio::test]
async fn zcode_manual_compact_runs_after_assistant_work() {
    let (session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    let mut config = turn_context.config.as_ref().clone();
    config.harness = Some("zcode".to_string());
    turn_context.config = std::sync::Arc::new(config);
    session
        .record_conversation_items(
            &turn_context,
            &[
                user_message("prompt"),
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "completed work".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
        )
        .await;

    assert!(!should_skip_zcode_manual_compact(&session, &turn_context).await);
}

#[tokio::test]
async fn process_compacted_history_replaces_developer_messages() {
    let compacted_history = vec![
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: "stale permissions".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "summary".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: "stale personality".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let (refreshed, mut expected) = process_compacted_history_with_test_session(
        compacted_history,
        /*previous_turn_settings*/ None,
    )
    .await;
    expected.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "summary".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(refreshed, expected);
}

#[tokio::test]
async fn process_compacted_history_reinjects_full_initial_context() {
    let compacted_history = vec![ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "summary".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }];
    let (refreshed, mut expected) = process_compacted_history_with_test_session(
        compacted_history,
        /*previous_turn_settings*/ None,
    )
    .await;
    expected.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "summary".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(refreshed, expected);
}

#[tokio::test]
async fn process_compacted_history_drops_non_user_content_messages() {
    let compacted_history = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: r#"# AGENTS.md instructions for /repo

<INSTRUCTIONS>
keep me updated
</INSTRUCTIONS>"#
                    .to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: r#"<environment_context>
  <cwd>/repo</cwd>
  <shell>zsh</shell>
</environment_context>"#
                    .to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: r#"<turn_aborted>
  <turn_id>turn-1</turn_id>
  <reason>interrupted</reason>
</turn_aborted>"#
                    .to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "summary".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: "stale developer instructions".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let (refreshed, mut expected) = process_compacted_history_with_test_session(
        compacted_history,
        /*previous_turn_settings*/ None,
    )
    .await;
    expected.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "summary".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(refreshed, expected);
}

#[tokio::test]
async fn process_compacted_history_drops_legacy_warnings() {
    let latest_user = user_message("latest user");
    let compacted_history = vec![
        user_message(
            "Warning: The maximum number of unified exec processes you can keep open is 60 and you currently have 61 processes open. Reuse older processes or close them to prevent automatic pruning of old processes",
        ),
        user_message(
            "Warning: apply_patch was requested via exec_command. Use the apply_patch tool instead of exec_command.",
        ),
        user_message(
            "Warning: Your account was flagged for potentially high-risk cyber activity and this request was routed to gpt-5.2 as a fallback. To regain access to gpt-5.3-codex, apply for trusted access: https://chatgpt.com/cyber or learn more: https://developers.openai.com/codex/concepts/cyber-safety",
        ),
        latest_user.clone(),
    ];
    let (refreshed, initial_context) = process_compacted_history_with_test_session(
        compacted_history,
        /*previous_turn_settings*/ None,
    )
    .await;
    let mut expected = initial_context;
    expected.push(latest_user);
    assert_eq!(refreshed, expected);
}

#[tokio::test]
async fn process_compacted_history_inserts_context_before_last_real_user_message_only() {
    let compacted_history = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "older user".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("{SUMMARY_PREFIX}\nsummary text"),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "latest user".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let (refreshed, initial_context) = process_compacted_history_with_test_session(
        compacted_history,
        /*previous_turn_settings*/ None,
    )
    .await;
    let mut expected = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "older user".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("{SUMMARY_PREFIX}\nsummary text"),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    expected.extend(initial_context);
    expected.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "latest user".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(refreshed, expected);
}

#[tokio::test]
async fn process_compacted_history_reinjects_model_switch_message() {
    let compacted_history = vec![ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "summary".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }];
    let previous_turn_settings = PreviousTurnSettings {
        model: "previous-regular-model".to_string(),
        comp_hash: None,
        realtime_active: None,
    };

    let (refreshed, initial_context) = process_compacted_history_with_test_session(
        compacted_history,
        Some(&previous_turn_settings),
    )
    .await;

    let ResponseItem::Message { role, content, .. } = &initial_context[0] else {
        panic!("expected developer message");
    };
    assert_eq!(role, "developer");
    let [ContentItem::InputText { text }, ..] = content.as_slice() else {
        panic!("expected developer text");
    };
    assert!(text.contains("<model_switch>"));

    let mut expected = initial_context;
    expected.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "summary".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(refreshed, expected);
}

#[test]
fn insert_initial_context_before_last_real_user_or_summary_keeps_summary_last() {
    let compacted_history = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "older user".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "latest user".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("{SUMMARY_PREFIX}\nsummary text"),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let initial_context = vec![ResponseItem::Message {
        id: None,
        role: "developer".to_string(),
        content: vec![ContentItem::InputText {
            text: "fresh permissions".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }];

    let refreshed =
        insert_initial_context_before_last_real_user_or_summary(compacted_history, initial_context);
    let expected = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "older user".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: "fresh permissions".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "latest user".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("{SUMMARY_PREFIX}\nsummary text"),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    assert_eq!(refreshed, expected);
}

#[test]
fn insert_initial_context_before_last_real_user_or_summary_keeps_compaction_last() {
    let compacted_history = vec![ResponseItem::Compaction {
        id: None,
        encrypted_content: "encrypted".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }];
    let initial_context = vec![ResponseItem::Message {
        id: None,
        role: "developer".to_string(),
        content: vec![ContentItem::InputText {
            text: "fresh permissions".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }];

    let refreshed =
        insert_initial_context_before_last_real_user_or_summary(compacted_history, initial_context);
    let expected = vec![
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: "fresh permissions".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Compaction {
            id: None,
            encrypted_content: "encrypted".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    assert_eq!(refreshed, expected);
}
