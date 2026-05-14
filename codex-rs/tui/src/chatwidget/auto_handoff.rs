//! Automatic fresh-session handoff after repeated context compaction.

use std::path::Path;

use codex_protocol::ThreadId;

use super::ChatWidget;
use super::ShellEscapePolicy;
use super::UserMessage;
use super::UserMessageHistoryOverride;
use super::UserMessageHistoryRecord;
use crate::app_event::AppEvent;

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
    phase: AutoHandoffPhase,
}

#[derive(Debug, Default)]
enum AutoHandoffPhase {
    #[default]
    Idle,
    WaitingForCompactionTurnComplete {
        compaction_count: u32,
        threshold: u32,
    },
    WaitingForPreparationResponse {
        generated_prompt: Option<String>,
    },
    Complete,
}

#[derive(Debug, PartialEq, Eq)]
enum AutoHandoffAction {
    SubmitPreparationPrompt(String),
    StartNewSession(String),
}

impl AutoHandoffState {
    pub(super) fn record_assistant_message(&mut self, message: &str) {
        let AutoHandoffPhase::WaitingForPreparationResponse { generated_prompt } = &mut self.phase
        else {
            return;
        };

        let message = message.trim();
        if !message.is_empty() {
            *generated_prompt = Some(message.to_string());
        }
    }

    pub(super) fn note_live_compaction(&mut self, threshold: Option<u32>) {
        let Some(threshold) = threshold.filter(|threshold| *threshold > 0) else {
            return;
        };
        if !matches!(self.phase, AutoHandoffPhase::Idle) {
            return;
        }

        self.live_compaction_count = self.live_compaction_count.saturating_add(1);
        if self.live_compaction_count < threshold {
            return;
        }

        self.phase = AutoHandoffPhase::WaitingForCompactionTurnComplete {
            compaction_count: self.live_compaction_count,
            threshold,
        };
    }

    fn action_after_turn_complete(
        &mut self,
        metadata: AutoHandoffMetadata<'_>,
    ) -> Option<AutoHandoffAction> {
        match &mut self.phase {
            AutoHandoffPhase::WaitingForCompactionTurnComplete {
                compaction_count,
                threshold,
            } => {
                let prompt = build_preparation_prompt(*threshold, metadata, *compaction_count);
                self.phase = AutoHandoffPhase::WaitingForPreparationResponse {
                    generated_prompt: None,
                };
                Some(AutoHandoffAction::SubmitPreparationPrompt(prompt))
            }
            AutoHandoffPhase::WaitingForPreparationResponse { generated_prompt } => {
                let prompt = generated_prompt.take()?;
                self.phase = AutoHandoffPhase::Complete;
                Some(AutoHandoffAction::StartNewSession(prompt))
            }
            AutoHandoffPhase::Idle | AutoHandoffPhase::Complete => None,
        }
    }
}

impl ChatWidget {
    pub(super) fn record_auto_handoff_user_message(&mut self, _message: &str) {}

    pub(super) fn record_auto_handoff_assistant_message(&mut self, message: &str) {
        if !self.chat_widget_auto_handoff_enabled() {
            return;
        }
        self.auto_handoff.record_assistant_message(message);
    }

    pub(super) fn maybe_prepare_auto_handoff_after_compaction(&mut self, from_replay: bool) {
        if from_replay {
            return;
        }
        if !self.chat_widget_auto_handoff_enabled() {
            return;
        }

        self.auto_handoff
            .note_live_compaction(self.config.auto_new_session_after_compactions);
    }

    pub(super) fn dispatch_pending_auto_handoff(&mut self) {
        if !self.chat_widget_auto_handoff_enabled() {
            return;
        }
        let metadata = AutoHandoffMetadata {
            thread_id: self.thread_id,
            thread_name: self.thread_name.as_deref(),
            cwd: self.current_cwd.as_deref(),
            rollout_path: self.current_rollout_path.as_deref(),
        };
        let Some(action) = self.auto_handoff.action_after_turn_complete(metadata) else {
            return;
        };

        match action {
            AutoHandoffAction::SubmitPreparationPrompt(prompt) => {
                let history_record =
                    UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
                        text: String::new(),
                        text_elements: Vec::new(),
                    });
                let _ = self.submit_user_message_with_history_and_shell_escape_policy(
                    UserMessage::from(prompt),
                    history_record,
                    ShellEscapePolicy::Disallow,
                );
            }
            AutoHandoffAction::StartNewSession(prompt) => {
                self.app_event_tx
                    .send(AppEvent::NewSessionWithInitialPrompt { text: prompt });
            }
        }
    }

    fn chat_widget_auto_handoff_enabled(&self) -> bool {
        matches!(&self.codex_op_target, super::CodexOpTarget::Direct(_))
    }
}

fn build_preparation_prompt(
    threshold: u32,
    metadata: AutoHandoffMetadata<'_>,
    compaction_count: u32,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "Prepare a concise prompt to continue this work in a fresh Codex session.\n\n\
         The current session reached the configured context-compaction threshold. \
         Do not continue implementation in this turn. Reply only with the prompt for the next \
         Codex session.\n\n\
         Include all important context, current status, checkpoints, files changed, blockers, \
         validation done, validation pending, and exact next steps. The next session will receive \
         your response as its initial user prompt, so make it self-contained and actionable.\n\n",
    );

    prompt.push_str("Session metadata to include if useful:\n");
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
        "- Context compactions observed: {compaction_count} of configured threshold {threshold}\n",
    ));

    prompt
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

        state.note_live_compaction(None);
        assert_eq!(state.action_after_turn_complete(metadata()), None);
        state.note_live_compaction(Some(0));
        assert_eq!(state.action_after_turn_complete(metadata()), None);
    }

    #[test]
    fn threshold_triggers_once() {
        let mut state = AutoHandoffState::default();

        state.note_live_compaction(Some(2));
        assert_eq!(state.action_after_turn_complete(metadata()), None);
        state.note_live_compaction(Some(2));

        let action = state
            .action_after_turn_complete(metadata())
            .expect("second compaction turn completion should prepare handoff");
        let AutoHandoffAction::SubmitPreparationPrompt(prompt) = action else {
            unreachable!("expected preparation prompt");
        };
        assert!(prompt.contains("Prepare a concise prompt"));
        assert!(prompt.contains("Reply only with the prompt"));
        assert!(prompt.contains("Context compactions observed: 2 of configured threshold 2"));

        assert_eq!(state.action_after_turn_complete(metadata()), None);
        state.record_assistant_message("GENERATED HANDOFF PROMPT");

        let action = state
            .action_after_turn_complete(metadata())
            .expect("preparation response should start a new session");
        let AutoHandoffAction::StartNewSession(prompt) = action else {
            unreachable!("expected new-session prompt");
        };
        assert_eq!(prompt, "GENERATED HANDOFF PROMPT");

        state.note_live_compaction(Some(2));
        assert_eq!(state.action_after_turn_complete(metadata()), None);
    }
}
