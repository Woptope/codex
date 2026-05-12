//! Automatic fresh-session handoff after repeated context compaction.

use std::collections::VecDeque;
use std::path::Path;

use codex_protocol::ThreadId;

use super::ChatWidget;
use crate::app_event::AppEvent;

const MAX_RECORDED_MESSAGES: usize = 12;
const MAX_MESSAGE_CHARS: usize = 2_000;

#[derive(Clone, Copy, Debug)]
pub(super) struct AutoHandoffMetadata<'a> {
    pub(super) thread_id: Option<ThreadId>,
    pub(super) thread_name: Option<&'a str>,
    pub(super) cwd: Option<&'a Path>,
    pub(super) rollout_path: Option<&'a Path>,
}

#[derive(Debug, Default)]
pub(super) struct AutoHandoffState {
    live_compaction_count: u32,
    handoff_requested: bool,
    recent_user_messages: VecDeque<String>,
    recent_assistant_messages: VecDeque<String>,
}

impl AutoHandoffState {
    pub(super) fn record_user_message(&mut self, message: &str) {
        push_recent(&mut self.recent_user_messages, message);
    }

    pub(super) fn record_assistant_message(&mut self, message: &str) {
        push_recent(&mut self.recent_assistant_messages, message);
    }

    pub(super) fn prompt_after_live_compaction(
        &mut self,
        threshold: Option<u32>,
        metadata: AutoHandoffMetadata<'_>,
    ) -> Option<String> {
        let threshold = threshold.filter(|threshold| *threshold > 0)?;
        self.live_compaction_count = self.live_compaction_count.saturating_add(1);
        if self.handoff_requested || self.live_compaction_count < threshold {
            return None;
        }

        self.handoff_requested = true;
        Some(self.build_prompt(threshold, metadata))
    }

    fn build_prompt(&self, threshold: u32, metadata: AutoHandoffMetadata<'_>) -> String {
        let mut prompt = String::new();
        prompt.push_str(
            "Continue the previous Codex session in a fresh context.\n\n\
             The previous session reached the configured context-compaction threshold. \
             Use this handoff as the starting context, then inspect the repository state \
             before making risky changes.\n\n",
        );

        prompt.push_str("Session metadata:\n");
        if let Some(thread_id) = metadata.thread_id {
            prompt.push_str(&format!("- Previous thread id: {thread_id}\n"));
        }
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
            self.live_compaction_count
        ));

        append_section(
            &mut prompt,
            "Recent user requests",
            &self.recent_user_messages,
        );
        append_section(
            &mut prompt,
            "Recent assistant responses",
            &self.recent_assistant_messages,
        );

        prompt.push_str(
            "\nInstructions for this fresh session:\n\
             - Continue from the latest user objective above.\n\
             - Treat the filesystem and git state as the source of truth.\n\
             - If important context is missing, ask a targeted question before proceeding.\n",
        );
        prompt
    }
}

impl ChatWidget {
    pub(super) fn record_auto_handoff_user_message(&mut self, message: &str) {
        self.auto_handoff.record_user_message(message);
    }

    pub(super) fn record_auto_handoff_assistant_message(&mut self, message: &str) {
        self.auto_handoff.record_assistant_message(message);
    }

    pub(super) fn maybe_prepare_auto_handoff_after_compaction(&mut self, from_replay: bool) {
        if from_replay {
            return;
        }

        let metadata = AutoHandoffMetadata {
            thread_id: self.thread_id,
            thread_name: self.thread_name.as_deref(),
            cwd: self.current_cwd.as_deref(),
            rollout_path: self.current_rollout_path.as_deref(),
        };
        if let Some(prompt) = self
            .auto_handoff
            .prompt_after_live_compaction(self.config.auto_new_session_after_compactions, metadata)
        {
            self.pending_auto_handoff_prompt = Some(prompt);
        }
    }

    pub(super) fn dispatch_pending_auto_handoff(&mut self) {
        if let Some(prompt) = self.pending_auto_handoff_prompt.take() {
            self.app_event_tx
                .send(AppEvent::NewSessionWithInitialPrompt { text: prompt });
        }
    }
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
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    fn metadata() -> AutoHandoffMetadata<'static> {
        AutoHandoffMetadata {
            thread_id: None,
            thread_name: Some("feature work"),
            cwd: Some(Path::new("/tmp/project")),
            rollout_path: None,
        }
    }

    #[test]
    fn disabled_threshold_does_not_trigger() {
        let mut state = AutoHandoffState::default();

        assert_eq!(state.prompt_after_live_compaction(None, metadata()), None);
        assert_eq!(
            state.prompt_after_live_compaction(Some(0), metadata()),
            None
        );
    }

    #[test]
    fn threshold_triggers_once() {
        let mut state = AutoHandoffState::default();
        state.record_user_message("Fix the flaky test.");
        state.record_assistant_message("I found the failing assertion.");

        assert_eq!(
            state.prompt_after_live_compaction(Some(2), metadata()),
            None
        );
        let prompt = state
            .prompt_after_live_compaction(Some(2), metadata())
            .expect("second compaction should trigger handoff");
        assert!(prompt.contains("Fix the flaky test."));
        assert!(prompt.contains("I found the failing assertion."));
        assert!(prompt.contains("Context compactions observed: 2 of configured threshold 2"));
        assert_eq!(
            state.prompt_after_live_compaction(Some(2), metadata()),
            None
        );
    }

    #[test]
    fn recent_messages_are_bounded() {
        let mut state = AutoHandoffState::default();
        for index in 0..20 {
            state.record_user_message(&format!("user message {index}"));
        }

        let prompt = state
            .prompt_after_live_compaction(Some(1), metadata())
            .expect("first compaction should trigger handoff");
        assert!(!prompt.contains("user message 0"));
        assert!(prompt.contains("user message 19"));
    }
}
