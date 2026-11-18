//! 图片文件文档：磁盘身份、磁盘状态与解码内容的唯一所有者。
//!
//! 图片和文本一样是文件文档：`Project` 的文件监听按路径把磁盘变化交给这里，文档持有解码后的 `Arc<RenderImage>`，预览只消费文档句柄，不自行读盘或缓存字节。

use std::{collections::HashMap, path::Path, sync::Arc, time::SystemTime};

use gpui::{
    App, AppContext, Context, Entity, EntityId, EventEmitter, Image, ImageFormat, RenderImage,
    Task, WeakEntity,
};
use zcv_path::{AbsolutePathBuf, normalize_for_comparison};

use crate::translate_path;

/// 图片文件在磁盘上的可比较状态；
/// 只用于判断是否需要重载，不参与内容身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiskState {
    modified: Option<SystemTime>,
    size: u64,
}

impl DiskState {
    fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok(),
            size: metadata.len(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageDocumentEvent {
    /// 图片内容已从磁盘重新加载：初次加载完成或文件变化后重载。
    Reloaded,
}

/// 一张已打开的图片文档。
pub struct ImageDocument {
    path: AbsolutePathBuf,
    disk_state: Option<DiskState>,
    image: Option<Arc<RenderImage>>,
    error: Option<String>,
    load_generation: u64,
    /// 持有进行中的加载任务；替换或释放文档时随字段一并取消。
    load_task: Option<Task<()>>,
}

impl EventEmitter<ImageDocumentEvent> for ImageDocument {}

impl ImageDocument {
    fn new(path: AbsolutePathBuf, cx: &mut Context<Self>) -> Self {
        let disk_state = DiskState::read(path.as_path());
        let mut document = Self {
            path,
            disk_state,
            image: None,
            error: None,
            load_generation: 0,
            load_task: None,
        };
        document.start_load(cx);
        document
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// 从磁盘重新读取并解码图片内容；旧内容在重载期间保持可见，完成后整体替换。
    fn start_load(&mut self, cx: &mut Context<Self>) {
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        let path = self.path.as_path().to_path_buf();
        let renderer = cx.svg_renderer();
        let load = cx.background_spawn(async move {
            let bytes = std::fs::read(&path).map_err(|error| format!("读取图片失败：{error}"))?;
            let loaded = match image_format_for_path(&path) {
                Some(format) => Image::from_bytes(format, bytes).to_image_data(renderer),
                // 未识别的栅格扩展按 SVG 处理；渲染失败会显式成为错误状态。
                None => renderer
                    .render_single_frame(&bytes, 1.0)
                    .map_err(anyhow::Error::from),
            };
            loaded.map_err(|error| format!("解码图片失败：{error:#}"))
        });
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |document, cx| {
                if document.load_generation != generation {
                    return;
                }
                document.load_task = None;
                match result {
                    Ok(image) => {
                        document.image = Some(image);
                        document.error = None;
                    }
                    Err(error) => {
                        document.error = Some(error);
                    }
                }
                cx.emit(ImageDocumentEvent::Reloaded);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// 文件监听通知：只有磁盘状态变化才重载。
    pub(crate) fn file_changed_on_disk(&mut self, cx: &mut Context<Self>) {
        let state = DiskState::read(self.path.as_path());
        if state == self.disk_state {
            return;
        }
        self.disk_state = state;
        self.start_load(cx);
    }
}

/// 打开图片文件对应的扩展名与解码格式映射。
pub fn image_format_for_path(path: &Path) -> Option<ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let format = match extension.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::Webp,
        "gif" => ImageFormat::Gif,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        "pbm" | "pgm" | "ppm" | "pnm" => ImageFormat::Pnm,
        _ => return None,
    };
    Some(format)
}

/// 已打开图片文档的弱索引；文档实体由消费者持有，全部释放后自动结束。
pub(crate) struct ImageStore {
    opened_images: HashMap<AbsolutePathBuf, WeakEntity<ImageDocument>>,
}

impl ImageStore {
    pub(crate) fn new() -> Self {
        Self {
            opened_images: HashMap::new(),
        }
    }

    /// 打开图片文件；同一个规范化路径始终复用仍然存活的图片文档。
    pub(crate) fn open_image(
        &mut self,
        path: &Path,
        cx: &mut App,
    ) -> anyhow::Result<Entity<ImageDocument>> {
        let path = normalize_for_comparison(path)?;
        if let Some(document) = self.opened_images.get(&path).and_then(WeakEntity::upgrade) {
            return Ok(document);
        }
        let document = cx.new(|cx| ImageDocument::new(path.clone(), cx));
        self.opened_images.insert(path, document.downgrade());
        Ok(document)
    }

    /// 文件事件可能只报告目录或 Rescan 根；返回受该路径覆盖的存活图片文档。
    pub(crate) fn images_under_path(&self, path: &Path) -> Vec<Entity<ImageDocument>> {
        self.opened_images
            .iter()
            .filter(|(opened_path, _)| opened_path.as_path().starts_with(path))
            .filter_map(|(_, document)| document.upgrade())
            .collect()
    }

    pub(crate) fn release_image(&mut self, id: EntityId) {
        self.opened_images.retain(|_, opened| {
            opened
                .upgrade()
                .is_some_and(|document| document.entity_id() != id)
        });
    }

    /// 将已打开图片文档的路径索引随文件或目录重命名一起迁移。
    pub(crate) fn rename_path(&mut self, from: &Path, to: &Path) {
        self.opened_images = self
            .opened_images
            .drain()
            .filter_map(|(path, document)| {
                let path = AbsolutePathBuf::new(translate_path(path.as_path(), from, to)).ok()?;
                Some((path, document))
            })
            .collect();
    }
}

#[cfg(test)]
#[path = "test/image_store_tests.rs"]
mod tests;
