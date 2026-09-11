use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::Sender,
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_TEXT_BYTES: usize = 100_000;
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const CHUNK_BYTES: usize = 256 * 1024;
const MAX_REQUEST_BYTES: usize = CHUNK_BYTES + 32 * 1024;

#[derive(Clone)]
pub enum Incoming {
    Text(String),
    Files(Vec<String>),
}

#[derive(Clone)]
enum SharedItem {
    Text {
        text: String,
        preview: String,
        sent_at: i64,
        revision: u64,
    },
    Image {
        png: Vec<u8>,
        preview: String,
        sent_at: i64,
        revision: u64,
    },
    Files {
        files: Vec<SharedFile>,
        preview: String,
        sent_at: i64,
        revision: u64,
    },
}

#[derive(Clone)]
struct SharedFile {
    id: String,
    name: String,
    path: PathBuf,
    size: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferInfo {
    pub id: String,
    pub name: String,
    pub direction: &'static str,
    pub state: &'static str,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
}

struct UploadSession {
    temporary: PathBuf,
    destination: PathBuf,
}

#[derive(Default)]
struct Inner {
    address: Option<String>,
    port: Option<u16>,
    token: Option<String>,
    last_seen_at: Option<i64>,
    latest: Option<SharedItem>,
    revision: u64,
    pending_device: Option<DeviceInfo>,
    approved_device: Option<DeviceInfo>,
    transfers: HashMap<String, TransferInfo>,
    downloads: HashMap<String, SharedFile>,
    uploads: HashMap<String, UploadSession>,
}

pub struct CrossDevice {
    inner: Arc<Mutex<Inner>>,
    generation: Arc<AtomicU64>,
    incoming: Sender<Incoming>,
    download_dir: PathBuf,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub url: Option<String>,
    pub pair_code: Option<String>,
    pub connected: bool,
    pub last_seen_at: Option<i64>,
    pub last_sent_at: Option<i64>,
    pub last_sent_preview: Option<String>,
    pub pending_device: Option<DeviceInfo>,
    pub approved_device: Option<DeviceInfo>,
    pub transfers: Vec<TransferInfo>,
}

#[derive(Serialize)]
pub struct SendResult {
    pub ok: bool,
    pub reason: Option<&'static str>,
}

impl CrossDevice {
    pub fn new(incoming: Sender<Incoming>, data_dir: &Path) -> Self {
        let download_dir = if cfg!(test) {
            data_dir.join("downloads")
        } else {
            dirs::download_dir()
                .unwrap_or_else(|| data_dir.to_path_buf())
                .join("Witch Clipboard")
        };
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            generation: Arc::new(AtomicU64::new(0)),
            incoming,
            download_dir,
        }
    }

