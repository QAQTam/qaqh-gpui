//! Bounded native previews for canonical multimodal image blocks.
use gpui::{AnyElement, Image, ImageFormat, div, img, prelude::*, px};
use qaqh_client::ImageBlock;
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};
thread_local! { static IMAGES: RefCell<HashMap<u64, Arc<Image>>> = RefCell::default(); }

pub fn preview(block: &ImageBlock) -> AnyElement {
    let result = IMAGES.with(|images| -> Result<Arc<Image>, String> {
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
    });
    match result {
        Ok(image) => img(image).w(px(160.)).h(px(100.)).into_any_element(),
        Err(error) => div().text_xs().child(error).into_any_element(),
    }
}
