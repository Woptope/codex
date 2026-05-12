//! Automatic fresh-session handoff after repeated live context compactions.

use std::collections::VecDeque;
use std::path::Path;

use codex_protocol::ThreadId;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::EventMsg;

const MAX_RECORDED_MESSAGES: usize = 12;
const MAX_MESSAGE_CHARS: usize = 2_000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AutoHandoffMetadata<'a> {
    pub(crate) thread_id: ThreadId,
    pub(crate) thread_name: Option<&'a str>,
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) rollout_path: Option<&'a Path>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoHandoffRequest {
    pub(crate) prompt: String,
    pub(crate) compaction_count: u32,
    pub(crate) threshold: u32,
}

#[derive(Clone, Debug)]
struct AutoHandoffSnapshot {
    compaction_count: u32,
    recent_user_messages: VecDeque<String>,
    recent_assistant_messages: VecDeque<String>,
}

#[derive(Debug, Default)]
pub(crate) struct AutoHandoffState {
    live_compaction_count: u32,
    handoff_requested: bool,
    recent_user_messages: VecDeque<String>,
    recent_assistant_messages: VecDeque<String>,
}

impl AutoHandoffState {
    pub(crate) fn record_event(
        &mut self,
        event: &EventMsg,
        threshold: Option<u32>,
        metadata: AutoHandoffMetadata<'_>,
    ) -> Option<AutoHandoffRequest> {
        match event {
            EventMsg::ItemCompleted(completed) => match &completed.item {
                TurnItem::UserMessage(message) => {
                    let text = message
                        .content
                        .iter()
                        .filter_map(|input| match input {
                            codex_protocol::user_input::UserInput::Text { text, .. } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.record_user_message(&text);
                    None
                }
                TurnItem::AgentMessage(message) => {
                    let text = message
                        .content
                        .iter()
                        .map(|content| match content {
                            AgentMessageContent::Text { text } => text.as_str(),
                        })
                        .collect::<String>();
                    self.record_assistant_message(&text);
                    None
                }
                TurnItem::ContextCompaction(_) => {
                    self.prompt_after_live_compaction(threshold, metadata)
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn record_user_message(&mut self, message: &str) {
        push_recent(&mut self.recent_user_messages, message);
    }

    fn record_assistant_message(&mut self, message: &str) {
        push_recent(&mut self.recent_assistant_messages, message);
    }

    fn prompt_after_live_compaction(
        &mut self,
        threshold: Option<u32>,
        metadata: AutoHandoffMetadata<'_>,
    ) -> Option<AutoHandoffRequest> {
        let threshold = threshold.filter(|threshold| *threshold > 0)?;
        self.live_compaction_count = self.live_compaction_count.saturating_add(1);
        if self.handoff_requested || self.live_compaction_count < threshold {
            return None;
        }

        self.handoff_requested = true;
        let snapshot = AutoHandoffSnapshot {
            compaction_count: self.live_compaction_count,
            recent_user_messages: self.recent_user_messages.clone(),
            recent_assistant_messages: self.recent_assistant_messages.clone(),
        };
        Some(AutoHandoffRequest {
            prompt: build_prompt(threshold, metadata, &snapshot),
            compaction_count: snapshot.compaction_count,
            threshold,
        })
    }
}

fn build_prompt(
    threshold: u32,
    metadata: AutoHandoffMetadata<'_>,
    snapshot: &AutoHandoffSnapshot,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "Continue the previous Codex session in a fresh context.\n\n\
         The previous session reached the configured context-compaction threshold. \
         Use this handoff as the starting context, then inspect the repository state \
         before making risky changes.\n\n",
    );

    prompt.push_str("Session metadata:\n");
    prompt.push_str(&format!("- Previous thread id: {}\n", metadata.thread_id));
    if let Some(thread_name) = metadata
        .thread_name
        .filter(|value| !value.trim().is_empty())
    {
        prompt.push_str(&format!("- Previous thread name: {}\n", thread_name.trim()));
    }
    if let Some(cwd) = metadata.cwd {
        prompt.push_str(&format!("- Working directory: {}\n", cwd.display()));
    }
    if let Some(rollout_path) = metadata.rollout_path {
        prompt.push_str(&format!(
            "- Previous rollout path: {}\n",
            rollout_path.display()
        ));
    }
    prompt.push_str(&format!(
        "- Context compactions observed: {} of configured threshold {threshold}\n",
        snapshot.compaction_count
    ));

    append_section(
        &mut prompt,
        "Recent user requests",
        &snapshot.recent_user_messages,
    );
    append_section(
        &mut prompt,
        "Recent assistant responses",
        &snapshot.recent_assistant_messages,
    );

    prompt.push_str(
        "\nInstructions for this fresh session:\n\
         - Continue from the latest user objective above.\n\
         - Treat the filesystem and git state as the source of truth.\n\
         - If important context is missing, ask a targeted question before proceeding.\n",
    );
    prompt
}

fn push_recent(messages: &mut VecDeque<String>, message: &str) {
    let message = trim_for_prompt(message, MAX_MESSAGE_CHARS);
    if message.is_empty() {
        return;
    }

    messages.push_back(message);
    while messages.len() > MAX_RECORDED_MESSAGES {
        messages.pop_front();
    }
}

fn append_section(prompt: &mut String, title: &str, messages: &VecDeque<String>) {
    if messages.is_empty() {
        return;
    }

    prompt.push_str(&format!("\n{title}:\n"));
    for (index, message) in messages.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. {}\n",
            index + 1,
            indent_continuation(message)
        ));
    }
}

fn indent_continuation(message: &str) -> String {
    message.replace('\n', "\n   ")
}

fn trim_for_prompt(message: &str, max_chars: usize) -> String {
    let trimmed = message.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let mut truncated = trimmed.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n[truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use codex_protocol::items::AgentMessageContent;
    use codex_protocol::items::AgentMessageItem;
    use codex_protocol::items::ContextCompactionItem;
    use codex_protocol::items::TurnItem;
    use codex_protocol::items::UserMessageItem;
    use codex_protocol::protocol::ItemCompletedEvent;
    use codex_protocol::user_input::UserInput;
    use pretty_assertions::assert_eq;

    use super::*;

    fn metadata() -> AutoHandoffMetadata<'static> {
        AutoHandoffMetadata {
            thread_id: ThreadId::new(),
            thread_name: Some("feature work"),
            cwd: Some(Path::new("/tmp/project")),
            rollout_path: None,
        }
    }

    fn item_completed(item: TurnItem) -> EventMsg {
        EventMsg::ItemCompleted(ItemCompletedEvent {
            thread_id: ThreadId::new(),
            item,
            turn_id: "turn-id".to_string(),
            completed_at_ms: 0,
        })
    }

    #[test]
    fn disabled_threshold_does_not_trigger() {
        let mut state = AutoHandoffState::default();
        let event = item_completed(TurnItem::ContextCompaction(ContextCompactionItem::new()));

        assert_eq!(state.record_event(&event, None, metadata()), None);
        assert_eq!(state.record_event(&event, Some(0), metadata()), None);
    }

    #[test]
    fn threshold_triggers_once() {
        let mut state = AutoHandoffState::default();
        state.record_event(
            &item_completed(TurnItem::UserMessage(UserMessageItem::new(&[
                UserInput::Text {
                    text: "Fix the flaky test.".to_string(),
                    text_elements: Vec::new(),
                },
            ]))),
            Some(2),
            metadata(),
        );
        state.record_event(
            &item_completed(TurnItem::AgentMessage(AgentMessageItem {
                id: "agent".to_string(),
                content: vec![AgentMessageContent::Text {
                    text: "I found the failing assertion.".to_string(),
                }],
                phase: None,
                memory_citation: None,
            })),
            Some(2),
            metadata(),
        );

        let event = item_completed(TurnItem::ContextCompaction(ContextCompactionItem::new()));
        assert_eq!(state.record_event(&event, Some(2), metadata()), None);
        let request = state
            .record_event(&event, Some(2), metadata())
            .expect("second compaction should trigger handoff");
        assert!(request.prompt.contains("Fix the flaky test."));
        assert!(request.prompt.contains("I found the failing assertion."));
        assert!(
            request
                .prompt
                .contains("Context compactions observed: 2 of configured threshold 2")
        );
        assert_eq!(request.compaction_count, 2);
        assert_eq!(request.threshold, 2);
        assert_eq!(state.record_event(&event, Some(2), metadata()), None);
    }
}
