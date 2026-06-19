use thiserror::Error;

use super::blit::Blitter;

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no suitable graphics adapter was found: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("the graphics device could not be created: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
}

/// The `wgpu` device shared by the user interface and the engine.
///
/// Page frames are copied into textures of this device, so the user interface must render with
/// the very same instance, adapter, device and queue.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub(crate) blitter: Blitter,
}

impl GpuContext {
    pub fn create() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: preferred_backends(),
            flags: wgpu::InstanceFlags::empty().with_env(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))?;

        let optional_features = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("atoll"),
                required_features: adapter.features() & optional_features,
                required_limits: adapter.limits(),
                ..Default::default()
            }))?;

        let info = adapter.get_info();
        tracing::info!(adapter = %info.name, backend = ?info.backend, "graphics device ready");

        let blitter = Blitter::new(&device);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            blitter,
        })
    }

    /// Whether page frames can be received as shared GPU textures on this device.
    pub fn supports_shared_textures(&self) -> bool {
        cfg!(target_os = "linux")
            && self
                .device
                .features()
                .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
    }

    pub(crate) fn create_texture(
        &self,
        label: &str,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }
}

fn preferred_backends() -> wgpu::Backends {
    if let Some(backends) = wgpu::Backends::from_env() {
        return backends;
    }
    if cfg!(target_os = "linux") {
        wgpu::Backends::VULKAN
    } else if cfg!(target_os = "windows") {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::PRIMARY
    }
}

/// Hides Vulkan drivers from the loader that cannot work on this machine.
///
/// A registered NVIDIA driver without the NVIDIA kernel module still gets loaded when the
/// Vulkan instance is created, which delays the first frame by about half a second. Nothing
/// changes if the user configured `VK_DRIVER_FILES` or `VK_ICD_FILENAMES` explicitly.
///
/// # Safety
///
/// Modifies the process environment, so it must be called before any other thread is started.
#[cfg(target_os = "linux")]
pub unsafe fn prune_unusable_vulkan_drivers() {
    use std::path::{Path, PathBuf};

    if std::env::var_os("VK_DRIVER_FILES").is_some()
        || std::env::var_os("VK_ICD_FILENAMES").is_some()
        || Path::new("/sys/module/nvidia").exists()
    {
        return;
    }

    let mut directories = vec![
        PathBuf::from("/etc/vulkan/icd.d"),
        PathBuf::from("/usr/local/share/vulkan/icd.d"),
        PathBuf::from("/usr/share/vulkan/icd.d"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share/vulkan/icd.d"));
    }
    let manifests: Vec<PathBuf> = directories
        .iter()
        .filter_map(|directory| std::fs::read_dir(directory).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    let (unusable, usable): (Vec<PathBuf>, Vec<PathBuf>) = manifests
        .into_iter()
        .partition(|path| path.to_string_lossy().contains("nvidia"));
    if unusable.is_empty() || usable.is_empty() {
        return;
    }
    let drivers = std::env::join_paths(&usable).unwrap_or_default();
    // SAFETY: the caller guarantees that no other thread exists yet.
    unsafe { std::env::set_var("VK_DRIVER_FILES", drivers) };
}

/// See the Linux version, there is nothing to prune on other platforms.
///
/// # Safety
///
/// Always safe to call on this platform.
#[cfg(not(target_os = "linux"))]
pub unsafe fn prune_unusable_vulkan_drivers() {}
