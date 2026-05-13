//! Automatic fresh-session handoff after repeated live context compactions.

use std::path::Path;

use codex_protocol::ThreadId;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SessionSource;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AutoHandoffMetadata<'a> {
    pub(crate) thread_id: ThreadId,
    pub(crate) thread_name: Option<&'a str>,
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) rollout_path: Option<&'a Path>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoHandoffPrepareRequest {
    pub(crate) prompt: String,
    pub(crate) compaction_count: u32,
    pub(crate) threshold: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoHandoffReplacementRequest {
    pub(crate) prompt: String,
    pub(crate) compaction_count: u32,
    pub(crate) threshold: u32,
    pub(crate) preparation_turn_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AutoHandoffAction {
    PreparePrompt(AutoHandoffPrepareRequest),
    StartReplacement(AutoHandoffReplacementRequest),
}

pub(crate) fn auto_handoff_threshold_for_session(
    threshold: Option<u32>,
    session_source: &SessionSource,
) -> Option<u32> {
    if session_source.is_internal() {
        return None;
    }

    threshold
}

#[derive(Debug, Default)]
pub(crate) struct AutoHandoffState {
    live_compaction_count: u32,
    phase: AutoHandoffPhase,
}

#[derive(Debug, Default)]
enum AutoHandoffPhase {
    #[default]
    Idle,
    WaitingForCompactionTurnComplete {
        turn_id: String,
        compaction_count: u32,
        threshold: u32,
    },
    AwaitingPreparationSubmission {
        compaction_count: u32,
        threshold: u32,
    },
    WaitingForPreparationTurnComplete {
        turn_id: String,
        compaction_count: u32,
        threshold: u32,
    },
    Complete,
}

impl AutoHandoffState {
    pub(crate) fn record_event(
        &mut self,
        event: &EventMsg,
        threshold: Option<u32>,
        metadata: AutoHandoffMetadata<'_>,
    ) -> Option<AutoHandoffAction> {
        match event {
            EventMsg::ItemCompleted(completed) => match &completed.item {
                TurnItem::ContextCompaction(_) => {
                    self.note_live_compaction(&completed.turn_id, threshold)
                }
                _ => None,
            },
            EventMsg::TurnComplete(completed) => self.on_turn_complete(
                &completed.turn_id,
                completed.last_agent_message.as_deref(),
                metadata,
            ),
            EventMsg::TurnAborted(aborted) => {
                if let Some(turn_id) = aborted.turn_id.as_deref() {
                    self.on_turn_aborted(turn_id);
                }
                None
            }
            _ => None,
        }
    }

    fn note_live_compaction(
        &mut self,
        turn_id: &str,
        threshold: Option<u32>,
    ) -> Option<AutoHandoffAction> {
        let threshold = threshold.filter(|threshold| *threshold > 0)?;
        if !matches!(self.phase, AutoHandoffPhase::Idle) {
            return None;
        }

        self.live_compaction_count = self.live_compaction_count.saturating_add(1);
        if self.live_compaction_count < threshold {
            return None;
        }

        self.phase = AutoHandoffPhase::WaitingForCompactionTurnComplete {
            turn_id: turn_id.to_string(),
            compaction_count: self.live_compaction_count,
            threshold,
        };
        None
    }

    fn on_turn_complete(
        &mut self,
        turn_id: &str,
        last_agent_message: Option<&str>,
        metadata: AutoHandoffMetadata<'_>,
    ) -> Option<AutoHandoffAction> {
        match &self.phase {
            AutoHandoffPhase::WaitingForCompactionTurnComplete {
                turn_id: compaction_turn_id,
                compaction_count,
                threshold,
            } if compaction_turn_id == turn_id => {
                let compaction_count = *compaction_count;
                let threshold = *threshold;
                self.phase = AutoHandoffPhase::AwaitingPreparationSubmission {
                    compaction_count,
                    threshold,
                };
                Some(AutoHandoffAction::PreparePrompt(
                    AutoHandoffPrepareRequest {
                        prompt: build_preparation_prompt(threshold, metadata, compaction_count),
                        compaction_count,
                        threshold,
                    },
                ))
            }
            AutoHandoffPhase::WaitingForPreparationTurnComplete {
                turn_id: preparation_turn_id,
                compaction_count,
                threshold,
            } if preparation_turn_id == turn_id => {
                let prompt = last_agent_message?.trim();
                if prompt.is_empty() {
                    self.phase = AutoHandoffPhase::Complete;
                    return None;
                }
                let request = AutoHandoffReplacementRequest {
                    prompt: prompt.to_string(),
                    compaction_count: *compaction_count,
                    threshold: *threshold,
                    preparation_turn_id: preparation_turn_id.clone(),
                };
                self.phase = AutoHandoffPhase::Complete;
                Some(AutoHandoffAction::StartReplacement(request))
            }
            _ => None,
        }
    }

    fn on_turn_aborted(&mut self, turn_id: &str) {
        match &self.phase {
            AutoHandoffPhase::WaitingForCompactionTurnComplete {
                turn_id: compaction_turn_id,
                ..
            } if compaction_turn_id == turn_id => {
                self.phase = AutoHandoffPhase::Complete;
            }
            AutoHandoffPhase::WaitingForPreparationTurnComplete {
                turn_id: preparation_turn_id,
                ..
            } if preparation_turn_id == turn_id => {
                self.phase = AutoHandoffPhase::Complete;
            }
            _ => {}
        }
    }

    pub(crate) fn mark_preparation_submitted(&mut self, turn_id: String) -> bool {
        let AutoHandoffPhase::AwaitingPreparationSubmission {
            compaction_count,
            threshold,
        } = &self.phase
        else {
            return false;
        };
        self.phase = AutoHandoffPhase::WaitingForPreparationTurnComplete {
            turn_id,
            compaction_count: *compaction_count,
            threshold: *threshold,
        };
        true
    }

    pub(crate) fn mark_preparation_submission_failed(&mut self) {
        if matches!(
            self.phase,
            AutoHandoffPhase::AwaitingPreparationSubmission { .. }
        ) {
            self.phase = AutoHandoffPhase::Complete;
        }
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
        "- Context compactions observed: {compaction_count} of configured threshold {threshold}\n",
    ));

    prompt
}