    pub fn start(&self) -> Result<Status, String> {
        if self
            .inner
            .lock()
            .map_err(|_| "cross-device lock poisoned")?
            .port
            .is_some()
        {
            return Ok(self.status());
        }
        let address =
            lan_address().ok_or_else(|| "没有检测到可用的局域网 IPv4 地址".to_string())?;
        let listener = TcpListener::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let token = random_id(24);
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "cross-device lock poisoned")?;
            inner.address = Some(address);
            inner.port = Some(port);
            inner.token = Some(token);
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let current = self.generation.clone();
        let inner = self.inner.clone();
        let incoming = self.incoming.clone();
        let download_dir = self.download_dir.clone();
        thread::spawn(move || {
            while current.load(Ordering::Acquire) == generation {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = inner.clone();
                        let incoming = incoming.clone();
                        let download_dir = download_dir.clone();
                        thread::spawn(move || handle(stream, &state, &incoming, &download_dir));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(35));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(self.status())
    }

    pub fn stop(&self) -> Status {
        self.generation.fetch_add(1, Ordering::AcqRel);
        *self.inner.lock().expect("cross-device lock poisoned") = Inner::default();
        self.status()
    }

    pub fn status(&self) -> Status {
        let inner = self.inner.lock().expect("cross-device lock poisoned");
        let running = inner.port.is_some() && inner.token.is_some();
        let url = match (&inner.address, inner.port, &inner.token) {
            (Some(address), Some(port), Some(token)) => {
                Some(format!("http://{address}:{port}/pair/{token}"))
            }
            _ => None,
        };
        let pair_code = inner
            .token
            .as_ref()
            .map(|token| token[token.len() - 6..].to_uppercase());
        let (last_sent_at, last_sent_preview) = match &inner.latest {
            Some(SharedItem::Text {
                sent_at, preview, ..
            })
            | Some(SharedItem::Image {
                sent_at, preview, ..
            })
            | Some(SharedItem::Files {
                sent_at, preview, ..
            }) => (Some(*sent_at), Some(preview.clone())),
            None => (None, None),
        };
        let mut transfers = inner.transfers.values().cloned().collect::<Vec<_>>();
        transfers.sort_by(|left, right| right.id.cmp(&left.id));
        Status {
            running,
            url,
            pair_code,
            connected: running
                && inner.approved_device.is_some()
                && inner
                    .last_seen_at
                    .is_some_and(|value| now_ms() - value < 8_000),
            last_seen_at: inner.last_seen_at,
            last_sent_at,
            last_sent_preview,
            pending_device: inner.pending_device.clone(),
            approved_device: inner.approved_device.clone(),
            transfers,
        }
    }

    pub fn approve_device(&self, id: &str) -> Result<Status, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "cross-device lock poisoned")?;
        let pending = inner
            .pending_device
            .clone()
            .ok_or_else(|| "没有等待确认的设备".to_string())?;
        if pending.id != id {
            return Err("设备确认请求已经变化，请刷新后重试".to_string());
        }
        inner.approved_device = Some(pending);
        inner.pending_device = None;
        drop(inner);
        Ok(self.status())
    }

    pub fn reject_device(&self, id: &str) -> Status {
        let mut inner = self.inner.lock().expect("cross-device lock poisoned");
        if inner
            .pending_device
            .as_ref()
            .is_some_and(|device| device.id == id)
        {
            inner.pending_device = None;
        }
        drop(inner);
        self.status()
    }

    pub fn cancel_transfer(&self, id: &str) -> Status {
        let mut inner = self.inner.lock().expect("cross-device lock poisoned");
        if let Some(transfer) = inner.transfers.get_mut(id) {
            transfer.state = "cancelled";
            transfer.error = Some("已由电脑取消".to_string());
        }
        drop(inner);
        self.status()
    }

    pub fn retry_transfer(&self, id: &str) -> Status {
        let mut inner = self.inner.lock().expect("cross-device lock poisoned");
        if let Some(transfer) = inner.transfers.get_mut(id) {
            if matches!(transfer.state, "failed" | "cancelled") {
                transfer.state = "pending";
                transfer.error = None;
            }
        }
        drop(inner);
        self.status()
    }

    pub fn publish_text(&self, text: String) -> SendResult {
        if !self.status().running {
            return fail("not-running");
        }
        if !self.status().connected {
            return fail("not-approved");
        }
        if text.len() > MAX_TEXT_BYTES {
            return fail("too-large");
        }
        if crate::classify::is_sensitive_sync_text(&text) {
            return fail("sensitive");
        }
        let mut inner = self.inner.lock().expect("cross-device lock poisoned");
        inner.revision += 1;
        inner.latest = Some(SharedItem::Text {
            preview: preview(&text),
            text,
            sent_at: now_ms(),
            revision: inner.revision,
        });
        success()
    }

    pub fn publish_image(&self, png: Vec<u8>, preview: String) -> SendResult {
        if !self.status().running {
            return fail("not-running");
        }
        if !self.status().connected {
            return fail("not-approved");
        }
        if png.is_empty() || png.len() > MAX_IMAGE_BYTES {
            return fail("too-large");
        }
        let mut inner = self.inner.lock().expect("cross-device lock poisoned");
        inner.revision += 1;
        inner.latest = Some(SharedItem::Image {
            png,
            preview,
            sent_at: now_ms(),
            revision: inner.revision,
        });
        success()
    }

    pub fn publish_files(&self, paths: Vec<String>) -> SendResult {
        if !self.status().running {
            return fail("not-running");
        }
        if !self.status().connected {
            return fail("not-approved");
        }
        let mut files = Vec::new();
        for path in paths.into_iter().take(20) {
            let path = PathBuf::from(path);
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            let name = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string());
            files.push(SharedFile {
                id: random_id(12),
                name,
                path,
                size: metadata.len(),
            });
        }
        if files.is_empty() {
            return fail("not-found");
        }
        let mut inner = self.inner.lock().expect("cross-device lock poisoned");
        inner.revision += 1;
        inner.downloads.clear();
        for file in &files {
            inner.downloads.insert(file.id.clone(), file.clone());
            inner.transfers.insert(
                file.id.clone(),
                TransferInfo {
                    id: file.id.clone(),
                    name: file.name.clone(),
                    direction: "download",
                    state: "pending",
                    bytes_transferred: 0,
                    total_bytes: file.size,
                    error: None,
                },
            );
        }
        let preview = if files.len() == 1 {
            files[0].name.clone()
        } else {
            format!("{} 个文件", files.len())
        };
        inner.latest = Some(SharedItem::Files {
            files,
            preview,
            sent_at: now_ms(),
            revision: inner.revision,
        });
        success()
    }
}

