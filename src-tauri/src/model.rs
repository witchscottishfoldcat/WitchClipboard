use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipItem {
    pub id: i64,
    pub kind: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub preview: String,
    pub hash: String,
    pub thumb: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: usize,
    pub source_app: Option<String>,
    pub auto_kind: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub use_count: u32,
    pub created_at: i64,
    pub last_used_at: i64,
    pub note: Option<String>,
    pub hotkey: Option<String>,
    pub group_id: Option<i64>,
}

/// 分组树节点；count 只统计直接挂在节点上的条目（不含子分组）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub count: i64,
}

/// 粘贴变换；大小写类只作用于 ASCII 字母，对中文等无字母文本为恒等变换。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PasteTransform {
    PlainText,
    Upper,
    Lower,
    Capitalize,
    Sentence,
    Camel,
    Trim,
}

pub fn apply_paste_transform(text: &str, transform: PasteTransform) -> String {
    match transform {
        PasteTransform::PlainText => text.to_string(),
        PasteTransform::Upper => text.to_uppercase(),
        PasteTransform::Lower => text.to_lowercase(),
        PasteTransform::Capitalize => text
            .split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        PasteTransform::Sentence => {
            let mut out = String::with_capacity(text.len());
            let mut capitalize_next = true;
            for ch in text.chars() {
                if ch.is_ascii_alphabetic() && capitalize_next {
                    out.extend(ch.to_uppercase());
                    capitalize_next = false;
                } else {
                    out.extend(ch.to_lowercase());
                }
                if ch == '.' || ch == '!' || ch == '?' {
                    capitalize_next = true;
                }
            }
            out
        }
        PasteTransform::Camel => {
            let words: Vec<String> = text
                .split(|c: char| !(c.is_alphanumeric()))
                .filter(|w| !w.is_empty())
                .map(|w| w.to_lowercase())
                .collect();
            let mut out = String::new();
            for (index, word) in words.iter().enumerate() {
                let mut chars = word.chars();
                if index == 0 {
                    out.push_str(&chars.as_str().to_lowercase());
                } else if let Some(first) = chars.next() {
                    out.extend(first.to_uppercase());
                    out.push_str(chars.as_str());
                }
            }
            out
        }
        PasteTransform::Trim => text.trim().to_string(),
    }
}

#[derive(Default, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub q: Option<String>,
    pub kind: Option<String>,
    pub auto_kind: Option<String>,
    pub tag: Option<String>,
    pub pinned_only: Option<bool>,
    /// 分组过滤；0 是保留值表示「未分组」，None 表示不过滤。
    pub group_id: Option<i64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ListResult {
    pub items: Vec<ClipItem>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub total: usize,
    pub pinned: usize,
    pub images: usize,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct PasteOutcome {
    pub ok: bool,
    pub reason: Option<&'static str>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItem {
    pub kind: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub preview: String,
    pub hash: String,
    pub blob_name: Option<String>,
    pub thumb: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: usize,
    pub source_app: Option<String>,
    pub auto_kind: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub use_count: u32,
    pub created_at: i64,
    pub last_used_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTombstone {
    pub hash: String,
    pub deleted_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub hotkey: String,
    pub quick_paste_modifiers: String,
    pub max_items: usize,
    pub max_days: u32,
    /// 本地入库条目大小上限（字节）。0 表示不限制；文件条目只记录路径，不受此限制。
    #[serde(default = "default_max_item_bytes")]
    pub max_item_bytes: usize,
    pub skip_sensitive: bool,
    pub sensitive_apps: Vec<String>,
    pub hide_after_paste: bool,
    pub tray_opens_mini: bool,
    pub visible_filters: Vec<String>,
    pub auto_launch: bool,
    pub theme: String,
    pub accent: String,
    pub opacity: u8,
    pub skipped_version: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "Alt+V".to_string(),
            quick_paste_modifiers: "Ctrl+Alt".to_string(),
            max_items: 2_000,
            max_days: 30,
            max_item_bytes: default_max_item_bytes(),
            skip_sensitive: true,
            sensitive_apps: [
                "keepass",
                "1password",
                "bitwarden",
                "lastpass",
                "enpass",
                "keeweb",
                "dashlane",
                "nordpass",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            hide_after_paste: true,
            tray_opens_mini: true,
            visible_filters: ["all", "text", "image", "files", "url", "key"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            auto_launch: false,
            theme: "system".to_string(),
            accent: "violet".to_string(),
            opacity: 90,
            skipped_version: None,
        }
    }
}

/// 默认单条 20 MB：足够覆盖常见截图与长文本，同时挡住误复制的超大内容拖垮库与界面。
fn default_max_item_bytes() -> usize {
    20 * 1024 * 1024
}
