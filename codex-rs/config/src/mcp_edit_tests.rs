use super::*;
use crate::McpServerToolConfig;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[tokio::test]
async fn replace_mcp_servers_serializes_per_tool_approval_overrides() -> anyhow::Result<()> {
    let unique_suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let codex_home = std::env::temp_dir().join(format!(
        "codex-config-mcp-edit-test-{}-{unique_suffix}",
        std::process::id()
    ));
    let servers = BTreeMap::from([(
        "docs".to_string(),
        McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: "docs-server".to_string(),
                args: Vec::new(),
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            experimental_environment: None,
            enabled: true,
            required: false,
            supports_parallel_tool_calls: true,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: Some(AppToolApproval::Auto),
            enabled_tools: None,
            disabled_tools: None,
            scopes: None,
            oauth_resource: None,
            tools: HashMap::from([
                (
                    "search".to_string(),
                    McpServerToolConfig {
                        approval_mode: Some(AppToolApproval::Approve),
                    },
                ),
                (
                    "read".to_string(),
                    McpServerToolConfig {
                        approval_mode: Some(AppToolApproval::Prompt),
                    },
                ),
            ]),
        },
    )]);

    ConfigEditsBuilder::new(&codex_home)
        .replace_mcp_servers(&servers)
        .apply()
        .await?;

    let config_path = codex_home.join(CONFIG_TOML_FILE);
    let serialized = std::fs::read_to_string(&config_path)?;
    assert_eq!(
        serialized,
        r#"[mcp_servers.docs]
command = "docs-server"
supports_parallel_tool_calls = true
default_tools_approval_mode = "auto"

[mcp_servers.docs.tools]

[mcp_servers.docs.tools.read]
approval_mode = "prompt"

[mcp_servers.docs.tools.search]
approval_mode = "approve"
"#
    );

    let loaded = load_global_mcp_servers(&codex_home).await?;
    assert_eq!(loaded, servers);

    std::fs::remove_dir_all(&codex_home)?;

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_edits_write_through_symlink_chain() -> anyhow::Result<()> {
    let unique_suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let codex_home = std::env::temp_dir().join(format!(
        "codex-config-plugin-edit-symlink-test-{}-{unique_suffix}",
        std::process::id()
    ));
    let target_home = std::env::temp_dir().join(format!(
        "codex-config-plugin-edit-symlink-target-{}-{unique_suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&codex_home)?;
    std::fs::create_dir_all(&target_home)?;
    let target_path = target_home.join(CONFIG_TOML_FILE);
    let link_path = codex_home.join("config-link.toml");
    let config_path = codex_home.join(CONFIG_TOML_FILE);
    std::fs::write(
        &target_path,
        r#"[plugins."linear@openai-curated"]
enabled = false
"#,
    )?;
    symlink(&target_path, &link_path)?;
    symlink("config-link.toml", &config_path)?;

    ConfigEditsBuilder::new(&codex_home)
        .set_plugin_enabled("linear@openai-curated", /*enabled*/ true)
        .apply()
        .await?;

    let meta = std::fs::symlink_metadata(&config_path)?;
    assert!(meta.file_type().is_symlink());
    assert_eq!(
        std::fs::read_to_string(target_path)?,
        r#"[plugins."linear@openai-curated"]
enabled = true
"#
    );

    std::fs::remove_dir_all(&codex_home)?;
    std::fs::remove_dir_all(&target_home)?;

    Ok(())
}

#[tokio::test]
async fn plugin_edits_set_and_clear_enabled_entries() -> anyhow::Result<()> {
    let unique_suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let codex_home = std::env::temp_dir().join(format!(
        "codex-config-plugin-edit-test-{}-{unique_suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&codex_home)?;
    std::fs::write(
        codex_home.join(CONFIG_TOML_FILE),
        r#"[plugins."linear@openai-curated"]
enabled = false

[plugins."gmail@openai-curated"]
enabled = true
"#,
    )?;

    ConfigEditsBuilder::new(&codex_home)
        .set_plugin_enabled("linear@openai-curated", /*enabled*/ true)
        .clear_plugin("gmail@openai-curated")
        .apply()
        .await?;

    let serialized = std::fs::read_to_string(codex_home.join(CONFIG_TOML_FILE))?;
    assert_eq!(
        serialized,
        r#"[plugins."linear@openai-curated"]
enabled = true
"#
    );

    std::fs::remove_dir_all(&codex_home)?;

    Ok(())
}

#[tokio::test]
async fn plugin_clear_leaves_malformed_plugins_scalar_unchanged() -> anyhow::Result<()> {
    let unique_suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let codex_home = std::env::temp_dir().join(format!(
        "codex-config-plugin-clear-malformed-test-{}-{unique_suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&codex_home)?;
    let config_path = codex_home.join(CONFIG_TOML_FILE);
    let config = r#"plugins = "bad"
"#;
    std::fs::write(&config_path, config)?;

    ConfigEditsBuilder::new(&codex_home)
        .clear_plugin("linear@openai-curated")
        .apply()
        .await?;

    assert_eq!(std::fs::read_to_string(&config_path)?, config);

    std::fs::remove_dir_all(&codex_home)?;

    Ok(())
}

#[tokio::test]
async fn plugin_clear_missing_entry_does_not_create_config() -> anyhow::Result<()> {
    let unique_suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let codex_home = std::env::temp_dir().join(format!(
        "codex-config-plugin-clear-missing-test-{}-{unique_suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&codex_home)?;
    let config_path = codex_home.join(CONFIG_TOML_FILE);

    ConfigEditsBuilder::new(&codex_home)
        .clear_plugin("linear@openai-curated")
        .apply()
        .await?;

    assert!(!config_path.exists());

    std::fs::remove_dir_all(&codex_home)?;

    Ok(())
}

#[tokio::test]
async fn plugin_enabled_update_preserves_existing_value_decor() -> anyhow::Result<()> {
    let unique_suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let codex_home = std::env::temp_dir().join(format!(
        "codex-config-plugin-edit-decor-test-{}-{unique_suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&codex_home)?;
    let config_path = codex_home.join(CONFIG_TOML_FILE);
    std::fs::write(
        &config_path,
        r#"[plugins."linear@openai-curated"]
enabled = false # keep
"#,
    )?;

    ConfigEditsBuilder::new(&codex_home)
        .set_plugin_enabled("linear@openai-curated", /*enabled*/ true)
        .apply()
        .await?;

    assert_eq!(
        std::fs::read_to_string(&config_path)?,
        r#"[plugins."linear@openai-curated"]
enabled = true # keep
"#
    );

    std::fs::remove_dir_all(&codex_home)?;

    Ok(())
}
