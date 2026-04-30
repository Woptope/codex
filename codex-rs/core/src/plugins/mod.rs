mod injection;
mod mentions;
mod render;
#[cfg(test)]
pub(crate) mod test_support;

use codex_config::types::McpServerConfig;

pub use codex_core_plugins::ConfiguredMarketplace;
pub use codex_core_plugins::ConfiguredMarketplaceListOutcome;
pub use codex_core_plugins::ConfiguredMarketplacePlugin;
pub use codex_core_plugins::PluginDetail;
pub use codex_core_plugins::PluginDetailsUnavailableReason;
pub use codex_core_plugins::PluginInstallError;
pub use codex_core_plugins::PluginInstallOutcome;
pub use codex_core_plugins::PluginInstallRequest;
pub use codex_core_plugins::PluginReadOutcome;
pub use codex_core_plugins::PluginReadRequest;
pub use codex_core_plugins::PluginRemoteSyncError;
pub use codex_core_plugins::PluginUninstallError;
pub use codex_core_plugins::PluginsManager;
pub use codex_core_plugins::RemotePluginSyncResult;
pub use codex_core_plugins::marketplace_upgrade::ConfiguredMarketplaceUpgradeError as PluginMarketplaceUpgradeError;
pub use codex_core_plugins::marketplace_upgrade::ConfiguredMarketplaceUpgradeOutcome as PluginMarketplaceUpgradeOutcome;
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
