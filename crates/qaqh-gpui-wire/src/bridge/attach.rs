//! 附件与引用内容：一切字节都走内容寻址 blob（`upload_blob` / `fetch_blob`），
//! 桥只负责"落盘读取 → 判 MIME → 上传"，不解释业务语义。

use super::*;

/// 附件上限（与图片口径一致）。
const MAX_ATTACHMENT_BYTES: usize = 4 * 1024 * 1024;

/// 附件读取 → `upload_blob`（图片与文本同一路，零 base64 膨胀）。
pub(super) async fn prepare_attachment(
    client: &V3Client<YamuxTransport>,
    session_id: &SessionId,
    path: PathBuf,
) -> Result<PreparedAttachment, String> {
    let (bytes, mime) = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, String), String> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .map_err(|error| error.to_string())?
            .take(MAX_ATTACHMENT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err("附件最大 4 MiB".into());
        }
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            "image/gif"
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else if std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0) {
            "text/plain; charset=utf-8"
        } else {
            return Err("支持 PNG、JPEG、GIF、WebP 图片和 UTF-8 文本附件".into());
        };
        Ok((bytes, mime.to_string()))
    })
    .await
    .map_err(|error| error.to_string())??;
    let hash = client
        .upload_blob(session_id, &bytes)
        .await
        .map_err(|error| error.to_string())?;
    Ok(PreparedAttachment {
        hash,
        len: bytes.len() as u64,
        mime,
    })
}

/// 内容引用取全文（`Inline` 直接用，`Blob` 拉回后按 UTF-8 解）。
pub(super) async fn review_text(
    client: &V3Client<YamuxTransport>,
    reference: &ContentRef,
) -> Result<String, String> {
    match reference {
        ContentRef::Inline { text } => Ok(text.clone()),
        ContentRef::Windowed { preview, .. } => Ok(preview.clone()),
        ContentRef::Blob { hash, .. } => {
            let bytes = client
                .fetch_blob(hash)
                .await
                .map_err(|error| error.to_string())?;
            String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())
        }
    }
}
