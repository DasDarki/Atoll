use ash::vk;
use cef::AcceleratedPaintInfo;
use cef::sys::cef_color_type_t;
use wgpu::hal::api::Vulkan;

use super::surface::SharedTextureError;

/// Wraps the DMA-BUF of an accelerated paint into a `wgpu` texture.
///
/// The returned texture owns the Vulkan image and the imported memory, both are released when it
/// is dropped. CEF closes its file descriptor after the paint callback, so the descriptor is
/// duplicated and handed over to the driver.
pub(crate) fn import(
    device: &wgpu::Device,
    info: &AcceleratedPaintInfo,
) -> Result<wgpu::Texture, SharedTextureError> {
    if info.plane_count != 1 {
        return Err(SharedTextureError::Unsupported(format!(
            "{} planes",
            info.plane_count
        )));
    }
    let (vulkan_format, format) = match *info.format.as_ref() {
        cef_color_type_t::CEF_COLOR_TYPE_BGRA_8888 => {
            (vk::Format::B8G8R8A8_UNORM, wgpu::TextureFormat::Bgra8Unorm)
        }
        cef_color_type_t::CEF_COLOR_TYPE_RGBA_8888 => {
            (vk::Format::R8G8B8A8_UNORM, wgpu::TextureFormat::Rgba8Unorm)
        }
        other => return Err(SharedTextureError::Unsupported(format!("{other:?}"))),
    };
    let size = wgpu::Extent3d {
        width: u32::try_from(info.extra.coded_size.width).unwrap_or(0),
        height: u32::try_from(info.extra.coded_size.height).unwrap_or(0),
        depth_or_array_layers: 1,
    };
    if size.width == 0 || size.height == 0 {
        return Err(SharedTextureError::Unsupported("empty frame".into()));
    }

    // SAFETY: the Vulkan objects are created on wgpu's own device and handed over to wgpu, which
    // destroys the image and frees the dedicated memory once the texture is no longer in use.
    let hal_texture = unsafe {
        let hal_device = device
            .as_hal::<Vulkan>()
            .ok_or_else(|| SharedTextureError::Unsupported("device is not Vulkan".into()))?;
        let (image, memory) = create_image(&hal_device, info, vulkan_format, size)?;
        hal_device.texture_from_raw(
            image,
            &wgpu::hal::TextureDescriptor {
                label: Some("page frame (dmabuf)"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUses::RESOURCE,
                memory_flags: wgpu::hal::MemoryFlags::empty(),
                view_formats: Vec::new(),
            },
            None,
            wgpu::hal::vulkan::TextureMemory::Dedicated(memory),
        )
    };

    // SAFETY: the texture was created from this device, matches the descriptor and its content is
    // fully written by the producer before the paint callback runs.
    let texture = unsafe {
        device.create_texture_from_hal::<Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("page frame (dmabuf)"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::TextureUses::UNINITIALIZED,
        )
    };
    Ok(texture)
}

unsafe fn create_image(
    hal_device: &<Vulkan as wgpu::hal::Api>::Device,
    info: &AcceleratedPaintInfo,
    format: vk::Format,
    size: wgpu::Extent3d,
) -> Result<(vk::Image, vk::DeviceMemory), SharedTextureError> {
    let device = hal_device.raw_device();
    let instance = hal_device.shared_instance().raw_instance();
    let plane = &info.planes[0];

    let plane_layouts = [vk::SubresourceLayout {
        offset: plane.offset,
        size: 0,
        row_pitch: u64::from(plane.stride),
        array_pitch: 0,
        depth_pitch: 0,
    }];
    let mut modifier_info = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(info.modifier)
        .plane_layouts(&plane_layouts);
    let mut external_info = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: size.width,
            height: size.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external_info)
        .push_next(&mut modifier_info);

    // SAFETY: plain Vulkan calls on a valid device. Every early return releases what was created
    // before, and ownership of the duplicated descriptor passes to the driver on success.
    unsafe {
        let image = device
            .create_image(&image_info, None)
            .map_err(|error| SharedTextureError::Vulkan(format!("create image: {error:?}")))?;

        let descriptor = libc::dup(plane.fd);
        if descriptor < 0 {
            device.destroy_image(image, None);
            return Err(SharedTextureError::Vulkan("duplicate descriptor".into()));
        }
        let release = |message: String| {
            libc::close(descriptor);
            device.destroy_image(image, None);
            SharedTextureError::Vulkan(message)
        };

        let fd_device = ash::khr::external_memory_fd::Device::new(instance, device);
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        if let Err(error) = fd_device.get_memory_fd_properties(
            vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
            descriptor,
            &mut fd_properties,
        ) {
            return Err(release(format!("descriptor properties: {error:?}")));
        }

        let requirements = device.get_image_memory_requirements(image);
        let memory_types = requirements.memory_type_bits & fd_properties.memory_type_bits;
        let memory_properties =
            instance.get_physical_device_memory_properties(hal_device.raw_physical_device());
        let Some(memory_type) =
            (0..memory_properties.memory_type_count).find(|index| memory_types & (1 << index) != 0)
        else {
            return Err(release("no compatible memory type".into()));
        };

        let mut import_info = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(descriptor);
        let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut import_info)
            .push_next(&mut dedicated_info);
        let memory = match device.allocate_memory(&allocate_info, None) {
            Ok(memory) => memory,
            Err(error) => return Err(release(format!("import memory: {error:?}"))),
        };

        if let Err(error) = device.bind_image_memory(image, memory, 0) {
            device.free_memory(memory, None);
            device.destroy_image(image, None);
            return Err(SharedTextureError::Vulkan(format!(
                "bind memory: {error:?}"
            )));
        }
        Ok((image, memory))
    }
}
