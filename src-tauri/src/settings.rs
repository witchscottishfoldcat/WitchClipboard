use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

use serde_json::Value;

use crate::model::Settings;

pub struct SettingsStore {
    path: PathBuf,
    value: RwLock<Settings>,
}

impl SettingsStore {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("settings.json");
        let value = fs::read_to_string(&path)
            .ok()
            .and_then(|text| {
                serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok()
            })
            .map(merge_settings)
            .unwrap_or_default();
        Self {
            path,
            value: RwLock::new(value),
        }
    }

    pub fn get(&self) -> Settings {
        self.value.read().expect("settings lock poisoned").clone()
    }

    pub fn save_patch(&self, patch: Value) -> Result<Settings, String> {
        let current = self.get();
        let mut merged = serde_json::to_value(current).map_err(|error| error.to_string())?;
        if let (Value::Object(target), Value::Object(source)) = (&mut merged, patch) {
            target.extend(source);
        }
        let next = merge_settings(merged);
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let serialized = serde_json::to_string_pretty(&next).map_err(|error| error.to_string())?;
        fs::write(&self.path, serialized).map_err(|error| error.to_string())?;
        *self.value.write().expect("settings lock poisoned") = next.clone();
        Ok(next)
    }
}

fn merge_settings(value: Value) -> Settings {
    let mut base = serde_json::to_value(Settings::default()).expect("default settings serialize");
    if let (Value::Object(target), Value::Object(source)) = (&mut base, value) {
        target.extend(source);
    }
    let mut settings = serde_json::from_value::<Settings>(base).unwrap_or_default();
    const ACCENTS: &[&str] = &["violet", "blue", "cyan", "teal", "green", "amber", "rose"];
    if !ACCENTS.contains(&settings.accent.as_str()) {
        settings.accent = "violet".to_string();
    }
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        settings.theme = "system".to_string();
    }
    settings.opacity = settings.opacity.clamp(20, 100);
    settings
        .visible_filters
        .retain(|filter| !filter.trim().is_empty() && filter != "all");
    settings.visible_filters.insert(0, "all".to_string());
    settings.visible_filters.dedup();
    let mut optional_count = 0;
    settings.visible_filters.retain(|filter| {
        if filter == "all" {
            true
        } else if optional_count < 5 {
            optional_count += 1;
            true
        } else {
            false
        }
    });
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_defaults_and_normalizes_filters() {
        let settings = merge_settings(serde_json::json!({
            "hotkey": "Ctrl+Space",
            "visibleFilters": ["url", "all", "url"],
            "accent": "invalid"
        }));
        assert_eq!(settings.hotkey, "Ctrl+Space");
        assert_eq!(settings.max_items, 2_000);
        assert_eq!(settings.visible_filters, ["all", "url"]);
        assert_eq!(settings.accent, "violet");
        assert_eq!(settings.opacity, 90);
    }

    #[test]
    fn opacity_is_clamped_to_a_visible_range() {
        assert_eq!(
            merge_settings(serde_json::json!({ "opacity": 0 })).opacity,
            20
        );
        assert_eq!(
            merge_settings(serde_json::json!({ "opacity": 255 })).opacity,
            100
        );
    }

    #[test]
    fn visible_filters_are_limited_to_five_optional_filters() {
        let settings = merge_settings(serde_json::json!({
            "visibleFilters": ["text", "image", "files", "url", "key", "model"]
        }));
        assert_eq!(
            settings.visible_filters,
            ["all", "text", "image", "files", "url", "key"]
        );
    }
}
