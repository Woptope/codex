mod injection;
mod mentions;
mod render;
#[cfg(test)]
pub(crate) mod test_support;

pub use codex_core_plugins::LoadedPlugin;
pub use codex_core_plugins::PluginLoadOutcome;
pub use codex_core_plugins::manager::ConfiguredMarketplace;
pub use codex_core_plugins::manager::ConfiguredMarketplaceListOutcome;
pub use codex_core_plugins::manager::ConfiguredMarketplacePlugin;
pub use codex_core_plugins::manager::PluginDetail;
pub use codex_core_plugins::manager::PluginDetailsUnavailableReason;
pub use codex_core_plugins::manager::PluginInstallError;
pub use codex_core_plugins::manager::PluginInstallOutcome;
pub use codex_core_plugins::manager::PluginInstallRequest;
pub use codex_core_plugins::manager::PluginReadOutcome;
pub use codex_core_plugins::manager::PluginReadRequest;
pub use codex_core_plugins::manager::PluginUninstallError;
pub use codex_core_plugins::manager::PluginsManager;
pub use codex_core_plugins::marketplace_upgrade::ConfiguredMarketplaceUpgradeError as PluginMarketplaceUpgradeError;
pub use codex_core_plugins::marketplace_upgrade::ConfiguredMarketplaceUpgradeOutcome as PluginMarketplaceUpgradeOutcome;
pub use codex_plugin::AppConnectorId;
pub use codex_plugin::EffectiveSkillRoots;
pub use codex_plugin::PluginCapabilitySummary;
pub use codex_plugin::PluginId;
pub use codex_plugin::PluginIdError;
pub use codex_plugin::PluginTelemetryMetadata;
pub use codex_plugin::validate_plugin_segment;

pub(crate) use injection::build_plugin_injections;
pub(crate) use render::render_explicit_plugin_instructions;

pub(crate) use mentions::build_connector_slug_counts;
pub(crate) use mentions::build_skill_name_counts;
pub(crate) use mentions::collect_explicit_app_ids;
pub(crate) use mentions::collect_explicit_plugin_mentions;
pub(crate) use mentions::collect_tool_mentions_from_messages;
