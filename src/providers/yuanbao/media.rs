//! Media helpers — MIME mapping, byte-level image dimension parsing,
//! download with size cap, and TIM `msg_body` builders.
//!
//! Tencent COS upload lives in [`super::cos`] to keep both files under
//! the 500-line ceiling.

use super::errors::YuanbaoError;
use super::types::MsgBodyElement;

// ─── MIME / image-format mapping ───────────────────────────────────

pub fn guess_mime_type(filename: &str) -> &'static str {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "heic" => "image/heic",
        "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "txt" => "text/plain",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

pub fn is_image(filename: &str, mime_type: &str) -> bool {
    if mime_type.starts_with("image/") {
        return true;
    }
    guess_mime_type(filename).starts_with("image/")
}

/// Map a MIME type to the TIM `image_format` enum.
pub fn image_format_code(mime: &str) -> u32 {
    match mime {
        "image/jpeg" | "image/jpg" => 1,
        "image/gif" => 2,
        "image/png" => 3,
        "image/bmp" => 4,
        "image/webp" | "image/heic" | "image/tiff" => 255,
        _ => 255,
    }
}

// ─── Pure-bytes image dimension parsing ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDims {
    pub width: u32,
    pub height: u32,
}

pub fn parse_image_size(data: &[u8]) -> Option<ImageDims> {
    parse_png(data)
        .or_else(|| parse_jpeg(data))
        .or_else(|| parse_gif(data))
        .or_else(|| parse_webp(data))
}

fn parse_png(buf: &[u8]) -> Option<ImageDims> {
    if buf.len() < 24 || &buf[..4] != b"\x89PNG" {
        return None;
    }
    let w = u32::from_be_bytes(buf[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(buf[20..24].try_into().ok()?);
    Some(ImageDims {
        width: w,
        height: h,
    })
}

fn parse_jpeg(buf: &[u8]) -> Option<ImageDims> {
    if buf.len() < 4 || buf[0] != 0xFF || buf[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < buf.len() {
        if buf[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = buf[i + 1];
        if marker == 0xC0 || marker == 0xC2 {
            let h = u16::from_be_bytes(buf[i + 5..i + 7].try_into().ok()?);
            let w = u16::from_be_bytes(buf[i + 7..i + 9].try_into().ok()?);
            return Some(ImageDims {
                width: w as u32,
                height: h as u32,
            });
        }
        if i + 3 >= buf.len() {
            break;
        }
        let seg_len = u16::from_be_bytes(buf[i + 2..i + 4].try_into().ok()?) as usize;
        i += 2 + seg_len;
    }
    None
}

fn parse_gif(buf: &[u8]) -> Option<ImageDims> {
    if buf.len() < 10 {
        return None;
    }
    let sig = &buf[..6];
    if sig != b"GIF87a" && sig != b"GIF89a" {
        return None;
    }
    let w = u16::from_le_bytes(buf[6..8].try_into().ok()?);
    let h = u16::from_le_bytes(buf[8..10].try_into().ok()?);
    Some(ImageDims {
        width: w as u32,
        height: h as u32,
    })
}

fn parse_webp(buf: &[u8]) -> Option<ImageDims> {
    if buf.len() < 16 || &buf[..4] != b"RIFF" || &buf[8..12] != b"WEBP" {
        return None;
    }
    let chunk = &buf[12..16];
    if chunk == b"VP8 " {
        if buf.len() >= 30 && buf[23] == 0x9D && buf[24] == 0x01 && buf[25] == 0x2A {
            let w = u16::from_le_bytes(buf[26..28].try_into().ok()?) & 0x3FFF;
            let h = u16::from_le_bytes(buf[28..30].try_into().ok()?) & 0x3FFF;
            return Some(ImageDims {
                width: w as u32,
                height: h as u32,
            });
        }
    } else if chunk == b"VP8L" {
        if buf.len() >= 25 && buf[20] == 0x2F {
            let bits = u32::from_le_bytes(buf[21..25].try_into().ok()?);
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            return Some(ImageDims {
                width: w,
                height: h,
            });
        }
    } else if chunk == b"VP8X" && buf.len() >= 30 {
        let w = (buf[24] as u32 | ((buf[25] as u32) << 8) | ((buf[26] as u32) << 16)) + 1;
        let h = (buf[27] as u32 | ((buf[28] as u32) << 8) | ((buf[29] as u32) << 16)) + 1;
        return Some(ImageDims {
            width: w,
            height: h,
        });
    }
    None
}

// ─── HTTP download with size cap ────────────────────────────────────

pub async fn download_url(
    http: &reqwest::Client,
    url: &str,
    max_size_mb: u64,
) -> Result<(Vec<u8>, String), YuanbaoError> {
    let limit = max_size_mb.saturating_mul(1024 * 1024);

    if let Ok(head) = http.head(url).send().await
        && let Some(len) = head
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        && len > limit
    {
        return Err(YuanbaoError::Media(format!(
            "remote file too large: {len} > limit {limit}"
        )));
    }

    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| YuanbaoError::Connection(format!("download {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(YuanbaoError::Media(format!(
            "download HTTP {} for {url}",
            resp.status()
        )));
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| YuanbaoError::Media(format!("read body: {e}")))?;
    if bytes.len() as u64 > limit {
        return Err(YuanbaoError::Media(format!(
            "downloaded file exceeds limit: {} > {}",
            bytes.len(),
            limit
        )));
    }
    Ok((bytes.to_vec(), ct))
}

// ─── TIM msg_body builders ──────────────────────────────────────────

/// Build a TIM `TIMImageElem` `msg_body` ready to send.
pub fn build_image_msg_body(
    url: &str,
    uuid: Option<&str>,
    filename: Option<&str>,
    size: u32,
    width: u32,
    height: u32,
    mime_type: &str,
) -> Vec<MsgBodyElement> {
    use super::types::{ImageInfo, MsgContent};
    let uuid_str = uuid
        .map(|s| s.to_string())
        .or_else(|| filename.map(|s| s.to_string()))
        .unwrap_or_else(|| "image".to_string());
    let format = if mime_type.is_empty() {
        255
    } else {
        image_format_code(mime_type)
    };
    vec![MsgBodyElement {
        msg_type: "TIMImageElem".into(),
        msg_content: MsgContent {
            uuid: Some(uuid_str),
            image_format: Some(format),
            image_info_array: vec![ImageInfo {
                image_type: 1,
                size,
                width,
                height,
                url: url.to_string(),
            }],
            ..Default::default()
        },
    }]
}

/// Build a TIM `TIMFileElem` `msg_body` ready to send.
pub fn build_file_msg_body(
    url: &str,
    filename: &str,
    uuid: Option<&str>,
    size: u32,
) -> Vec<MsgBodyElement> {
    use super::types::MsgContent;
    let uuid_str = uuid
        .map(|s| s.to_string())
        .unwrap_or_else(|| filename.to_string());
    vec![MsgBodyElement {
        msg_type: "TIMFileElem".into(),
        msg_content: MsgContent {
            uuid: Some(uuid_str),
            file_name: Some(filename.to_string()),
            file_size: Some(size),
            url: Some(url.to_string()),
            ..Default::default()
        },
    }]
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
