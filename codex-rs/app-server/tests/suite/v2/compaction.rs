//! End-to-end compaction flow tests.
//!
//! Phases:
//! 1) Arrange: mock responses/compact endpoints + config.
//! 2) Act: start a thread and submit multiple turns to trigger auto-compaction.
//! 3) Assert: verify item/started + item/completed notifications for context compaction.

#![expect(clippy::expect_used)]

use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::McpProcess;
use app_test_support::to_response;
use app_test_support::write_chatgpt_auth;
use app_test_support::write_mock_responses_config_toml;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadAutoHandoffNotification;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadGoalGetParams;
use codex_app_server_protocol::ThreadGoalGetResponse;
use codex_app_server_protocol::ThreadGoalStatus;
use codex_app_server_protocol::ThreadGoalUpdatedNotification;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_config::types::AuthCredentialsStoreMode;
use codex_features::Feature;
use codex_protocol::ThreadId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_state::StateRuntime;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

// macOS and Windows Bazel CI can spend tens of seconds starting app-server
// subprocesses or processing test RPCs under load.
#[cfg(any(target_os = "macos", windows))]
const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(not(any(target_os = "macos", windows)))]
const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const AUTO_COMPACT_LIMIT: i64 = 1_000;
const COMPACT_PROMPT: &str = "Summarize the conversation.";
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_compaction_local_emits_started_and_completed_items() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let sse1 = responses::sse(vec![
        responses::ev_assistant_message("m1", "FIRST_REPLY"),
        responses::ev_completed_with_tokens("r1", /*total_tokens*/ 70_000),
    ]);
    let sse2 = responses::sse(vec![
        responses::ev_assistant_message("m2", "SECOND_REPLY"),
        responses::ev_completed_with_tokens("r2", /*total_tokens*/ 330_000),
    ]);
    let sse3 = responses::sse(vec![
        responses::ev_assistant_message("m3", "LOCAL_SUMMARY"),
        responses::ev_completed_with_tokens("r3", /*total_tokens*/ 200),
    ]);
    let sse4 = responses::sse(vec![
        responses::ev_assistant_message("m4", "FINAL_REPLY"),
        responses::ev_completed_with_tokens("r4", /*total_tokens*/ 120),
    ]);
    responses::mount_sse_sequence(&server, vec![sse1, sse2, sse3, sse4]).await;

    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        AUTO_COMPACT_LIMIT,
        /*requires_openai_auth*/ None,
        "mock_provider",
        COMPACT_PROMPT,
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_id = start_thread(&mut mcp).await?;
    for message in ["first", "second", "third"] {
        send_turn_and_wait(&mut mcp, &thread_id, message).await?;
    }

    let started = wait_for_context_compaction_started(&mut mcp).await?;
    let completed = wait_for_context_compaction_completed(&mut mcp).await?;

    let ThreadItem::ContextCompaction { id: started_id } = started.item else {
        unreachable!("started item should be context compaction");
    };
    let ThreadItem::ContextCompaction { id: completed_id } = completed.item else {
        unreachable!("completed item should be context compaction");
    };

    assert_eq!(started.thread_id, thread_id);
    assert_eq!(completed.thread_id, thread_id);
    assert_eq!(started_id, completed_id);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_compaction_remote_emits_started_and_completed_items() -> Result<()> {
    skip_if_no_network!(Ok(()));
    const REMOTE_AUTO_COMPACT_LIMIT: i64 = 200_000;

    let server = responses::start_mock_server().await;
    let sse1 = responses::sse(vec![
        responses::ev_assistant_message("m1", "FIRST_REPLY"),
        responses::ev_completed_with_tokens("r1", /*total_tokens*/ 70_000),
    ]);
    let sse2 = responses::sse(vec![
        responses::ev_assistant_message("m2", "SECOND_REPLY"),
        responses::ev_completed_with_tokens("r2", /*total_tokens*/ 330_000),
    ]);
    let sse3 = responses::sse(vec![
        responses::ev_assistant_message("m3", "FINAL_REPLY"),
        responses::ev_completed_with_tokens("r3", /*total_tokens*/ 120),
    ]);
    let responses_log = responses::mount_sse_sequence(&server, vec![sse1, sse2, sse3]).await;

    let compacted_history = vec![
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "REMOTE_COMPACT_SUMMARY".to_string(),
            }],
            phase: None,
        },
        ResponseItem::Compaction {
            encrypted_content: "ENCRYPTED_COMPACTION_SUMMARY".to_string(),
        },
    ];
    let compact_mock = responses::mount_compact_json_once(
        &server,
        serde_json::json!({ "output": compacted_history }),
    )
    .await;

    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        REMOTE_AUTO_COMPACT_LIMIT,
        Some(true),
        "mock_provider",
        COMPACT_PROMPT,
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("access-chatgpt").plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_id = start_thread(&mut mcp).await?;
    for message in ["first", "second", "third"] {
        send_turn_and_wait(&mut mcp, &thread_id, message).await?;
    }

    let started = wait_for_context_compaction_started(&mut mcp).await?;
    let completed = wait_for_context_compaction_completed(&mut mcp).await?;

    let ThreadItem::ContextCompaction { id: started_id } = started.item else {
        unreachable!("started item should be context compaction");
    };
    let ThreadItem::ContextCompaction { id: completed_id } = completed.item else {
        unreachable!("completed item should be context compaction");
    };

    assert_eq!(started.thread_id, thread_id);
    assert_eq!(completed.thread_id, thread_id);
    assert_eq!(started_id, completed_id);

    let compact_requests = compact_mock.requests();
    assert_eq!(compact_requests.len(), 1);
    assert_eq!(compact_requests[0].path(), "/v1/responses/compact");

    let response_requests = responses_log.requests();
    assert_eq!(response_requests.len(), 3);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_compact_start_triggers_compaction_and_returns_empty_response() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let sse = responses::sse(vec![
        responses::ev_assistant_message("m1", "MANUAL_COMPACT_SUMMARY"),
        responses::ev_completed_with_tokens("r1", /*total_tokens*/ 200),
    ]);
    responses::mount_sse_sequence(&server, vec![sse]).await;

    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        AUTO_COMPACT_LIMIT,
        /*requires_openai_auth*/ None,
        "mock_provider",
        COMPACT_PROMPT,
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_id = start_thread(&mut mcp).await?;
    let compact_id = mcp
        .send_thread_compact_start_request(ThreadCompactStartParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let compact_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(compact_id)),
    )
    .await??;
    let _compact: ThreadCompactStartResponse =
        to_response::<ThreadCompactStartResponse>(compact_resp)?;

    let started = wait_for_context_compaction_started(&mut mcp).await?;
    let completed = wait_for_context_compaction_completed(&mut mcp).await?;

    let ThreadItem::ContextCompaction { id: started_id } = started.item else {
        unreachable!("started item should be context compaction");
    };
    let ThreadItem::ContextCompaction { id: completed_id } = completed.item else {
        unreachable!("completed item should be context compaction");
    };

    assert_eq!(started.thread_id, thread_id);
    assert_eq!(completed.thread_id, thread_id);
    assert_eq!(started_id, completed_id);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_handoff_after_compaction_starts_replacement_thread() -> Result<()> {
    skip_if_no_network!(Ok(()));
    const HANDOFF_AUTO_COMPACT_LIMIT: i64 = 200_000;

    let server = responses::start_mock_server().await;
    let sse1 = responses::sse(vec![
        responses::ev_assistant_message("m1", "FIRST_REPLY"),
        responses::ev_completed_with_tokens("r1", /*total_tokens*/ 120),
    ]);
    let sse2 = responses::sse(vec![
        responses::ev_assistant_message("m2", "LOCAL_SUMMARY"),
        responses::ev_completed_with_tokens("r2", /*total_tokens*/ 200),
    ]);
    let sse3 = responses::sse(vec![
        responses::ev_assistant_message("m3", "GENERATED_HANDOFF_PROMPT"),
        responses::ev_completed_with_tokens("r3", /*total_tokens*/ 120),
    ]);
    let sse4 = responses::sse(vec![
        responses::ev_assistant_message("m4", "HANDOFF_REPLY"),
        responses::ev_completed_with_tokens("r4", /*total_tokens*/ 120),
    ]);
    let responses_log = responses::mount_sse_sequence(&server, vec![sse1, sse2, sse3, sse4]).await;

    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        HANDOFF_AUTO_COMPACT_LIMIT,
        /*requires_openai_auth*/ None,
        "mock_provider",
        COMPACT_PROMPT,
    )?;
    append_auto_new_session_after_compactions(codex_home.path(), 1)?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_id = start_thread(&mut mcp).await?;
    send_turn_and_wait(&mut mcp, &thread_id, "first request").await?;

    let compact_id = mcp
        .send_thread_compact_start_request(ThreadCompactStartParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let compact_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(compact_id)),
    )
    .await??;
    let _compact: ThreadCompactStartResponse =
        to_response::<ThreadCompactStartResponse>(compact_resp)?;

    let completed = wait_for_context_compaction_completed(&mut mcp).await?;
    assert_eq!(completed.thread_id, thread_id);

    let handoff = wait_for_thread_auto_handoff(&mut mcp).await?;
    assert_eq!(handoff.previous_thread_id, thread_id);
    assert_ne!(handoff.thread.id, thread_id);
    assert_eq!(handoff.compaction_count, 1);
    assert_eq!(handoff.threshold, 1);
    wait_for_turn_completed(&mut mcp, &handoff.turn_id).await?;

    let requests = responses_log.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].body_contains_text("Prepare a concise prompt"));
    assert!(requests[2].body_contains_text("Reply only with the prompt"));
    assert!(requests[3].body_contains_text("GENERATED_HANDOFF_PROMPT"));
    assert!(!requests[3].body_contains_text("Prepare a concise prompt"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_handoff_after_compaction_preserves_active_goal() -> Result<()> {
    skip_if_no_network!(Ok(()));
    const HANDOFF_AUTO_COMPACT_LIMIT: i64 = 200_000;

    let server = responses::start_mock_server().await;
    let sse1 = responses::sse(vec![
        responses::ev_assistant_message("m1", "FIRST_REPLY"),
        responses::ev_completed_with_tokens("r1", /*total_tokens*/ 120),
    ]);
    let sse2 = responses::sse(vec![
        responses::ev_assistant_message("m3", "LOCAL_SUMMARY"),
        responses::ev_completed_with_tokens("r3", /*total_tokens*/ 200),
    ]);
    let sse3 = responses::sse(vec![
        responses::ev_assistant_message("m4", "GENERATED_HANDOFF_PROMPT"),
        responses::ev_completed_with_tokens("r4", /*total_tokens*/ 120),
    ]);
    let sse4 = responses::sse(vec![
        responses::ev_assistant_message("m5", "HANDOFF_REPLY"),
        responses::ev_completed_with_tokens("r5", /*total_tokens*/ 120),
    ]);
    responses::mount_sse_sequence(&server, vec![sse1, sse2, sse3, sse4]).await;

    let mut features = BTreeMap::new();
    features.insert(Feature::Goals, true);
    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &features,
        HANDOFF_AUTO_COMPACT_LIMIT,
        /*requires_openai_auth*/ None,
        "mock_provider",
        COMPACT_PROMPT,
    )?;
    append_auto_new_session_after_compactions(codex_home.path(), 1)?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_id = start_thread(&mut mcp).await?;
    send_turn_and_wait(&mut mcp, &thread_id, "first request").await?;
    let state_db =
        StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into()).await?;
    let state_thread_id = ThreadId::from_string(&thread_id)?;
    let goal = state_db
        .thread_goals()
        .replace_thread_goal(
            state_thread_id,
            "finish auto handoff",
            codex_state::ThreadGoalStatus::Active,
            Some(10_000),
        )
        .await?;
    let codex_state::GoalAccountingOutcome::Updated(_) = state_db
        .thread_goals()
        .account_thread_goal_usage(
            state_thread_id,
            /*time_delta_seconds*/ 12,
            /*token_delta*/ 150,
            codex_state::GoalAccountingMode::ActiveOnly,
            Some(goal.goal_id.as_str()),
        )
        .await?
    else {
        unreachable!("active goal should account usage");
    };
    let accounted_goal = get_thread_goal(&mut mcp, &thread_id)
        .await?
        .expect("source goal should exist after direct accounting");
    assert_eq!(accounted_goal.objective, "finish auto handoff");
    assert_eq!(accounted_goal.status, ThreadGoalStatus::Active);
    assert_eq!(accounted_goal.token_budget, Some(10_000));
    assert_eq!(accounted_goal.tokens_used, 150);
    assert_eq!(accounted_goal.time_used_seconds, 12);

    let compact_id = mcp
        .send_thread_compact_start_request(ThreadCompactStartParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let compact_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(compact_id)),
    )
    .await??;
    let _compact: ThreadCompactStartResponse =
        to_response::<ThreadCompactStartResponse>(compact_resp)?;

    let completed = wait_for_context_compaction_completed(&mut mcp).await?;
    assert_eq!(completed.thread_id, thread_id);

    let handoff = wait_for_thread_auto_handoff(&mut mcp).await?;
    assert_eq!(handoff.previous_thread_id, thread_id);
    assert_ne!(handoff.thread.id, thread_id);
    let copied_goal_snapshot = wait_for_thread_goal_updated(&mut mcp, &handoff.thread.id).await?;

    assert_eq!(copied_goal_snapshot.thread_id, handoff.thread.id);
    assert_eq!(copied_goal_snapshot.objective, accounted_goal.objective);
    assert_eq!(copied_goal_snapshot.status, ThreadGoalStatus::Active);
    assert_eq!(
        copied_goal_snapshot.token_budget,
        accounted_goal.token_budget
    );
    assert!(copied_goal_snapshot.tokens_used >= accounted_goal.tokens_used);
    assert!(copied_goal_snapshot.time_used_seconds >= accounted_goal.time_used_seconds);
    assert_eq!(copied_goal_snapshot.created_at, accounted_goal.created_at);
    assert!(copied_goal_snapshot.updated_at >= accounted_goal.updated_at);

    wait_for_turn_completed(&mut mcp, &handoff.turn_id).await?;
    let replacement_goal = get_thread_goal(&mut mcp, &handoff.thread.id)
        .await?
        .expect("replacement should keep the copied goal after the handoff turn");
    assert_eq!(replacement_goal.objective, accounted_goal.objective);
    assert_eq!(replacement_goal.status, ThreadGoalStatus::Active);
    assert_eq!(replacement_goal.token_budget, accounted_goal.token_budget);
    assert!(replacement_goal.tokens_used >= copied_goal_snapshot.tokens_used);
    assert!(replacement_goal.time_used_seconds >= copied_goal_snapshot.time_used_seconds);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_compact_start_rejects_invalid_thread_id() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        AUTO_COMPACT_LIMIT,
        /*requires_openai_auth*/ None,
        "mock_provider",
        COMPACT_PROMPT,
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_thread_compact_start_request(ThreadCompactStartParams {
            thread_id: "not-a-thread-id".to_string(),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert!(error.error.message.contains("invalid thread id"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_compact_start_rejects_unknown_thread_id() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        AUTO_COMPACT_LIMIT,
        /*requires_openai_auth*/ None,
        "mock_provider",
        COMPACT_PROMPT,
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_thread_compact_start_request(ThreadCompactStartParams {
            thread_id: "67e55044-10b1-426f-9247-bb680e5fe0c8".to_string(),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert!(error.error.message.contains("thread not found"));

    Ok(())
}

fn append_auto_new_session_after_compactions(codex_home: &Path, threshold: u32) -> Result<()> {
    let config_path = codex_home.join("config.toml");
    let config = std::fs::read_to_string(&config_path)?;
    let updated = config.replacen(
        "\n[features]",
        &format!("\nauto_new_session_after_compactions = {threshold}\n\n[features]"),
        1,
    );
    std::fs::write(config_path, updated)?;
    Ok(())
}

async fn start_thread(mcp: &mut McpProcess) -> Result<String> {
    let thread_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;
    Ok(thread.id)
}

async fn send_turn_and_wait(mcp: &mut McpProcess, thread_id: &str, text: &str) -> Result<String> {
    let turn_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.to_string(),
            input: vec![V2UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    let TurnStartResponse { turn } = to_response::<TurnStartResponse>(turn_resp)?;
    wait_for_turn_completed(mcp, &turn.id).await?;
    Ok(turn.id)
}

async fn get_thread_goal(
    mcp: &mut McpProcess,
    thread_id: &str,
) -> Result<Option<codex_app_server_protocol::ThreadGoal>> {
    let request_id = mcp
        .send_raw_request(
            "thread/goal/get",
            Some(serde_json::to_value(ThreadGoalGetParams {
                thread_id: thread_id.to_string(),
            })?),
        )
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ThreadGoalGetResponse { goal } = to_response(response)?;
    Ok(goal)
}

async fn wait_for_thread_goal_updated(
    mcp: &mut McpProcess,
    thread_id: &str,
) -> Result<codex_app_server_protocol::ThreadGoal> {
    let notification: JSONRPCNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_matching_notification("thread/goal/updated", |notification| {
            notification.method == "thread/goal/updated"
                && notification
                    .params
                    .as_ref()
                    .and_then(|params| {
                        serde_json::from_value::<ThreadGoalUpdatedNotification>(params.clone()).ok()
                    })
                    .is_some_and(|payload| payload.thread_id == thread_id)
        }),
    )
    .await??;
    let updated: ThreadGoalUpdatedNotification =
        serde_json::from_value(notification.params.expect("thread/goal/updated params"))?;
    Ok(updated.goal)
}

async fn wait_for_turn_completed(mcp: &mut McpProcess, turn_id: &str) -> Result<()> {
    loop {
        let notification: JSONRPCNotification = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_notification_message("turn/completed"),
        )
        .await??;
        let completed: TurnCompletedNotification =
            serde_json::from_value(notification.params.clone().expect("turn/completed params"))?;
        if completed.turn.id == turn_id {
            return Ok(());
        }
    }
}

async fn wait_for_thread_auto_handoff(
    mcp: &mut McpProcess,
) -> Result<ThreadAutoHandoffNotification> {
    let notification: JSONRPCNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/autoHandoff"),
    )
    .await??;
    let handoff: ThreadAutoHandoffNotification =
        serde_json::from_value(notification.params.expect("thread/autoHandoff params"))?;
    Ok(handoff)
}

async fn wait_for_context_compaction_started(
    mcp: &mut McpProcess,
) -> Result<ItemStartedNotification> {
    loop {
        let notification: JSONRPCNotification = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_notification_message("item/started"),
        )
        .await??;
        let started: ItemStartedNotification =
            serde_json::from_value(notification.params.clone().expect("item/started params"))?;
        if let ThreadItem::ContextCompaction { .. } = started.item {
            return Ok(started);
        }
    }
}

async fn wait_for_context_compaction_completed(
    mcp: &mut McpProcess,
) -> Result<ItemCompletedNotification> {
    loop {
        let notification: JSONRPCNotification = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_notification_message("item/completed"),
        )
        .await??;
        let completed: ItemCompletedNotification =
            serde_json::from_value(notification.params.clone().expect("item/completed params"))?;
        if let ThreadItem::ContextCompaction { .. } = completed.item {
            return Ok(completed);
        }
    }
}
