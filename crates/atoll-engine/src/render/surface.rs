use std::cell::{Cell, RefCell};

use cef::{AcceleratedPaintInfo, Rect};
use thiserror::Error;

use super::blit::Blitter;
use super::gpu::GpuContext;

#[derive(Debug, Error)]
pub(crate) enum SharedTextureError {
    #[error("unsupported shared texture: {0}")]
    Unsupported(String),
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    #[error("Vulkan import failed: {0}")]
    Vulkan(String),
}

/// The latest frame of a view or of its popup widget.
///
/// The texture is reused for consecutive frames of the same size. `generation` changes whenever a
/// new texture was allocated, so consumers only need to re-wrap the texture in that case.
#[derive(Debug, Clone)]
pub struct SurfaceFrame {
    pub texture: wgpu::Texture,
    pub generation: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceLayer {
    Page,
    Popup,
}

#[derive(Default)]
struct Layer {
    target: Option<wgpu::Texture>,
    upload: Option<wgpu::Texture>,
    generation: u64,
}

/// Receives the frames of one browser and keeps them as `Rgba8Unorm` textures.
#[derive(Default)]
pub(crate) struct ViewSurface {
    page: RefCell<Layer>,
    popup: RefCell<Layer>,
    next_generation: Cell<u64>,
}

impl ViewSurface {
    pub(crate) fn frame(&self, layer: SurfaceLayer) -> Option<SurfaceFrame> {
        let layer = self.layer(layer).borrow();
        layer.target.as_ref().map(|texture| SurfaceFrame {
            texture: texture.clone(),
            generation: layer.generation,
        })
    }

    pub(crate) fn clear(&self, layer: SurfaceLayer) {
        *self.layer(layer).borrow_mut() = Layer::default();
    }

    /// Copies a shared GPU texture handed out by an accelerated paint.
    pub(crate) fn present_shared(
        &self,
        gpu: &GpuContext,
        layer: SurfaceLayer,
        info: &AcceleratedPaintInfo,
    ) -> Result<(), SharedTextureError> {
        let source = import_shared(gpu, info)?;
        let target = self.target(gpu, layer, source.size());
        gpu.blitter.blit(&gpu.device, &gpu.queue, &source, &target);
        Ok(())
    }

    /// Uploads the dirty regions of a BGRA buffer handed out by a software paint.
    pub(crate) fn present_buffer(
        &self,
        gpu: &GpuContext,
        layer: SurfaceLayer,
        buffer: &[u8],
        width: u32,
        height: u32,
        dirty_rects: &[Rect],
    ) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let (upload, fresh) = self.upload(gpu, layer, size);
        let full_frame = [Rect {
            x: 0,
            y: 0,
            width: width as i32,
            height: height as i32,
        }];
        let regions = if fresh || dirty_rects.is_empty() {
            &full_frame[..]
        } else {
            dirty_rects
        };
        for region in regions {
            write_region(gpu, &upload, buffer, width, height, region);
        }
        let target = self.target(gpu, layer, size);
        gpu.blitter.blit(&gpu.device, &gpu.queue, &upload, &target);
    }

    fn layer(&self, layer: SurfaceLayer) -> &RefCell<Layer> {
        match layer {
            SurfaceLayer::Page => &self.page,
            SurfaceLayer::Popup => &self.popup,
        }
    }

    fn target(&self, gpu: &GpuContext, layer: SurfaceLayer, size: wgpu::Extent3d) -> wgpu::Texture {
        let mut layer = self.layer(layer).borrow_mut();
        if let Some(target) = layer.target.as_ref().filter(|target| target.size() == size) {
            return target.clone();
        }
        let target = gpu.create_texture(
            "page frame",
            size,
            Blitter::TARGET_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let generation = self.next_generation.get() + 1;
        self.next_generation.set(generation);
        layer.generation = generation;
        layer.target = Some(target.clone());
        target
    }

    fn upload(
        &self,
        gpu: &GpuContext,
        layer: SurfaceLayer,
        size: wgpu::Extent3d,
    ) -> (wgpu::Texture, bool) {
        let mut layer = self.layer(layer).borrow_mut();
        if let Some(upload) = layer.upload.as_ref().filter(|upload| upload.size() == size) {
            return (upload.clone(), false);
        }
        let upload = gpu.create_texture(
            "page upload",
            size,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        layer.upload = Some(upload.clone());
        (upload, true)
    }
}

fn write_region(
    gpu: &GpuContext,
    upload: &wgpu::Texture,
    buffer: &[u8],
    width: u32,
    height: u32,
    region: &Rect,
) {
    let x = region.x.clamp(0, width as i32) as u32;
    let y = region.y.clamp(0, height as i32) as u32;
    let region_width = (region.width.max(0) as u32).min(width - x);
    let region_height = (region.height.max(0) as u32).min(height - y);
    if region_width == 0 || region_height == 0 {
        return;
    }
    let row_bytes = width as usize * 4;
    let offset = y as usize * row_bytes + x as usize * 4;
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: upload,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        &buffer[offset..],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(region_height),
        },
        wgpu::Extent3d {
            width: region_width,
            height: region_height,
            depth_or_array_layers: 1,
        },
    );
}

#[cfg(target_os = "linux")]
fn import_shared(
    gpu: &GpuContext,
    info: &AcceleratedPaintInfo,
) -> Result<wgpu::Texture, SharedTextureError> {
    super::dmabuf::import(&gpu.device, info)
}

#[cfg(not(target_os = "linux"))]
fn import_shared(
    _gpu: &GpuContext,
    _info: &AcceleratedPaintInfo,
) -> Result<wgpu::Texture, SharedTextureError> {
    Err(SharedTextureError::Unsupported(
        "shared textures are only supported on Linux".into(),
    ))
}
