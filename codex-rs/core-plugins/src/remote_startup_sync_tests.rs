use super::*;
use crate::manager::PluginsManager;
use crate::remote::REMOTE_GLOBAL_MARKETPLACE_NAME;
use codex_config::ConfigLayerStack;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tempfile::tempdir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

const LEGACY_STARTUP_REMOTE_PLUGIN_SYNC_MARKER_FILE: &str = ".tmp/app-server-remote-plugin-sync-v1";

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("file should have a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn write_cached_plugin(codex_home: &Path, marketplace_name: &str, plugin_name: &str) {
    let plugin_root = codex_home
        .join("plugins/cache")
        .join(marketplace_name)
        .join(plugin_name)
        .join("local");
    write_file(
        &plugin_root.join(".codex-plugin/plugin.json"),
        &format!(r#"{{"name":"{plugin_name}"}}"#),
    );
    write_file(&plugin_root.join("skills/SKILL.md"), "skill");
}

async fn mount_installed_plugins(server: &MockServer) {
    let empty_page_body = r#"{
  "plugins": [],
  "pagination": {
    "limit": 50,
    "next_page_token": null
  }
}"#;
    let global_installed_body = r#"{
  "plugins": [
    {
      "id": "plugins~Plugin_linear",
      "name": "linear",
      "scope": "GLOBAL",
      "installation_policy": "AVAILABLE",
      "authentication_policy": "ON_USE",
      "release": {
        "version": "local",
        "bundle_download_url": "https://example.com/linear.tar.gz",
        "display_name": "Linear",
        "description": "Track work in Linear",
        "app_ids": [],
        "interface": {
          "short_description": "Plan and track work",
          "capabilities": ["Read", "Write"]
        },
        "skills": []
      },
      "enabled": true,
      "disabled_skill_names": []
    }
  ],
  "pagination": {
    "limit": 50,
    "next_page_token": null
  }
}"#;

    Mock::given(method("GET"))
        .and(path("/backend-api/ps/plugins/installed"))
        .and(query_param("scope", "GLOBAL"))
        .and(query_param("includeDownloadUrls", "true"))
        .and(header("authorization", "Bearer Access Token"))
        .and(header("chatgpt-account-id", "account_id"))
        .respond_with(ResponseTemplate::new(200).set_body_string(global_installed_body))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/backend-api/ps/plugins/installed"))
        .and(query_param("scope", "WORKSPACE"))
        .and(query_param("includeDownloadUrls", "true"))
        .and(header("authorization", "Bearer Access Token"))
        .and(header("chatgpt-account-id", "account_id"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_page_body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn startup_remote_plugin_sync_refreshes_remote_installed_cache() {
    let tmp = tempdir().expect("tempdir");
    write_cached_plugin(tmp.path(), REMOTE_GLOBAL_MARKETPLACE_NAME, "linear");
    write_file(
        &tmp.path()
            .join(LEGACY_STARTUP_REMOTE_PLUGIN_SYNC_MARKER_FILE),
        "ok\n",
    );

    let server = MockServer::start().await;
    mount_installed_plugins(&server).await;

    let manager = Arc::new(PluginsManager::new(tmp.path().to_path_buf()));
    let auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    let notification_count = Arc::new(AtomicUsize::new(0));
    let notification_count_for_callback = Arc::clone(&notification_count);

    start_startup_remote_plugin_sync_once(RemoteStartupPluginSyncRequest {
        manager: Arc::clone(&manager),
        plugins_enabled: true,
        remote_plugins_enabled: true,
        chatgpt_base_url: format!("{}/backend-api/", server.uri()),
        auth_manager,
        on_effective_plugins_changed: Some(Arc::new(move || {
            notification_count_for_callback.fetch_add(1, Ordering::SeqCst);
        })),
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if notification_count.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remote installed cache should refresh");

    let outcome = manager
        .plugins_for_config(
            &ConfigLayerStack::default(),
            /*plugins_enabled*/ true,
            /*remote_plugins_enabled*/ true,
            /*plugin_hooks_enabled*/ true,
        )
        .await;
    assert_eq!(outcome.plugins().len(), 1);
    assert_eq!(outcome.plugins()[0].config_name, "linear@chatgpt-global");
    assert_eq!(notification_count.load(Ordering::SeqCst), 1);
}
