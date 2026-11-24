//! Bounded native previews for canonical multimodal image blocks.
use gpui_kit::{AnyElement, Image, ImageFormat, div, img, prelude::*, px};
use qaqh_client::ImageBlock;
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};
thread_local! { static IMAGES: RefCell<HashMap<u64, Arc<Image>>> = RefCell::default(); }

fn cached_image(block: &ImageBlock) -> Result<Arc<Image>, String> {
    IMAGES.with(|images| {
        let mut key = std::hash::DefaultHasher::new();
        block.mime_type.hash(&mut key);
        block.data.hash(&mut key);
        let key = key.finish();
        let mut images = images.borrow_mut();
        if let Some(image) = images.get(&key) {
            return Ok(image.clone());
        }
        let format = match block.mime_type.as_str() {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            _ => return Err("图片格式暂不支持".into()),
        };
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&block.data)
            .map_err(|e| e.to_string())?;
        let image = Arc::new(Image::from_bytes(format, bytes));
        if images.len() >= 32 {
            images.clear();
        }
        images.insert(key, image.clone());
        Ok(image)
    })
}

pub fn preview(block: &ImageBlock) -> AnyElement {
    let result = cached_image(block);
    match result {
        Ok(image) => img(image).w(px(160.)).h(px(100.)).into_any_element(),
        Err(error) => div().text_xs().child(error).into_any_element(),
    }
}

/// composer 附件缩略:52px 方图裁切;移除交互由调用方包层挂 on_click。
pub fn thumb(block: &ImageBlock) -> AnyElement {
    match cached_image(block) {
        Ok(image) => img(image)
            .w(px(52.))
            .h(px(52.))
            .rounded(px(8.))
            .object_fit(gpui_kit::ObjectFit::Cover)
            .into_any_element(),
        Err(error) => div().text_xs().child(error).into_any_element(),
    }
}
