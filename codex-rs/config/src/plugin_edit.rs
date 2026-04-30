use toml_edit::DocumentMut;
use toml_edit::InlineTable;
use toml_edit::Item as TomlItem;
use toml_edit::Table as TomlTable;
use toml_edit::value;

pub(crate) enum PluginConfigEdit {
    SetEnabled { plugin_id: String, enabled: bool },
    Clear { plugin_id: String },
}

impl PluginConfigEdit {
    pub(crate) fn set_enabled(plugin_id: &str, enabled: bool) -> Self {
        Self::SetEnabled {
            plugin_id: plugin_id.to_string(),
            enabled,
        }
    }

    pub(crate) fn clear(plugin_id: &str) -> Self {
        Self::Clear {
            plugin_id: plugin_id.to_string(),
        }
    }
}

pub(crate) fn apply_plugin_config_edit(doc: &mut DocumentMut, edit: &PluginConfigEdit) -> bool {
    match edit {
        PluginConfigEdit::SetEnabled { plugin_id, enabled } => {
            set_plugin_enabled(doc, plugin_id, *enabled)
        }
        PluginConfigEdit::Clear { plugin_id } => clear_plugin(doc, plugin_id),
    }
}

fn set_plugin_enabled(doc: &mut DocumentMut, plugin_id: &str, enabled: bool) -> bool {
    let root = doc.as_table_mut();
    let plugins = ensure_table(root, "plugins", /*implicit*/ true);
    let plugin = ensure_table(plugins, plugin_id, /*implicit*/ false);
    let mut replacement = value(enabled);
    if let Some(existing) = plugin.get("enabled") {
        preserve_decor(existing, &mut replacement);
    }
    plugin["enabled"] = replacement;
    true
}

fn clear_plugin(doc: &mut DocumentMut, plugin_id: &str) -> bool {
    let root = doc.as_table_mut();
    let Some(item) = root.get_mut("plugins") else {
        return false;
    };
    match item {
        TomlItem::Table(plugins) => plugins.remove(plugin_id).is_some(),
        TomlItem::Value(value) => {
            let Some(inline) = value.as_inline_table().cloned() else {
                return false;
            };
            *item = TomlItem::Table(table_from_inline(&inline, /*implicit*/ true));
            if let TomlItem::Table(plugins) = item {
                return plugins.remove(plugin_id).is_some();
            }
            false
        }
        _ => false,
    }
}

fn preserve_decor(existing: &TomlItem, replacement: &mut TomlItem) {
    if let (TomlItem::Value(existing_value), TomlItem::Value(replacement_value)) =
        (existing, replacement)
    {
        replacement_value
            .decor_mut()
            .clone_from(existing_value.decor());
    }
}

fn ensure_table<'a>(parent: &'a mut TomlTable, key: &str, implicit: bool) -> &'a mut TomlTable {
    match parent.get_mut(key) {
        Some(TomlItem::Table(_)) => {}
        Some(item @ TomlItem::Value(_)) => {
            if let Some(inline) = item.as_value().and_then(toml_edit::Value::as_inline_table) {
                *item = TomlItem::Table(table_from_inline(inline, implicit));
            } else {
                *item = TomlItem::Table(new_table(implicit));
            }
        }
        Some(item) => {
            *item = TomlItem::Table(new_table(implicit));
        }
        None => {
            parent.insert(key, TomlItem::Table(new_table(implicit)));
        }
    }
    let Some(TomlItem::Table(table)) = parent.get_mut(key) else {
        unreachable!("inserted value should be a table");
    };
    table
}

fn new_table(implicit: bool) -> TomlTable {
    let mut table = TomlTable::new();
    table.set_implicit(implicit);
    table
}

fn table_from_inline(inline: &InlineTable, implicit: bool) -> TomlTable {
    let mut table = new_table(implicit);
    for (key, value) in inline.iter() {
        let mut value = value.clone();
        value.decor_mut().set_suffix("");
        table.insert(key, TomlItem::Value(value));
    }
    table
}

#[cfg(test)]
#[path = "plugin_edit_tests.rs"]
mod tests;
