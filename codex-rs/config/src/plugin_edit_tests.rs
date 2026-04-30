use crate::CONFIG_TOML_FILE;
use crate::ConfigEditsBuilder;
use pretty_assertions::assert_eq;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

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
