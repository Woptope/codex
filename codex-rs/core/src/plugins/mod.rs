mod injection;
mod mentions;
mod render;
#[cfg(test)]
pub(crate) mod test_support;

use codex_config::types::McpServerConfig;

pub use codex_plugin::AppConnectorId;
pub use codex_plugin::EffectiveSkillRoots;
pub use codex_plugin::PluginCapabilitySummary;
pub use codex_plugin::PluginId;
pub use codex_plugin::PluginIdError;
pub use codex_plugin::PluginTelemetryMetadata;
pub use codex_plugin::validate_plugin_segment;

pub type LoadedPlugin = codex_plugin::LoadedPlugin<McpServerConfig>;
pub type PluginLoadOutcome = codex_plugin::PluginLoadOutcome<McpServerConfig>;

pub(crate) use injection::build_plugin_injections;
pub(crate) use render::render_explicit_plugin_instructions;

pub(crate) use mentions::build_connector_slug_counts;
pub(crate) use mentions::build_skill_name_counts;
pub(crate) use mentions::collect_explicit_app_ids;
pub(crate) use mentions::collect_explicit_plugin_mentions;
pub(crate) use mentions::collect_tool_mentions_from_messages;