fn handle(
    mut stream: TcpStream,
    inner: &Arc<Mutex<Inner>>,
    incoming: &Sender<Incoming>,
    download_dir: &Path,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(12)));
    let peer = stream
        .peer_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| "?".to_string());
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("[cross-device] {peer} read failed: {error}");
            return;
        }
    };
    let first = request.header.lines().next().unwrap_or_default();
    eprintln!("[cross-device] {peer} {first}");
    let mut first_parts = first.split_whitespace();
    let method = first_parts.next().unwrap_or("");
    let raw_path = first_parts.next().unwrap_or("/");
    let clean = raw_path.split('?').next().unwrap_or(raw_path);
    let segments = clean.trim_matches('/').split('/').collect::<Vec<_>>();

    if method == "GET" && segments.first() == Some(&"pair") {
        if valid_token(inner, segments.get(1).copied()) {
            respond(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                mobile_page().as_bytes(),
                &[],
            );
        } else {
            respond_text(&mut stream, 404, "Pairing expired");
        }
        return;
    }
    let token = segments.get(2).copied();
    if !valid_token(inner, token) {
        respond_text(&mut stream, 404, "Pairing expired");
        return;
    }

    if method == "POST" && segments.get(1) == Some(&"hello") {
        let hello = serde_json::from_slice::<Hello>(&request.body).unwrap_or_default();
        if hello.id.trim().is_empty() {
            respond_json(&mut stream, 400, serde_json::json!({"ok":false}));
            return;
        }
        let device = DeviceInfo {
            id: hello.id.chars().take(80).collect(),
            name: hello.name.trim().chars().take(64).collect(),
        };
        let mut state = inner.lock().expect("cross-device lock poisoned");
        let approved = state
            .approved_device
            .as_ref()
            .is_some_and(|current| current.id == device.id);
        if approved {
            state.last_seen_at = Some(now_ms());
        } else {
            state.pending_device = Some(device);
        }
        drop(state);
        respond_json(
            &mut stream,
            200,
            serde_json::json!({"ok":true,"approved":approved}),
        );
        return;
    }

    let device_id = header_value(&request.header, "x-witch-device")
        .or_else(|| query_value(raw_path, "device").map(str::to_string))
        .unwrap_or_default();
    if !approved(inner, &device_id) {
        respond_json(
            &mut stream,
            403,
            serde_json::json!({"ok":false,"reason":"approval-required"}),
        );
        return;
    }
    inner
        .lock()
        .expect("cross-device lock poisoned")
        .last_seen_at = Some(now_ms());

    if method == "GET" && segments.get(1) == Some(&"state") {
        let latest = state_payload(inner, token.unwrap_or_default(), &device_id);
        respond_json(
            &mut stream,
            200,
            serde_json::json!({"ok":true,"latest":latest}),
        );
        return;
    }
    if method == "GET" && segments.get(1) == Some(&"image") {
        let image = inner.lock().ok().and_then(|state| match &state.latest {
            Some(SharedItem::Image { png, .. }) => Some(png.clone()),
            _ => None,
        });
        if let Some(png) = image {
            respond(&mut stream, 200, "image/png", &png, &[]);
        } else {
            respond_text(&mut stream, 404, "Not found");
        }
        return;
    }
    if method == "POST" && segments.get(1) == Some(&"send") {
        let text = serde_json::from_slice::<PhoneText>(&request.body)
            .map(|value| value.text)
            .unwrap_or_default();
        if text.trim().is_empty() || crate::classify::is_sensitive_sync_text(&text) {
            respond_json(&mut stream, 403, serde_json::json!({"ok":false}));
            return;
        }
        let _ = incoming.send(Incoming::Text(text.clone()));
        let mut state = inner.lock().expect("cross-device lock poisoned");
        state.revision += 1;
        state.latest = Some(SharedItem::Text {
            preview: preview(&text),
            text,
            sent_at: now_ms(),
            revision: state.revision,
        });
        respond_json(&mut stream, 200, serde_json::json!({"ok":true}));
        return;
    }
    if method == "POST" && segments.get(1) == Some(&"upload-init") {
        initialize_upload(&mut stream, inner, &request.body, download_dir);
        return;
    }
    if method == "PUT" && segments.get(1) == Some(&"upload-chunk") {
        let id = segments.get(3).copied().unwrap_or_default();
        let offset = query_value(raw_path, "offset")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        append_upload_chunk(&mut stream, inner, incoming, id, offset, &request.body);
        return;
    }
    if method == "POST" && segments.get(1) == Some(&"upload-cancel") {
        let id = segments.get(3).copied().unwrap_or_default();
        cancel_upload(&mut stream, inner, id);
        return;
    }
    if method == "GET" && segments.get(1) == Some(&"file") {
        let id = segments.get(3).copied().unwrap_or_default();
        send_file(
            &mut stream,
            inner,
            id,
            header_value(&request.header, "range"),
        );
        return;
    }
    respond_text(&mut stream, 404, "Not found");
}

struct HttpRequest {
    header: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<HttpRequest> {
    let peer = stream
        .peer_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| "?".to_string());
    let mut data = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let mut expected = None;
    // WouldBlock 在 Windows 上是"数据还没到"的瞬态信号（请求体分多个突发到达时必然出现），
    // 必须继续等待而不是断开；总时长仍由 deadline 兜底，防止死连接占住线程。
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        if std::time::Instant::now() >= deadline {
            eprintln!("[cross-device] {peer} read deadline exceeded at {} bytes", data.len());
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "read deadline exceeded",
            ));
        }
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => {
                eprintln!("[cross-device] {peer} socket read error after {} bytes: {error}", data.len());
                return Err(error);
            }
        };
        if count == 0 {
            if expected.is_some_and(|length| data.len() < length) {
                eprintln!("[cross-device] {peer} client closed early: got {} of expected bytes", data.len());
            }
            break;
        }
        data.extend_from_slice(&chunk[..count]);
        if data.len() > MAX_REQUEST_BYTES {
            eprintln!("[cross-device] {peer} request too large: {} bytes (limit {MAX_REQUEST_BYTES})", data.len());
            return Err(std::io::Error::other("request too large"));
        }
        if expected.is_none() {
            if let Some(position) = data.windows(4).position(|value| value == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&data[..position]);
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
        if expected.is_some_and(|length| data.len() >= length) {
            break;
        }
    }
    let position = data
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .ok_or_else(|| {
            eprintln!("[cross-device] {peer} missing header terminator after {} bytes", data.len());
            std::io::Error::other("missing header")
        })?;
    Ok(HttpRequest {
        header: String::from_utf8_lossy(&data[..position]).into_owned(),
        body: data[position + 4..].to_vec(),
    })
}

