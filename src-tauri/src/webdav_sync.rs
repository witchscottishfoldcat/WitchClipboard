use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

use hmac::{Hmac, Mac};
use reqwest::{
    blocking::{Client, Response},
    header::{ETAG, IF_MATCH, IF_NONE_MATCH},
    Method, StatusCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    crypto::{self, KeyMaterial},
    model::{SyncItem, SyncTombstone},
    storage::SqliteStore,
};

const CONFIG_FILE: &str = "webdav-sync.secret";
const REMOTE_ROOT: &str = "witch-clipboard-v1";
const STATE_FILE: &str = "state.wcs";
const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretConfig {
    enabled: bool,
    url: String,
    username: String,
    password: String,
    sync_key: String,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPatch {
    pub enabled: bool,
    pub url: String,
    pub username: String,
    pub password: Option<String>,
    pub sync_key: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicConfig {
    pub enabled: bool,
    pub url: String,
    pub username: String,
    pub has_password: bool,
    pub has_sync_key: bool,
    pub key_fingerprint: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: &'static str,
    pub last_sync_at: Option<i64>,
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted: usize,
    pub error: Option<String>,
}

impl Default for SyncStatus {
    fn default() -> Self {
        Self {
            state: "idle",
            last_sync_at: None,
            uploaded: 0,
            downloaded: 0,
            deleted: 0,
            error: None,
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    version: u32,
    updated_at: i64,
    items: Vec<SyncItem>,
    tombstones: Vec<SyncTombstone>,
}

pub struct WebDavSync {
    path: PathBuf,
    keys: KeyMaterial,
    status: Mutex<SyncStatus>,
    running: AtomicBool,
}

impl WebDavSync {
    pub fn new(store: &SqliteStore) -> Self {
        Self {
            path: store.data_dir().join(CONFIG_FILE),
            keys: store.keys(),
            status: Mutex::new(SyncStatus::default()),
            running: AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> Result<PublicConfig, String> {
        let config = self.load_config()?;
        Ok(public_config(&config))
    }

    pub fn save_config(&self, patch: ConfigPatch) -> Result<PublicConfig, String> {
        let mut current = self.load_config()?;
        let url = normalize_url(&patch.url)?;
        current.enabled = patch.enabled;
        current.url = url;
        current.username = patch.username.trim().to_string();
        if let Some(password) = patch.password {
            if !password.is_empty() {
                current.password = password;
            }
        }
        if let Some(sync_key) = patch.sync_key {
            if !sync_key.trim().is_empty() {
                crypto::decode_sync_key(&sync_key).map_err(|error| error.to_string())?;
                current.sync_key = sync_key.trim().to_string();
            }
        }
        if current.sync_key.is_empty() {
            current.sync_key = crypto::generate_sync_key();
        }
        if current.enabled
            && (current.url.is_empty()
                || current.username.is_empty()
                || current.password.is_empty())
        {
            return Err("启用 WebDAV 前必须填写地址、用户名和密码".to_string());
        }
        self.write_config(&current)?;
        Ok(public_config(&current))
    }

    pub fn reveal_sync_key(&self) -> Result<String, String> {
        let config = self.load_config()?;
        if config.sync_key.is_empty() {
            return Err("尚未生成同步密钥".to_string());
        }
        Ok(config.sync_key)
    }

    pub fn status(&self) -> SyncStatus {
        self.status
            .lock()
            .expect("sync status lock poisoned")
            .clone()
    }

    pub fn sync_now(&self, store: &SqliteStore) -> Result<SyncStatus, String> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Err("同步已经在进行中".to_string());
        }
        self.set_status(SyncStatus {
            state: "syncing",
            ..self.status()
        });
        let result = self.sync_inner(store);
        self.running.store(false, Ordering::Release);
        match result {
            Ok(status) => {
                self.set_status(status.clone());
                Ok(status)
            }
            Err(error) => {
                let status = SyncStatus {
                    state: "error",
                    error: Some(error.clone()),
                    ..self.status()
                };
                self.set_status(status);
                Err(error)
            }
        }
    }

    fn sync_inner(&self, store: &SqliteStore) -> Result<SyncStatus, String> {
        let config = self.load_config()?;
        if !config.enabled {
            return Err("WebDAV 同步尚未启用".to_string());
        }
        validate_config(&config)?;
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .user_agent(concat!("Witch-Clipboard/", env!("CARGO_PKG_VERSION"), " WebDAV-E2EE"))
            .build()
            .map_err(|error| error.to_string())?;
        ensure_collection(&client, &config, remote_root(&config))?;
        ensure_collection(&client, &config, blobs_root(&config))?;

        for attempt in 0..3 {
            let (remote, etag) = fetch_snapshot(&client, &config)?;
            let mut downloaded = 0;
            let mut deleted = 0;
            for tombstone in &remote.tombstones {
                if store
                    .apply_sync_tombstone(tombstone)
                    .map_err(|error| error.to_string())?
                {
                    deleted += 1;
                }
            }
            for item in &remote.items {
                let image = if let Some(blob_hash) = item.blob_name.as_deref() {
                    if store
                        .image_png_by_hash(blob_hash)
                        .map_err(|error| error.to_string())?
                        .is_none()
                    {
                        Some(fetch_blob(&client, &config, blob_hash)?)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if store
                    .apply_sync_item(item, image.as_deref())
                    .map_err(|error| error.to_string())?
                {
                    downloaded += 1;
                }
            }

            let items = store.all_for_sync().map_err(|error| error.to_string())?;
            let tombstones = store.sync_tombstones().map_err(|error| error.to_string())?;
            let mut uploaded = 0;
            for item in &items {
                if let Some(blob_hash) = item.blob_name.as_deref() {
                    if !remote_blob_exists(&client, &config, blob_hash)? {
                        let png = store
                            .image_png_by_hash(blob_hash)
                            .map_err(|error| error.to_string())?
                            .ok_or_else(|| format!("本地图片 Blob 缺失：{blob_hash}"))?;
                        upload_blob(&client, &config, blob_hash, &png)?;
                        uploaded += 1;
                    }
                }
            }
            let snapshot = Snapshot {
                version: 1,
                updated_at: now_ms(),
                items,
                tombstones,
            };
            match upload_snapshot(&client, &config, &snapshot, etag.as_deref()) {
                Ok(()) => {
                    uploaded += snapshot.items.len();
                    return Ok(SyncStatus {
                        state: "idle",
                        last_sync_at: Some(now_ms()),
                        uploaded,
                        downloaded,
                        deleted,
                        error: None,
                    });
                }
                Err(error) if error == "conflict" && attempt < 2 => continue,
                Err(error) => return Err(error),
            }
        }
        Err("WebDAV 状态连续发生并发冲突，请稍后重试".to_string())
    }

    fn load_config(&self) -> Result<SecretConfig, String> {
        let backup = self.path.with_extension("secret.bak");
        let path = if self.path.exists() {
            &self.path
        } else if backup.exists() {
            &backup
        } else {
            return Ok(SecretConfig::default());
        };
        let sealed = fs::read(path).map_err(|error| error.to_string())?;
        let plain = self
            .keys
            .open_local_secret(&sealed)
            .map_err(|error| error.to_string())?;
        serde_json::from_slice(&plain).map_err(|error| error.to_string())
    }

    fn write_config(&self, config: &SecretConfig) -> Result<(), String> {
        let plain = serde_json::to_vec(config).map_err(|error| error.to_string())?;
        let sealed = self
            .keys
            .seal_local_secret(&plain)
            .map_err(|error| error.to_string())?;
        let temporary = self.path.with_extension("secret.tmp");
        let backup = self.path.with_extension("secret.bak");
        fs::write(&temporary, sealed).map_err(|error| error.to_string())?;
        if self.path.exists() {
            if backup.exists() {
                fs::remove_file(&backup).map_err(|error| error.to_string())?;
            }
            fs::rename(&self.path, &backup).map_err(|error| error.to_string())?;
        }
        if let Err(error) = fs::rename(&temporary, &self.path) {
            if backup.exists() {
                let _ = fs::rename(&backup, &self.path);
            }
            return Err(error.to_string());
        }
        if backup.exists() {
            fs::remove_file(backup).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn set_status(&self, status: SyncStatus) {
        *self.status.lock().expect("sync status lock poisoned") = status;
    }
}

fn public_config(config: &SecretConfig) -> PublicConfig {
    PublicConfig {
        enabled: config.enabled,
        url: config.url.clone(),
        username: config.username.clone(),
        has_password: !config.password.is_empty(),
        has_sync_key: !config.sync_key.is_empty(),
        key_fingerprint: (!config.sync_key.is_empty()).then(|| {
            let digest = Sha256::digest(config.sync_key.as_bytes());
            format!("{:x}", digest)[..12].to_string()
        }),
    }
}

fn normalize_url(raw: &str) -> Result<String, String> {
    let url = raw.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return Ok(url);
    }
    let parsed = reqwest::Url::parse(&url).map_err(|_| "WebDAV 地址格式无效".to_string())?;
    let local_http = parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if parsed.scheme() != "https" && !local_http {
        return Err("WebDAV 必须使用 HTTPS；仅本机测试允许 HTTP".to_string());
    }
    Ok(url)
}

fn validate_config(config: &SecretConfig) -> Result<(), String> {
    normalize_url(&config.url)?;
    crypto::decode_sync_key(&config.sync_key).map_err(|error| error.to_string())?;
    if config.url.is_empty() || config.username.is_empty() || config.password.is_empty() {
        return Err("WebDAV 配置不完整".to_string());
    }
    Ok(())
}

fn request(
    client: &Client,
    config: &SecretConfig,
    method: Method,
    url: String,
) -> reqwest::blocking::RequestBuilder {
    client
        .request(method, url)
        .basic_auth(&config.username, Some(&config.password))
}

fn remote_root(config: &SecretConfig) -> String {
    format!("{}/{}", config.url, REMOTE_ROOT)
}
fn blobs_root(config: &SecretConfig) -> String {
    format!("{}/blobs", remote_root(config))
}
fn state_url(config: &SecretConfig) -> String {
    format!("{}/{}", remote_root(config), STATE_FILE)
}
fn blob_url(config: &SecretConfig, hash: &str) -> Result<String, String> {
    let key = crypto::decode_sync_key(&config.sync_key).map_err(|error| error.to_string())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|error| error.to_string())?;
    mac.update(b"witch-clipboard-blob-id-v1\0");
    mac.update(hash.as_bytes());
    Ok(format!(
        "{}/{:x}.wcs",
        blobs_root(config),
        mac.finalize().into_bytes()
    ))
}

fn ensure_collection(client: &Client, config: &SecretConfig, url: String) -> Result<(), String> {
    let method = Method::from_bytes(b"MKCOL").map_err(|error| error.to_string())?;
    let response = request(client, config, method, url)
        .send()
        .map_err(|error| error.to_string())?;
    if response.status().is_success() || response.status() == StatusCode::METHOD_NOT_ALLOWED {
        Ok(())
    } else {
        Err(format!("WebDAV 创建目录失败：HTTP {}", response.status()))
    }
}

fn read_limited(mut response: Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length as usize > MAX_STATE_BYTES)
    {
        return Err("WebDAV 状态文件超过 64 MB 安全上限".to_string());
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_STATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err("WebDAV 状态文件超过 64 MB 安全上限".to_string());
    }
    Ok(bytes)
}

fn fetch_snapshot(
    client: &Client,
    config: &SecretConfig,
) -> Result<(Snapshot, Option<String>), String> {
    let response = request(client, config, Method::GET, state_url(config))
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok((Snapshot::default(), None));
    }
    if !response.status().is_success() {
        return Err(format!("WebDAV 下载状态失败：HTTP {}", response.status()));
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let encrypted = read_limited(response)?;
    let plain = crypto::open_sync(&config.sync_key, &encrypted)
        .map_err(|_| "无法解密 WebDAV 历史：同步密钥不匹配或远端文件已损坏".to_string())?;
    let snapshot: Snapshot =
        serde_json::from_slice(&plain).map_err(|error| format!("WebDAV 状态格式无效：{error}"))?;
    if snapshot.version != 1 {
        return Err(format!("不支持的 WebDAV 同步版本：{}", snapshot.version));
    }
    validate_snapshot(&snapshot)?;
    Ok((snapshot, etag))
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<(), String> {
    let invalid_item = snapshot.items.iter().find(|item| {
        !is_sha256(&item.hash)
            || item
                .blob_name
                .as_deref()
                .is_some_and(|hash| !is_sha256(hash))
    });
    let invalid_tombstone = snapshot
        .tombstones
        .iter()
        .find(|tombstone| !is_sha256(&tombstone.hash));
    if invalid_item.is_some() || invalid_tombstone.is_some() {
        return Err("WebDAV 状态包含无效的内容哈希".to_string());
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn upload_snapshot(
    client: &Client,
    config: &SecretConfig,
    snapshot: &Snapshot,
    etag: Option<&str>,
) -> Result<(), String> {
    let plain = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
    let encrypted =
        crypto::seal_sync(&config.sync_key, &plain).map_err(|error| error.to_string())?;
    let mut builder = request(client, config, Method::PUT, state_url(config)).body(encrypted);
    builder = if let Some(etag) = etag {
        builder.header(IF_MATCH, etag)
    } else {
        builder.header(IF_NONE_MATCH, "*")
    };
    let response = builder.send().map_err(|error| error.to_string())?;
    if matches!(
        response.status(),
        StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT
    ) {
        return Err("conflict".to_string());
    }
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("WebDAV 上传状态失败：HTTP {}", response.status()))
    }
}

fn remote_blob_exists(client: &Client, config: &SecretConfig, hash: &str) -> Result<bool, String> {
    let response = request(client, config, Method::HEAD, blob_url(config, hash)?)
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == StatusCode::NOT_FOUND {
        Ok(false)
    } else if response.status().is_success() {
        Ok(true)
    } else {
        Err(format!("WebDAV 检查 Blob 失败：HTTP {}", response.status()))
    }
}

fn upload_blob(
    client: &Client,
    config: &SecretConfig,
    hash: &str,
    plain: &[u8],
) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(plain));
    if actual != hash {
        return Err(format!("拒绝上传哈希不匹配的 Blob：{hash}"));
    }
    let encrypted =
        crypto::seal_sync(&config.sync_key, plain).map_err(|error| error.to_string())?;
    let response = request(client, config, Method::PUT, blob_url(config, hash)?)
        .header(IF_NONE_MATCH, "*")
        .body(encrypted)
        .send()
        .map_err(|error| error.to_string())?;
    if response.status().is_success() || response.status() == StatusCode::PRECONDITION_FAILED {
        Ok(())
    } else {
        Err(format!("WebDAV 上传 Blob 失败：HTTP {}", response.status()))
    }
}

fn fetch_blob(client: &Client, config: &SecretConfig, hash: &str) -> Result<Vec<u8>, String> {
    let response = request(client, config, Method::GET, blob_url(config, hash)?)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("WebDAV 下载 Blob 失败：HTTP {}", response.status()));
    }
    let encrypted = read_limited(response)?;
    let plain = crypto::open_sync(&config.sync_key, &encrypted)
        .map_err(|_| format!("Blob 解密失败：{hash}"))?;
    if format!("{:x}", Sha256::digest(&plain)) != hash {
        return Err(format!("Blob 完整性校验失败：{hash}"));
    }
    Ok(plain)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::ListQuery, storage::NewItem};
    use std::{
        io::Write as _,
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    fn mock_webdav(request_count: usize) -> (String, Arc<Mutex<Vec<u8>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let encrypted_state = Arc::new(Mutex::new(Vec::new()));
        let shared_state = encrypted_state.clone();
        let server = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                let mut expected = None;
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if expected.is_none() {
                        if let Some(position) =
                            request.windows(4).position(|value| value == b"\r\n\r\n")
                        {
                            let header = String::from_utf8_lossy(&request[..position]);
                            let length = header
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|value| value.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            expected = Some(position + 4 + length);
                        }
                    }
                    if expected.is_some_and(|length| request.len() >= length) {
                        break;
                    }
                }
                let separator = request
                    .windows(4)
                    .position(|value| value == b"\r\n\r\n")
                    .unwrap();
                let header = String::from_utf8_lossy(&request[..separator]);
                let method = header.split_whitespace().next().unwrap_or_default();
                let path = header.split_whitespace().nth(1).unwrap_or_default();
                assert!(header
                    .to_ascii_lowercase()
                    .contains("authorization: basic "));

                let (status, extra, body) = match method {
                    "MKCOL" => ("201 Created", "", Vec::new()),
                    "GET" if path.ends_with(STATE_FILE) => {
                        let state = shared_state.lock().unwrap().clone();
                        if state.is_empty() {
                            ("404 Not Found", "", Vec::new())
                        } else {
                            ("200 OK", "ETag: \"mock-1\"\r\n", state)
                        }
                    }
                    "PUT" if path.ends_with(STATE_FILE) => {
                        let body = request[separator + 4..].to_vec();
                        *shared_state.lock().unwrap() = body;
                        ("201 Created", "ETag: \"mock-1\"\r\n", Vec::new())
                    }
                    _ => panic!("unexpected WebDAV request: {method} {path}"),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        (format!("http://{address}/dav"), encrypted_state, server)
    }

    #[test]
    fn rejects_insecure_remote_webdav_but_allows_local_test_server() {
        assert!(normalize_url("http://example.com/dav").is_err());
        assert_eq!(
            normalize_url("http://127.0.0.1:8080/dav/").unwrap(),
            "http://127.0.0.1:8080/dav"
        );
        assert_eq!(
            normalize_url("https://dav.example.com/root/").unwrap(),
            "https://dav.example.com/root"
        );
    }

    #[test]
    fn public_config_never_exposes_credentials_or_sync_key() {
        let config = SecretConfig {
            enabled: true,
            url: "https://dav.example.com".into(),
            username: "alice".into(),
            password: "top-secret".into(),
            sync_key: crypto::generate_sync_key(),
        };
        let json = serde_json::to_string(&public_config(&config)).unwrap();
        assert!(!json.contains("top-secret"));
        assert!(!json.contains(&config.sync_key));
        assert!(json.contains("keyFingerprint"));
    }

    #[test]
    fn two_clients_sync_through_ciphertext_only_webdav_state() {
        let (url, encrypted_state, server) = mock_webdav(8);
        let sync_key = crypto::generate_sync_key();
        let first_directory = tempfile::tempdir().unwrap();
        let first_store = SqliteStore::open(first_directory.path()).unwrap();
        let secret = "cloud secret that must not reach WebDAV in plaintext";
        let hash = format!("{:x}", Sha256::digest(secret.as_bytes()));
        first_store
            .add(NewItem {
                kind: "text".into(),
                text: Some(secret.into()),
                html: None,
                preview: secret.into(),
                auto_kind: "plain".into(),
                hash,
                blob_name: None,
                thumb: None,
                width: None,
                height: None,
                bytes: secret.len(),
                source_app: Some("first.exe".into()),
            })
            .unwrap();
        let first_sync = WebDavSync::new(&first_store);
        let patch = ConfigPatch {
            enabled: true,
            url: url.clone(),
            username: "alice".into(),
            password: Some("password".into()),
            sync_key: Some(sync_key.clone()),
        };
        first_sync.save_config(patch).unwrap();
        first_sync
            .save_config(ConfigPatch {
                enabled: true,
                url: url.clone(),
                username: "alice".into(),
                password: None,
                sync_key: None,
            })
            .unwrap();
        first_sync.sync_now(&first_store).unwrap();
        assert!(!encrypted_state
            .lock()
            .unwrap()
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));

        let second_directory = tempfile::tempdir().unwrap();
        let second_store = SqliteStore::open(second_directory.path()).unwrap();
        let second_sync = WebDavSync::new(&second_store);
        second_sync
            .save_config(ConfigPatch {
                enabled: true,
                url,
                username: "alice".into(),
                password: Some("password".into()),
                sync_key: Some(sync_key),
            })
            .unwrap();
        let status = second_sync.sync_now(&second_store).unwrap();
        assert_eq!(status.downloaded, 1);
        let history = second_store.list(&ListQuery::default()).unwrap();
        assert_eq!(history.items[0].text.as_deref(), Some(secret));
        server.join().unwrap();
    }

    #[test]
    fn snapshot_rejects_non_sha256_paths() {
        let snapshot = Snapshot {
            version: 1,
            items: vec![SyncItem {
                kind: "image".into(),
                text: None,
                html: None,
                preview: "bad".into(),
                hash: "a".repeat(64),
                blob_name: Some("../../outside".into()),
                thumb: None,
                width: None,
                height: None,
                bytes: 1,
                source_app: None,
                auto_kind: "image".into(),
                tags: Vec::new(),
                pinned: false,
                use_count: 0,
                created_at: 1,
                last_used_at: 1,
            }],
            ..Snapshot::default()
        };
        assert!(validate_snapshot(&snapshot).is_err());
    }

    #[test]
    fn remote_blob_name_is_keyed_and_does_not_expose_content_hash() {
        let hash = "a".repeat(64);
        let config = SecretConfig {
            url: "https://dav.example.com".into(),
            sync_key: crypto::generate_sync_key(),
            ..SecretConfig::default()
        };
        let url = blob_url(&config, &hash).unwrap();
        assert!(!url.contains(&hash));
        assert_eq!(url, blob_url(&config, &hash).unwrap());

        let other = SecretConfig {
            sync_key: crypto::generate_sync_key(),
            ..config.clone()
        };
        assert_ne!(url, blob_url(&other, &hash).unwrap());
    }
}
