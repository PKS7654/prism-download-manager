//! Embedded Loopback HTTP Streaming Server for Media Preview
//!
//! Streams downloading media (.prismpart) over localhost HTTP with range support.
//! Benefits:
//! 1. No duplicate preview file on disk in user's download directory (Explorer shows only 1 file).
//! 2. Progressive streaming: media players (Windows Media Player, VLC, etc.) see an active HTTP
//!    stream and continuously buffer newly downloaded frames in real-time without locking on a static EOF.

use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use prism_core::manager::Core;
use tauri::async_runtime::spawn;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

static SERVER_PORT: AtomicU16 = AtomicU16::new(0);
static CORE_REF: OnceLock<Core> = OnceLock::new();

pub fn init(core: Core) {
    let _ = CORE_REF.set(core.clone());
    spawn(async move {
        if let Err(e) = run_server(core).await {
            tracing::error!("Preview streaming server stopped: {e}");
        }
    });
}

pub fn get_server_port() -> u16 {
    SERVER_PORT.load(Ordering::Relaxed)
}

async fn run_server(core: Core) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let port = addr.port();
    SERVER_PORT.store(port, Ordering::Relaxed);
    tracing::info!("Preview loopback HTTP server listening on http://127.0.0.1:{}", port);

    loop {
        let (socket, _) = match listener.accept().await {
            Ok(res) => res,
            Err(e) => {
                tracing::debug!("Loopback accept error: {e}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let core_clone = core.clone();
        spawn(async move {
            if let Err(e) = handle_connection(socket, core_clone).await {
                tracing::trace!("Preview stream connection ended: {e}");
            }
        });
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    core: Core,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; 4096];
    let n = socket.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut lines = req.lines();
    let request_line = match lines.next() {
        Some(l) => l,
        None => return Ok(()),
    };

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];
    let path = parts[1];

    if method != "GET" && method != "HEAD" {
        let resp = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
        socket.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    // Path format: /stream/<item_id> or /stream/<item_id>/filename
    let path_segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if path_segments.len() < 2 || path_segments[0] != "stream" {
        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        socket.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    let id = path_segments[1];
    let item = match core.item(id) {
        Some(i) => i,
        None => {
            let resp = "HTTP/1.1 404 Item Not Found\r\nContent-Length: 0\r\n\r\n";
            socket.write_all(resp.as_bytes()).await?;
            return Ok(());
        }
    };

    let target_path = item.path();
    let mut part_path_buf = target_path.as_os_str().to_os_string();
    part_path_buf.push(".prismpart");
    let part_path = PathBuf::from(part_path_buf);

    let file_to_read = if target_path.exists() {
        target_path.clone()
    } else if part_path.exists() {
        part_path
    } else {
        let resp = "HTTP/1.1 404 File Not Found\r\nContent-Length: 0\r\n\r\n";
        socket.write_all(resp.as_bytes()).await?;
        return Ok(());
    };

    // Parse Range header if present: Range: bytes=X-Y or bytes=X-
    let mut range_start: u64 = 0;
    let mut range_end: Option<u64> = None;
    for line in lines {
        if line.to_ascii_lowercase().starts_with("range:") {
            if let Some(eq_idx) = line.find('=') {
                let range_val = line[eq_idx + 1..].trim();
                let r_parts: Vec<&str> = range_val.split('-').collect();
                if let Ok(s) = r_parts[0].trim().parse::<u64>() {
                    range_start = s;
                }
                if r_parts.len() > 1 && !r_parts[1].trim().is_empty() {
                    if let Ok(e) = r_parts[1].trim().parse::<u64>() {
                        range_end = Some(e);
                    }
                }
            }
            break;
        }
    }

    let total_size = item.size;
    let ext = target_path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "mkv" => "video/x-matroska",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "m4a" | "aac" => "audio/mp4",
        _ => "video/mp4",
    };

    // Open file
    let mut file = match tokio::fs::File::open(&file_to_read).await {
        Ok(f) => f,
        Err(_) => {
            let resp = "HTTP/1.1 500 Cannot Open File\r\nContent-Length: 0\r\n\r\n";
            socket.write_all(resp.as_bytes()).await?;
            return Ok(());
        }
    };

    if range_start > 0 {
        if file.seek(SeekFrom::Start(range_start)).await.is_err() {
            let resp = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n";
            socket.write_all(resp.as_bytes()).await?;
            return Ok(());
        }
    }

    let target_end = range_end.or(total_size.map(|s| s.saturating_sub(1)));
    let status_line = if range_start > 0 || range_end.is_some() {
        "HTTP/1.1 206 Partial Content"
    } else {
        "HTTP/1.1 200 OK"
    };

    let mut headers = format!(
        "{}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n",
        status_line, mime
    );

    if let Some(tot) = total_size {
        let eff_end = target_end.unwrap_or(tot.saturating_sub(1)).min(tot.saturating_sub(1));
        let content_len = eff_end.saturating_sub(range_start) + 1;
        headers.push_str(&format!(
            "Content-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\n",
            range_start, eff_end, tot, content_len
        ));
    }

    headers.push_str("\r\n");
    socket.write_all(headers.as_bytes()).await?;

    if method == "HEAD" {
        return Ok(());
    }

    // Progressive streaming loop:
    // Read and forward chunks to the player.
    // If we catch up with currently written data, sleep briefly to wait for incoming downloaded data!
    let mut chunk_buf = vec![0u8; 64 * 1024];
    let mut current_offset = range_start;
    let max_offset = target_end.unwrap_or(u64::MAX);

    while current_offset <= max_offset {
        let to_read = ((max_offset - current_offset + 1) as usize).min(chunk_buf.len());
        match file.read(&mut chunk_buf[..to_read]).await {
            Ok(0) => {
                // EOF or caught up with downloader. Check if file is still downloading!
                let is_finished = target_path.exists()
                    || core
                        .item(id)
                        .map_or(true, |it| it.downloaded == it.size.unwrap_or(0));
                if is_finished {
                    break;
                }
                // Wait 100ms for writer to flush more incoming downloaded bytes
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok(n) => {
                if socket.write_all(&chunk_buf[..n]).await.is_err() {
                    // Player disconnected or closed
                    break;
                }
                current_offset += n as u64;
            }
            Err(_) => break,
        }
    }

    Ok(())
}
