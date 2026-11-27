//! 内容寻址图片的本地预览(V3:原始字节经 `BlobTransfer` 传输,禁止 base64 内联)。
//!
//! 渲染层不发起网络/进程内请求:字节由调用方从 `SessionStore::blobs` 取好后传入,
//! 未到货时给「内容加载中」占位;`BlobCache` 到货会触发重渲染。
use gpui_kit::{AnyElement, Image, ImageFormat, div, img, prelude::*, px};
use qaqh_core_vocab::ContentHash;
use qaqh_ringing_v3::ImageRefView;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::Arc,
};

thread_local! { static IMAGES: RefCell<HashMap<ContentHash, Arc<Image>>> = RefCell::default(); }

fn image_format(mime: &str) -> Option<ImageFormat> {
    match mime {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::Webp),
        _ => None,
    }
}

fn cached_image(reference: &ImageRefView, bytes: Option<Arc<[u8]>>) -> Result<Arc<Image>, String> {
    let Some(bytes) = bytes else {
        return Err("内容加载中…".into());
    };
    let format = image_format(&reference.mime_type).ok_or("图片格式暂不支持")?;
    IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        if let Some(image) = images.get(&reference.hash) {
            return Ok(image.clone());
        }
        let image = Arc::new(Image::from_bytes(format, bytes.to_vec()));
        if images.len() >= 32 {
            images.clear();
        }
        images.insert(reference.hash, image.clone());
        Ok(image)
    })
}

pub fn preview(reference: &ImageRefView, bytes: Option<Arc<[u8]>>) -> AnyElement {
    match cached_image(reference, bytes) {
        Ok(image) => img(image).w(px(160.)).h(px(100.)).into_any_element(),
        Err(error) => div().text_xs().child(error).into_any_element(),
    }
}

/// composer 附件缩略:52px 方图裁切;移除交互由调用方包层挂 on_click。
pub fn thumb(reference: &ImageRefView, bytes: Option<Arc<[u8]>>) -> AnyElement {
    match cached_image(reference, bytes) {
        Ok(image) => img(image)
            .w(px(52.))
            .h(px(52.))
            .rounded(px(8.))
            .object_fit(gpui_kit::ObjectFit::Cover)
            .into_any_element(),
        Err(error) => div().text_xs().child(error).into_any_element(),
    }
}