#[derive(Default, Deserialize)]
struct Hello {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct PhoneText {
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadInit {
    name: String,
    size: u64,
    client_id: String,
}

fn initialize_upload(
    stream: &mut TcpStream,
    inner: &Arc<Mutex<Inner>>,
    body: &[u8],
    download_dir: &Path,
) {
    let Ok(upload) = serde_json::from_slice::<UploadInit>(body) else {
        respond_json(
            stream,
            400,
            serde_json::json!({"ok":false,"reason":"invalid"}),
        );
        return;
    };
    if upload.size == 0 || upload.size > MAX_FILE_BYTES || upload.client_id.is_empty() {
        respond_json(
            stream,
            413,
            serde_json::json!({"ok":false,"reason":"too-large"}),
        );
        return;
    }
    let name = safe_filename(&upload.name);
    let id = format!("{:x}", Sha256::digest(upload.client_id.as_bytes()))[..24].to_string();
    if fs::create_dir_all(download_dir).is_err() {
        respond_json(stream, 500, serde_json::json!({"ok":false,"reason":"io"}));
        return;
    }
    let destination = unique_destination(download_dir, &name);
    let temporary = download_dir.join(format!(".{id}.part"));
    let offset = fs::metadata(&temporary)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        .min(upload.size);
    let mut state = inner.lock().expect("cross-device lock poisoned");
    state.uploads.entry(id.clone()).or_insert(UploadSession {
        temporary,
        destination,
    });
    state.transfers.insert(
        id.clone(),
        TransferInfo {
            id: id.clone(),
            name,
            direction: "upload",
            state: "transferring",
            bytes_transferred: offset,
            total_bytes: upload.size,
            error: None,
        },
    );
    respond_json(
        stream,
        200,
        serde_json::json!({"ok":true,"id":id,"offset":offset,"chunkSize":CHUNK_BYTES}),
    );
}

fn append_upload_chunk(
    stream: &mut TcpStream,
    inner: &Arc<Mutex<Inner>>,
    incoming: &Sender<Incoming>,
    id: &str,
    offset: u64,
    body: &[u8],
) {
    if body.is_empty() || body.len() > CHUNK_BYTES {
        respond_json(
            stream,
            413,
            serde_json::json!({"ok":false,"reason":"chunk-size"}),
        );
        return;
    }
    let (temporary, destination, total, cancelled) = {
        let state = inner.lock().expect("cross-device lock poisoned");
        let Some(session) = state.uploads.get(id) else {
            respond_json(stream, 404, serde_json::json!({"ok":false}));
            return;
        };
        let Some(transfer) = state.transfers.get(id) else {
            respond_json(stream, 404, serde_json::json!({"ok":false}));
            return;
        };
        (
            session.temporary.clone(),
            session.destination.clone(),
            transfer.total_bytes,
            transfer.state == "cancelled",
        )
    };
    if cancelled {
        respond_json(
            stream,
            409,
            serde_json::json!({"ok":false,"reason":"cancelled"}),
        );
        return;
    }
    let current = fs::metadata(&temporary)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current != offset {
        respond_json(
            stream,
            409,
            serde_json::json!({"ok":false,"reason":"offset","offset":current}),
        );
        return;
    }
    if current.saturating_add(body.len() as u64) > total {
        respond_json(
            stream,
            413,
            serde_json::json!({"ok":false,"reason":"exceeds-total","offset":current}),
        );
        return;
    }
    let write_result = (|| -> std::io::Result<u64> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&temporary)?;
        file.write_all(body)?;
        file.sync_data()?;
        Ok(current + body.len() as u64)
    })();
    let Ok(next) = write_result else {
        set_transfer_failed(inner, id, "写入接收文件失败");
        respond_json(stream, 500, serde_json::json!({"ok":false,"reason":"io"}));
        return;
    };
    let completed = next >= total;
    if completed {
        if fs::rename(&temporary, &destination).is_err() {
            set_transfer_failed(inner, id, "完成文件时重命名失败");
            respond_json(stream, 500, serde_json::json!({"ok":false,"reason":"io"}));
            return;
        }
        let _ = incoming.send(Incoming::Files(vec![destination
            .to_string_lossy()
            .into_owned()]));
    }
    if let Some(transfer) = inner
        .lock()
        .expect("cross-device lock poisoned")
        .transfers
        .get_mut(id)
    {
        transfer.bytes_transferred = next;
        transfer.state = if completed {
            "completed"
        } else {
            "transferring"
        };
    }
    respond_json(
        stream,
        200,
        serde_json::json!({"ok":true,"offset":next,"completed":completed}),
    );
}

fn cancel_upload(stream: &mut TcpStream, inner: &Arc<Mutex<Inner>>, id: &str) {
    if let Some(transfer) = inner
        .lock()
        .expect("cross-device lock poisoned")
        .transfers
        .get_mut(id)
    {
        transfer.state = "cancelled";
        transfer.error = Some("已由手机取消，可稍后续传".to_string());
        respond_json(stream, 200, serde_json::json!({"ok":true}));
    } else {
        respond_json(stream, 404, serde_json::json!({"ok":false}));
    }
}

