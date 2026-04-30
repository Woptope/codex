use std::sync::Arc;

use crate::manager::PluginsManager;
use crate::manager::remote_plugin_service_config;
use codex_login::AuthManager;
use tracing::info;
use tracing::warn;

pub(crate) struct RemoteStartupPluginSyncRequest {
    pub(crate) manager: Arc<PluginsManager>,
    pub(crate) plugins_enabled: bool,
    pub(crate) remote_plugins_enabled: bool,
    pub(crate) chatgpt_base_url: String,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) on_effective_plugins_changed: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
}

pub(crate) fn start_startup_remote_plugin_sync_once(request: RemoteStartupPluginSyncRequest) {
    let RemoteStartupPluginSyncRequest {
        manager,
        plugins_enabled,
        remote_plugins_enabled,
        chatgpt_base_url,
        auth_manager,
        on_effective_plugins_changed,
    } = request;
    if !plugins_enabled || !remote_plugins_enabled {
        return;
    }

    tokio::spawn(async move {
        let auth = auth_manager.auth().await;
        match manager
            .refresh_remote_installed_plugins_cache(
                &remote_plugin_service_config(&chatgpt_base_url),
                auth.as_ref(),
            )
            .await
        {
            Ok(cache_changed) => {
                info!(cache_changed, "completed startup remote plugin sync");
                if cache_changed
                    && let Some(on_effective_plugins_changed) = on_effective_plugins_changed
                {
                    on_effective_plugins_changed();
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "startup remote plugin sync failed; will retry on next app-server start"
                );
            }
        }
    });
}

#[cfg(test)]
#[path = "remote_startup_sync_tests.rs"]
mod tests;