#[cfg(test)]
mod tests {
    use codex_protocol::items::ContextCompactionItem;
    use codex_protocol::items::TurnItem;
    use codex_protocol::protocol::ItemCompletedEvent;
    use codex_protocol::protocol::SubAgentSource;
    use codex_protocol::protocol::TurnCompleteEvent;
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

    fn item_completed(turn_id: &str, item: TurnItem) -> EventMsg {
        EventMsg::ItemCompleted(ItemCompletedEvent {
            thread_id: ThreadId::new(),
            item,
            turn_id: turn_id.to_string(),
            completed_at_ms: 0,
        })
    }

    fn turn_complete(turn_id: &str, last_agent_message: Option<&str>) -> EventMsg {
        EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id: turn_id.to_string(),
            last_agent_message: last_agent_message.map(str::to_string),
            completed_at: None,
            duration_ms: None,
            time_to_first_token_ms: None,
        })
    }

    #[test]
    fn disabled_threshold_does_not_trigger() {
        let mut state = AutoHandoffState::default();
        let event = item_completed(
            "compact-turn",
            TurnItem::ContextCompaction(ContextCompactionItem::new()),
        );

        assert_eq!(state.record_event(&event, None, metadata()), None);
        assert_eq!(state.record_event(&event, Some(0), metadata()), None);
    }

    #[test]
    fn threshold_is_enabled_for_subagents() {
        assert_eq!(
            auto_handoff_threshold_for_session(Some(2), &SessionSource::Mcp),
            Some(2)
        );

        let subagent_source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: ThreadId::new(),
            depth: 1,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
        });
        assert_eq!(
            auto_handoff_threshold_for_session(Some(2), &subagent_source),
            Some(2)
        );
    }

    #[test]
    fn threshold_triggers_once() {
        let mut state = AutoHandoffState::default();
        let first_compaction = item_completed(
            "compact-turn-1",
            TurnItem::ContextCompaction(ContextCompactionItem::new()),
        );
        let second_compaction = item_completed(
            "compact-turn-2",
            TurnItem::ContextCompaction(ContextCompactionItem::new()),
        );

        assert_eq!(
            state.record_event(&first_compaction, Some(2), metadata()),
            None
        );
        assert_eq!(
            state.record_event(&second_compaction, Some(2), metadata()),
            None
        );

        let prepare = state
            .record_event(&turn_complete("compact-turn-2", None), Some(2), metadata())
            .expect("compaction turn completion should request a handoff prompt");
        let AutoHandoffAction::PreparePrompt(prepare) = prepare else {
            unreachable!("expected preparation request");
        };
        assert!(prepare.prompt.contains("Prepare a concise prompt"));
        assert!(prepare.prompt.contains("Reply only with the prompt"));
        assert!(
            prepare
                .prompt
                .contains("Context compactions observed: 2 of configured threshold 2")
        );
        assert_eq!(prepare.compaction_count, 2);
        assert_eq!(prepare.threshold, 2);

        assert!(state.mark_preparation_submitted("prepare-turn".to_string()));
        let replacement = state
            .record_event(
                &turn_complete("prepare-turn", Some("GENERATED HANDOFF PROMPT")),
                Some(2),
                metadata(),
            )
            .expect("preparation turn completion should start replacement");
        let AutoHandoffAction::StartReplacement(replacement) = replacement else {
            unreachable!("expected replacement request");
        };
        assert_eq!(replacement.prompt, "GENERATED HANDOFF PROMPT");
        assert_eq!(replacement.compaction_count, 2);
        assert_eq!(replacement.threshold, 2);
        assert_eq!(replacement.preparation_turn_id, "prepare-turn");

        let third_compaction = item_completed(
            "compact-turn-3",
            TurnItem::ContextCompaction(ContextCompactionItem::new()),
        );
        assert_eq!(
            state.record_event(&third_compaction, Some(2), metadata()),
            None
        );
    }
}