fn send_file(stream: &mut TcpStream, inner: &Arc<Mutex<Inner>>, id: &str, range: Option<String>) {
    let file = inner
        .lock()
        .ok()
        .and_then(|state| state.downloads.get(id).cloned());
    let Some(shared) = file else {
        respond_text(stream, 404, "Not found");
        return;
    };
    let start = range.as_deref().and_then(parse_range_start).unwrap_or(0);
    if start >= shared.size && shared.size > 0 {
        respond_text(stream, 416, "Invalid range");
        return;
    }
    let Some(length) = shared.size.checked_sub(start) else {
        respond_text(stream, 416, "Invalid range");
        return;
    };
    let Ok(mut file) = File::open(&shared.path) else {
        set_transfer_failed(inner, id, "源文件已不存在");
        respond_text(stream, 404, "Source file missing");
        return;
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        respond_text(stream, 500, "Seek failed");
        return;
    }
    let status = if start > 0 { 206 } else { 200 };
    let mut extra = vec![
        ("Accept-Ranges", "bytes".to_string()),
        (
            "Content-Disposition",
            format!(
                "attachment; filename*=UTF-8''{}",
                percent_encode(&shared.name)
            ),
        ),
    ];
    if start > 0 {
        extra.push((
            "Content-Range",
            format!(
                "bytes {start}-{}/{}",
                shared.size.saturating_sub(1),
                shared.size
            ),
        ));
    }
    write_header(stream, status, "application/octet-stream", length, &extra);
    let mut sent = start;
    let mut buffer = vec![0u8; CHUNK_BYTES];
    while sent < shared.size {
        let cancelled = inner
            .lock()
            .ok()
            .and_then(|state| {
                state
                    .transfers
                    .get(id)
                    .map(|value| value.state == "cancelled")
            })
            .unwrap_or(true);
        if cancelled {
            return;
        }
        let wanted = buffer.len().min((shared.size - sent) as usize);
        let Ok(count) = file.read(&mut buffer[..wanted]) else {
            set_transfer_failed(inner, id, "读取源文件失败");
            return;
        };
        if count == 0 {
            break;
        }
        if stream.write_all(&buffer[..count]).is_err() {
            set_transfer_failed(inner, id, "连接中断，可使用 Range 续传");
            return;
        }
        sent += count as u64;
        if let Some(transfer) = inner
            .lock()
            .expect("cross-device lock poisoned")
            .transfers
            .get_mut(id)
        {
            transfer.state = "transferring";
            transfer.bytes_transferred = sent;
            transfer.error = None;
        }
    }
    if sent == shared.size {
        if let Some(transfer) = inner
            .lock()
            .expect("cross-device lock poisoned")
            .transfers
            .get_mut(id)
        {
            transfer.state = "completed";
            transfer.bytes_transferred = sent;
        }
    }
}

fn state_payload(inner: &Arc<Mutex<Inner>>, token: &str, device_id: &str) -> serde_json::Value {
    let state = inner.lock().expect("cross-device lock poisoned");
    match &state.latest {
        Some(SharedItem::Text {
            text,
            preview,
            sent_at,
            revision,
        }) => {
            serde_json::json!({"revision":revision,"kind":"text","text":text,"preview":preview,"sentAt":sent_at})
        }
        Some(SharedItem::Image {
            preview,
            sent_at,
            revision,
            ..
        }) => {
            serde_json::json!({"revision":revision,"kind":"image","preview":preview,"sentAt":sent_at,"imageUrl":format!("/api/image/{token}?device={}",percent_encode(device_id))})
        }
        Some(SharedItem::Files {
            files,
            preview,
            sent_at,
            revision,
        }) => serde_json::json!({
            "revision":revision,"kind":"files","preview":preview,"sentAt":sent_at,
            "files":files.iter().map(|file| serde_json::json!({"id":file.id,"name":file.name,"size":file.size,"url":format!("/api/file/{token}/{}?device={}",file.id,percent_encode(device_id))})).collect::<Vec<_>>()
        }),
        None => serde_json::Value::Null,
    }
}

fn approved(inner: &Arc<Mutex<Inner>>, id: &str) -> bool {
    !id.is_empty()
        && inner
            .lock()
            .ok()
            .and_then(|state| state.approved_device.as_ref().map(|device| device.id == id))
            .unwrap_or(false)
}

fn valid_token(inner: &Arc<Mutex<Inner>>, token: Option<&str>) -> bool {
    inner
        .lock()
        .ok()
        .and_then(|state| state.token.clone())
        .as_deref()
        == token
}

fn set_transfer_failed(inner: &Arc<Mutex<Inner>>, id: &str, error: &str) {
    if let Some(transfer) = inner
        .lock()
        .expect("cross-device lock poisoned")
        .transfers
        .get_mut(id)
    {
        transfer.state = "failed";
        transfer.error = Some(error.to_string());
    }
}

fn safe_filename(raw: &str) -> String {
    Path::new(raw)
        .file_name()
        .map(|value| {
            value
                .to_string_lossy()
                .chars()
                .filter(|character| !character.is_control())
                .collect::<String>()
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "received-file".to_string())
}

fn unique_destination(directory: &Path, name: &str) -> PathBuf {
    let initial = directory.join(name);
    if !initial.exists() {
        return initial;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..10_000 {
        let candidate = directory.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{}-{name}", now_ms()))
}

fn parse_range_start(value: &str) -> Option<u64> {
    value
        .strip_prefix("bytes=")?
        .split('-')
        .next()?
        .parse()
        .ok()
}
fn query_value<'a>(path: &'a str, key: &str) -> Option<&'a str> {
    path.split_once('?')?.1.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then_some(value)
    })
}
fn header_value(header: &str, name: &str) -> Option<String> {
    header.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

fn respond_json(stream: &mut TcpStream, status: u16, value: serde_json::Value) {
    let body = serde_json::to_vec(&value).unwrap_or_default();
    respond(stream, status, "application/json", &body, &[]);
}
fn respond_text(stream: &mut TcpStream, status: u16, text: &str) {
    respond(
        stream,
        status,
        "text/plain; charset=utf-8",
        text.as_bytes(),
        &[],
    );
}
fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, String)],
) {
    write_header(stream, status, content_type, body.len() as u64, extra);
    let _ = stream.write_all(body);
}
fn write_header(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    length: u64,
    extra: &[(&str, String)],
) {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let mut header = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n");
    for (name, value) in extra {
        header.push_str(&format!("{name}: {value}\r\n"));
    }
    header.push_str("\r\n");
    let _ = stream.write_all(header.as_bytes());
}

/// 用默认路由探测"能上外网的出口 IP"；在 TUN/VPN 场景下可能返回虚拟网卡地址，调用方需校验
fn default_route_address() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:80").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

fn lan_address() -> Option<String> {
    // 默认路由探测会把 TUN/VPN 虚拟网卡当成"出口"（如 Clash TUN 返回 198.18.0.1），
    // 手机访问不到这类地址；只有落在局域网私有网段才可信，否则枚举网卡重新挑选。
    if let Some(address) = default_route_address() {
        if is_rfc1918_address(&address) {
            return Some(address);
        }
    }
    let candidates = crate::platform::lan_candidates();
    if candidates.is_empty() {
        return default_route_address();
    }
    candidates
        .iter()
        .filter_map(|(name, if_type, ip)| {
            lan_candidate_rank(name, *if_type, ip).map(|rank| (rank, ip.clone()))
        })
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, ip)| ip)
}

fn is_rfc1918_address(ip: &str) -> bool {
    ip.parse::<std::net::Ipv4Addr>()
        .map(|v4| v4.is_private())
        .unwrap_or(false)
}

const IF_TYPE_LOOPBACK: u32 = 24;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_TYPE_TUNNEL: u32 = 131;

/// 已知虚拟网卡关键字：这类网卡（VMware/WSL/Hyper-V/Radmin/Clash TUN 等）手机到不了
const VIRTUAL_ADAPTER_KEYWORDS: &[&str] = &[
    "vmware",
    "virtualbox",
    "vethernet",
    "hyper-v",
    "wsl",
    "loopback",
    "radmin",
    "clash",
    "tailscale",
    "zerotier",
    "hamachi",
    "tun",
];

/// 排除环回、隧道类型、非 RFC1918 网段以及名字命中虚拟网卡关键字的候选，
/// 剩余里 Wi-Fi（IEEE 802.11）优先。返回 None 表示该网卡不适合放进二维码。
fn lan_candidate_rank(name: &str, if_type: u32, ip: &str) -> Option<u8> {
    if !is_rfc1918_address(ip) {
        return None;
    }
    if if_type == IF_TYPE_LOOPBACK || if_type == IF_TYPE_TUNNEL {
        return None;
    }
    let lowered = name.to_ascii_lowercase();
    if VIRTUAL_ADAPTER_KEYWORDS
        .iter()
        .any(|keyword| lowered.contains(keyword))
    {
        return None;
    }
    Some(if if_type == IF_TYPE_IEEE80211 { 2 } else { 1 })
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn preview(text: &str) -> String {
    crate::classify::make_preview(text, 160)
}
fn success() -> SendResult {
    SendResult {
        ok: true,
        reason: None,
    }
}
fn fail(reason: &'static str) -> SendResult {
    SendResult {
        ok: false,
        reason: Some(reason),
    }
}
fn random_id(bytes: usize) -> String {
    let mut random = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut random);
    random.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn percent_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(byte) {
                (*byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn mobile_page() -> String {
    r#"<!doctype html><html lang=zh-CN><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>Witch Clipboard</title><style>body{font:15px system-ui;max-width:720px;margin:24px auto;padding:16px;background:#18131f;color:#eee}section{background:#2a2234;padding:18px;border-radius:18px;margin:12px 0}textarea{box-sizing:border-box;width:100%;min-height:110px;padding:12px}button,input{font:inherit}button{padding:10px 16px;background:#8b5cf6;color:white;border:0;border-radius:10px}.muted{color:#aaa;font-size:13px}progress{width:100%}a{color:#c4b5fd}</style><h1>Witch Clipboard</h1><p id=approval>正在请求电脑确认这台设备…</p><main hidden><section><h3>来自电脑</h3><div id=received>等待内容…</div></section><section><h3>发送文字</h3><textarea id=out></textarea><p><button onclick=sendText()>发送</button></p></section><section><h3>发送文件</h3><input id=file type=file multiple><p><button onclick=uploadFiles()>上传</button></p><div id=uploads></div></section></main><script>const token=location.pathname.split('/').pop(),deviceId=()=>crypto.randomUUID?crypto.randomUUID():'wcc-'+Date.now().toString(36)+'-'+Math.random().toString(36).slice(2,10),device=localStorage.wccDevice||(localStorage.wccDevice=deviceId()),headers={'X-Witch-Device':device};let revision=0;async function hello(){try{let r=await fetch('/api/hello/'+token,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:device,name:navigator.platform||'浏览器设备'})}),j=await r.json();if(j.approved){approval.textContent='已由电脑确认';document.querySelector('main').hidden=false;poll()}else setTimeout(hello,1200)}catch(e){approval.textContent='连接失败，请在电脑上重新生成二维码后再次扫码'}}async function poll(){try{let r=await fetch('/api/state/'+token,{headers,cache:'no-store'});if(r.status===403){location.reload();return}let s=await r.json(),v=s.latest;if(v&&v.revision!==revision){revision=v.revision;if(v.kind==='image')received.innerHTML='<img style="max-width:100%" src="'+v.imageUrl+'">';else if(v.kind==='files')received.innerHTML=v.files.map(f=>'<p><a download href="'+f.url+'">'+esc(f.name)+'</a> · '+fmt(f.size)+'</p>').join('');else received.innerHTML='<pre></pre>',received.querySelector('pre').textContent=v.text}}catch{}setTimeout(poll,1000)}async function sendText(){await fetch('/api/send/'+token,{method:'POST',headers:{...headers,'Content-Type':'application/json'},body:JSON.stringify({text:out.value})});out.value=''}async function uploadFiles(){for(const f of file.files)await upload(f)}async function upload(f){let client=device+':'+f.name+':'+f.size+':'+f.lastModified,row=document.createElement('p');uploads.append(row);let init=await fetch('/api/upload-init/'+token,{method:'POST',headers:{...headers,'Content-Type':'application/json'},body:JSON.stringify({name:f.name,size:f.size,clientId:client})}).then(r=>r.json()),offset=init.offset||0;while(offset<f.size){row.textContent=f.name+' '+Math.round(offset/f.size*100)+'%';let chunk=f.slice(offset,Math.min(offset+init.chunkSize,f.size)),ok=false,resync=false;for(let retry=0;retry<3&&!ok&&!resync;retry++){try{let r=await fetch('/api/upload-chunk/'+token+'/'+init.id+'?offset='+offset,{method:'PUT',headers,body:chunk}),j=await r.json();if(r.status===409&&j.offset!=null)offset=j.offset,resync=true;else if(r.ok)offset=j.offset,ok=true}catch{}if(!ok&&!resync)await new Promise(r=>setTimeout(r,500*(retry+1)))}if(resync)continue;if(!ok){row.textContent=f.name+' 传输中断，再次选择同一文件可续传';return}}row.textContent=f.name+' 完成'}function esc(s){let d=document.createElement('div');d.textContent=s;return d.innerHTML}function fmt(n){return n>1048576?(n/1048576).toFixed(1)+' MB':Math.ceil(n/1024)+' KB'}hello()</script></html>"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_rank_rejects_tunnel_virtual_and_non_lan_candidates() {
        // 开发机真实网卡形态：FlClash TUN 抢默认路由返回 198.18.0.1，Radmin 是 26 段
        assert_eq!(lan_candidate_rank("FlClash", IF_TYPE_TUNNEL, "198.18.0.1"), None);
        assert_eq!(lan_candidate_rank("Radmin VPN", 6, "26.139.172.191"), None);
        assert_eq!(lan_candidate_rank("以太网", 6, "169.254.169.111"), None);
        assert_eq!(lan_candidate_rank("loopback", IF_TYPE_LOOPBACK, "127.0.0.1"), None);
        assert_eq!(
            lan_candidate_rank("VMware Network Adapter VMnet8", 6, "10.16.0.1"),
            None
        );
        assert_eq!(
            lan_candidate_rank("vEthernet (WSL (Hyper-V firewall))", 6, "172.23.96.1"),
            None
        );
    }

    #[test]
    fn lan_rank_prefers_wifi_over_ethernet_for_real_lan_addresses() {
        assert_eq!(lan_candidate_rank("WLAN", IF_TYPE_IEEE80211, "192.168.31.143"), Some(2));
        assert_eq!(lan_candidate_rank("以太网", 6, "192.168.1.8"), Some(1));
    }

    #[test]
    fn rfc1918_check_accepts_only_private_lan_ranges() {
        assert!(is_rfc1918_address("192.168.31.143"));
        assert!(is_rfc1918_address("10.0.0.5"));
        assert!(is_rfc1918_address("172.16.1.100"));
        assert!(!is_rfc1918_address("198.18.0.1"));
        assert!(!is_rfc1918_address("169.254.169.111"));
        assert!(!is_rfc1918_address("127.0.0.1"));
        assert!(!is_rfc1918_address("not-an-ip"));
    }

    fn request(port: &str, raw: &str) -> String {
        request_via("127.0.0.1", port, raw)
    }

    /// 手机侧等价请求：连到指定主机（测试里用真实局域网 IP）而不是回环地址
    fn request_via(host: &str, port: &str, raw: &str) -> String {
        let mut stream = TcpStream::connect(format!("{host}:{port}")).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8(response).unwrap()
    }

    /// 真实局域网环境端到端验证：二维码地址必须是真实局域网 IP，
    /// 且手机视角（经该 IP 连接）能加载配对页并完成 hello 握手与桌面批准。
    /// 依赖测试机存在局域网 IPv4，CI/无网卡环境用 `cargo test -- --ignored` 显式跑。
    #[test]
    #[ignore = "requires a machine with a real LAN adapter"]
    fn pair_url_targets_the_real_lan_and_serves_over_it() {
        let lan = lan_address().expect("this machine must expose a LAN IPv4 for the e2e check");
        assert!(
            is_rfc1918_address(&lan),
            "lan_address must be a private LAN address, got {lan}"
        );

        let directory = tempfile::tempdir().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let service = CrossDevice::new(sender, directory.path());
        let status = service.start().unwrap();
        let url = status.url.expect("started service must expose a pair url");
        let (base, token) = url.split_once("/pair/").unwrap();
        let authority = base.strip_prefix("http://").unwrap();
        let host = authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority);
        assert_eq!(
            host, lan,
            "pair url must point at the real LAN address, got {url}"
        );
        let port = authority.rsplit(':').next().unwrap();

        // 手机第 1 步：扫码后浏览器加载配对页
        let page = request_via(lan.trim(), port, &format!("GET /pair/{token} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"));
        assert!(page.contains("200 OK"), "pair page must load over the LAN address");
        assert!(page.contains("Witch Clipboard"), "pair page html must be served");

        // 手机第 2 步：hello 握手，等待桌面批准
        let hello = r#"{"id":"phone-lan","name":"LAN e2e phone"}"#;
        let response = request_via(lan.trim(), port, &format!("POST /api/hello/{token} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", hello.len(), hello));
        assert!(response.contains("\"approved\":false"), "first hello must be pending, got {response}");

        // 桌面批准后，同一设备再次 hello 应获得确认（配对页轮询的就是这条路径）
        service.approve_device("phone-lan").unwrap();
        let response = request_via(lan.trim(), port, &format!("POST /api/hello/{token} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", hello.len(), hello));
        assert!(response.contains("\"approved\":true"), "approved hello must confirm, got {response}");

        // 批准后的设备发送文字，桌面端应收到
        let body = r#"{"text":"via lan"}"#;
        let response = request_via(lan.trim(), port, &format!("POST /api/send/{token} HTTP/1.1\r\nHost: {host}\r\nX-Witch-Device: phone-lan\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body));
        assert!(response.contains("200 OK"));
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), Incoming::Text(value) if value == "via lan")
        );
    }

    fn request_bytes(port: &str, header: &str, body: &[u8]) -> Vec<u8> {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        response
    }

    #[test]
    fn device_must_be_confirmed_before_text_and_file_transfer() {
        let directory = tempfile::tempdir().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let service = CrossDevice::new(sender, directory.path());
        let status = service.start().unwrap();
        let url = status.url.unwrap();
        let tail = url.split_once("/pair/").unwrap();
        let port = tail.0.rsplit(':').next().unwrap();
        let token = tail.1;
        let hello = r#"{"id":"phone-1","name":"Test phone"}"#;
        let response = request(port, &format!("POST /api/hello/{token} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}", hello.len(), hello));
        assert!(response.contains("\"approved\":false"));
        assert!(!service.publish_text("blocked".into()).ok);
        assert_eq!(service.status().pending_device.unwrap().name, "Test phone");
        service.approve_device("phone-1").unwrap();

        let body = r#"{"text":"from phone"}"#;
        let response = request(port, &format!("POST /api/send/{token} HTTP/1.1\r\nHost: localhost\r\nX-Witch-Device: phone-1\r\nContent-Length: {}\r\n\r\n{}", body.len(), body));
        assert!(response.contains("200 OK"));
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), Incoming::Text(value) if value == "from phone")
        );
        assert!(service.publish_text("from desktop".into()).ok);

        let init = r#"{"name":"resume.txt","size":11,"clientId":"phone-1:file-1"}"#;
        let response = request(port, &format!("POST /api/upload-init/{token} HTTP/1.1\r\nHost: localhost\r\nX-Witch-Device: phone-1\r\nContent-Length: {}\r\n\r\n{}", init.len(), init));
        let init_json: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let upload_id = init_json["id"].as_str().unwrap();

        let first = b"hello ";
        let response = request_bytes(port, &format!("PUT /api/upload-chunk/{token}/{upload_id}?offset=0 HTTP/1.1\r\nHost: localhost\r\nX-Witch-Device: phone-1\r\nContent-Length: {}\r\n\r\n", first.len()), first);
        assert!(String::from_utf8_lossy(&response).contains("\"offset\":6"));
        let conflict = request_bytes(port, &format!("PUT /api/upload-chunk/{token}/{upload_id}?offset=0 HTTP/1.1\r\nHost: localhost\r\nX-Witch-Device: phone-1\r\nContent-Length: {}\r\n\r\n", first.len()), first);
        assert!(String::from_utf8_lossy(&conflict).contains("409 Conflict"));
        assert!(String::from_utf8_lossy(&conflict).contains("\"offset\":6"));

        let second = b"world";
        let response = request_bytes(port, &format!("PUT /api/upload-chunk/{token}/{upload_id}?offset=6 HTTP/1.1\r\nHost: localhost\r\nX-Witch-Device: phone-1\r\nContent-Length: {}\r\n\r\n", second.len()), second);
        assert!(String::from_utf8_lossy(&response).contains("\"completed\":true"));
        let uploaded = match receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
            Incoming::Files(paths) => PathBuf::from(&paths[0]),
            Incoming::Text(_) => panic!("expected uploaded file"),
        };
        assert_eq!(fs::read(&uploaded).unwrap(), b"hello world");

        assert!(
            service
                .publish_files(vec![uploaded.to_string_lossy().into_owned()])
                .ok
        );
        let download_id = service
            .status()
            .transfers
            .into_iter()
            .find(|transfer| transfer.direction == "download")
            .unwrap()
            .id;
        let response = request_bytes(port, &format!("GET /api/file/{token}/{download_id}?device=phone-1 HTTP/1.1\r\nHost: localhost\r\nRange: bytes=6-\r\n\r\n"), &[]);
        let separator = response
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .unwrap();
        assert!(String::from_utf8_lossy(&response[..separator]).contains("206 Partial Content"));
        assert_eq!(&response[separator + 4..], b"world");
        assert!(!service.stop().running);
    }

    #[test]
    fn filenames_cannot_escape_download_directory() {
        assert_eq!(safe_filename("../../secret.txt"), "secret.txt");
        assert_eq!(safe_filename("..\\..\\secret.txt"), "secret.txt");
        assert_eq!(
            percent_encode("剪贴板.txt"),
            "%E5%89%AA%E8%B4%B4%E6%9D%BF.txt"
        );
    }
}
