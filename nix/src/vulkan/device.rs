use std::{
    ffi, fmt,
    sync::{Arc, Weak},
};

use ash::vk::{self, Handle};
use gpu_alloc::MemoryBlock;
use gpu_alloc_ash::AshMemoryDevice;
use hashbrown::HashMap;
use parking_lot::Mutex;
use raw_window_handle::{
    HasRawDisplayHandle, HasRawWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use slab::Slab;

use crate::generic::{
    compile_shader, ArgumentKind, BufferDesc, CreateLibraryError, CreatePipelineError, Features,
    ImageDesc, ImageError, LibraryDesc, LibraryInput, Memory, OutOfMemory, PrimitiveTopology,
    RenderPipelineDesc, SamplerDesc, ShaderLanguage, SurfaceError, VertexStepMode,
};

use super::{
    buffer::Buffer,
    from::{IntoAsh, TryIntoAsh},
    handle_host_oom,
    image::Image,
    layout::{
        DescriptorSetLayout, DescriptorSetLayoutDesc, PipelineLayout, PipelineLayoutDesc,
        WeakDescriptorSetLayout, WeakPipelineLayout,
    },
    queue::{Family, Queue},
    render_pipeline::RenderPipeline,
    sampler::WeakSampler,
    shader::Library,
    surface::{Surface, SurfaceErrorKind},
    unexpected_error, Sampler, Version,
};

struct DeviceInner {
    device: ash::Device,
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    version: Version,
    families: Vec<Family>,
    features: Features,
    properties: ash::vk::PhysicalDeviceProperties,

    buffers: Mutex<Slab<vk::Buffer>>,
    images: Mutex<Slab<vk::Image>>,
    samplers: Mutex<HashMap<SamplerDesc, WeakSampler>>,

    set_layouts: Mutex<HashMap<DescriptorSetLayoutDesc, WeakDescriptorSetLayout>>,
    pipeline_layouts: Mutex<HashMap<PipelineLayoutDesc, WeakPipelineLayout>>,
    pipelines: Mutex<Slab<vk::Pipeline>>,

    allocator: Mutex<gpu_alloc::GpuAllocator<vk::DeviceMemory>>,

    _entry: ash::Entry,

    #[cfg(any(debug_assertions, feature = "debug"))]
    debug_utils: Option<ash::extensions::ext::DebugUtils>,

    surface: Option<ash::extensions::khr::Surface>,
    swapchain: Option<ash::extensions::khr::Swapchain>,

    #[cfg(target_os = "windows")]
    win32_surface: Option<ash::extensions::khr::Win32Surface>,
}

#[derive(Clone)]
pub struct Device {
    inner: Arc<DeviceInner>,
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Device({:p}@{:p})",
            self.inner.device.handle(),
            self.inner.instance.handle()
        )
    }
}

#[derive(Clone)]
pub(super) struct WeakDevice {
    inner: std::sync::Weak<DeviceInner>,
}

impl WeakDevice {
    #[inline(always)]
    pub fn upgrade(&self) -> Option<Device> {
        self.inner.upgrade().map(|inner| Device { inner })
    }

    #[inline]
    pub fn drop_buffer(&self, idx: usize, block: MemoryBlock<vk::DeviceMemory>) {
        if let Some(inner) = self.inner.upgrade() {
            unsafe {
                inner
                    .allocator
                    .lock()
                    .dealloc(AshMemoryDevice::wrap(&inner.device), block)
            }

            let mut buffers = inner.buffers.lock();
            let buffer = buffers.remove(idx);
            unsafe {
                inner.device.destroy_buffer(buffer, None);
            }
        }
    }

    #[inline]
    pub fn drop_image(&self, idx: usize, block: MemoryBlock<vk::DeviceMemory>) {
        if let Some(inner) = self.inner.upgrade() {
            unsafe {
                inner
                    .allocator
                    .lock()
                    .dealloc(AshMemoryDevice::wrap(&inner.device), block)
            }

            let mut images = inner.images.lock();
            let image = images.remove(idx);
            unsafe {
                inner.device.destroy_image(image, None);
            }
        }
    }

    #[inline]
    pub fn drop_sampler(&self, desc: SamplerDesc) {
        if let Some(inner) = self.inner.upgrade() {
            let mut samplers = inner.samplers.lock();
            match samplers.entry(desc) {
                hashbrown::hash_map::Entry::Occupied(entry) => {
                    let weak = entry.get();
                    // It is only safe to drop when no strong refs exist.
                    // While this function is called when last strong reference is dropped
                    // the entry could be replaced by new sampler before lock was acquired.
                    if weak.unused() {
                        // No strong references exists.
                        unsafe {
                            inner.device.destroy_sampler(weak.handle(), None);
                        }
                    }
                }
                _ => {
                    // Entry was removed, probably in `new_sampler` call with the same `SamplerDesc`.
                }
            }
        }
    }

    #[inline]
    pub fn drop_descriptor_set_layout(&self, desc: DescriptorSetLayoutDesc) {
        if let Some(inner) = self.inner.upgrade() {
            let mut samplers = inner.set_layouts.lock();
            match samplers.entry(desc) {
                hashbrown::hash_map::Entry::Occupied(entry) => {
                    let weak = entry.get();
                    // It is only safe to drop when no strong refs exist.
                    // While this function is called when last strong reference is dropped
                    // the entry could be replaced by new layout before lock was acquired.
                    if weak.unused() {
                        // No strong references exists.
                        unsafe {
                            inner
                                .device
                                .destroy_descriptor_set_layout(weak.handle(), None);
                        }
                    }
                }
                _ => {
                    // Entry was removed, probably in `new_sampler` call with the same `SamplerDesc`.
                }
            }
        }
    }

    #[inline]
    pub fn drop_pipeline_layout(&self, desc: PipelineLayoutDesc) {
        if let Some(inner) = self.inner.upgrade() {
            let mut pipeline_layouts = inner.pipeline_layouts.lock();
            match pipeline_layouts.entry(desc) {
                hashbrown::hash_map::Entry::Occupied(entry) => {
                    let weak = entry.get();
                    // It is only safe to drop when no strong refs exist.
                    // While this function is called when last strong reference is dropped
                    // the entry could be replaced by new layout before lock was acquired.
                    if weak.unused() {
                        // No strong references exists.
                        unsafe {
                            inner.device.destroy_pipeline_layout(weak.handle(), None);
                        }
                    }
                }
                _ => {
                    // Entry was removed, probably in `new_sampler` call with the same `SamplerDesc`.
                }
            }
        }
    }

    #[inline]
    pub fn drop_pipeline(&self, idx: usize) {
        if let Some(inner) = self.inner.upgrade() {
            let pipeline = inner.pipelines.lock().remove(idx);
            unsafe {
                inner.device.destroy_pipeline(pipeline, None);
            }
        }
    }
}

pub(super) trait DeviceOwned {
    fn owner(&self) -> &WeakDevice;
}

impl Device {
    pub(super) fn new(
        version: Version,
        entry: ash::Entry,
        instance: ash::Instance,
        physical_device: vk::PhysicalDevice,
        device: ash::Device,
        families: Vec<Family>,
        features: Features,
        properties: ash::vk::PhysicalDeviceProperties,
        allocator: gpu_alloc::GpuAllocator<vk::DeviceMemory>,
        #[cfg(any(debug_assertions, feature = "debug"))] debug_utils: Option<
            ash::extensions::ext::DebugUtils,
        >,
        surface: Option<ash::extensions::khr::Surface>,
        swapchain: Option<ash::extensions::khr::Swapchain>,
        #[cfg(target_os = "windows")] win32_surface: Option<ash::extensions::khr::Win32Surface>,
    ) -> Self {
        Device {
            inner: Arc::new(DeviceInner {
                device,
                instance,
                physical_device,
                version,
                families,
                features,
                properties,
                buffers: Mutex::new(Slab::new()),
                images: Mutex::new(Slab::new()),
                samplers: Mutex::new(HashMap::new()),
                set_layouts: Mutex::new(HashMap::new()),
                pipeline_layouts: Mutex::new(HashMap::new()),
                pipelines: Mutex::new(Slab::new()),
                allocator: Mutex::new(allocator),
                debug_utils,
                surface,
                swapchain,
                win32_surface,
                _entry: entry,
            }),
        }
    }

    pub(super) fn ash(&self) -> &ash::Device {
        &self.inner.device
    }

    pub(super) fn is(&self, weak: &WeakDevice) -> bool {
        Arc::as_ptr(&self.inner) == Weak::as_ptr(&weak.inner)
    }

    pub(super) fn is_owner(&self, owned: &impl DeviceOwned) -> bool {
        self.is(owned.owner())
    }

    pub(super) fn weak(&self) -> WeakDevice {
        WeakDevice {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub(super) fn swapchain(&self) -> &ash::extensions::khr::Swapchain {
        self.inner.swapchain.as_ref().unwrap()
    }

    pub(super) fn surface(&self) -> &ash::extensions::khr::Surface {
        self.inner.surface.as_ref().unwrap()
    }

    pub(super) fn physical_device(&self) -> vk::PhysicalDevice {
        self.inner.physical_device
    }

    pub(super) fn queue_families(&self) -> impl Iterator<Item = u32> + '_ {
        self.inner.families.iter().map(|f| f.index)
    }

    #[cfg(any(debug_assertions, feature = "debug"))]
    fn set_object_name(&self, ty: vk::ObjectType, handle: u64, name: &str) {
        if !name.is_empty() {
            if let Some(debug_utils) = &self.inner.debug_utils {
                let name_cstr = ffi::CString::new(name).unwrap();
                let _ = unsafe {
                    debug_utils.set_debug_utils_object_name(
                        self.inner.device.handle(),
                        &vk::DebugUtilsObjectNameInfoEXT::builder()
                            .object_type(ty)
                            .object_handle(handle)
                            .object_name(&name_cstr),
                    )
                };
            }
        }
    }

    fn new_sampler_slow(&self, count: usize, desc: SamplerDesc) -> Result<Sampler, OutOfMemory> {
        if self.inner.properties.limits.max_sampler_allocation_count as usize >= count {
            return Err(OutOfMemory);
        }

        let result = unsafe {
            self.ash().create_sampler(
                &ash::vk::SamplerCreateInfo::builder()
                    .min_filter(desc.min_filter.into_ash())
                    .mag_filter(desc.mag_filter.into_ash())
                    .mipmap_mode(desc.mip_map_mode.into_ash())
                    .address_mode_u(desc.address_mode[0].into_ash())
                    .address_mode_v(desc.address_mode[1].into_ash())
                    .address_mode_w(desc.address_mode[2].into_ash())
                    .anisotropy_enable(desc.anisotropy.is_some())
                    .max_anisotropy(desc.anisotropy.unwrap_or(0.0))
                    .unnormalized_coordinates(!desc.normalized),
                None,
            )
        };

        let handle = result.map_err(|err| match err {
            ash::vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
            ash::vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => OutOfMemory,
            _ => unexpected_error(err),
        })?;

        Ok(Sampler::new(self.weak(), handle, desc))
    }

    fn new_set_layout_slow(
        &self,
        desc: DescriptorSetLayoutDesc,
    ) -> Result<DescriptorSetLayout, OutOfMemory> {
        let bindings = desc
            .arguments
            .iter()
            .enumerate()
            .map(|(idx, arg)| {
                ash::vk::DescriptorSetLayoutBinding::builder()
                    .binding(idx as u32)
                    .descriptor_count(arg.size)
                    .descriptor_type(match arg.kind {
                        ArgumentKind::Constant => ash::vk::DescriptorType::INLINE_UNIFORM_BLOCK,
                        ArgumentKind::UniformBuffer => ash::vk::DescriptorType::UNIFORM_BUFFER,
                        ArgumentKind::StorageBuffer => ash::vk::DescriptorType::STORAGE_BUFFER,
                        ArgumentKind::SampledImage => ash::vk::DescriptorType::SAMPLED_IMAGE,
                        ArgumentKind::StorageImage => ash::vk::DescriptorType::STORAGE_IMAGE,
                        ArgumentKind::Sampler => ash::vk::DescriptorType::SAMPLER,
                    })
                    .stage_flags(arg.stages.into_ash())
                    .build()
            })
            .collect::<Vec<_>>();

        let result = unsafe {
            self.ash().create_descriptor_set_layout(
                &ash::vk::DescriptorSetLayoutCreateInfo::builder()
                    .flags(ash::vk::DescriptorSetLayoutCreateFlags::PUSH_DESCRIPTOR_KHR)
                    .bindings(&bindings),
                None,
            )
        };

        let handle = result.map_err(|err| match err {
            ash::vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
            ash::vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => OutOfMemory,
            _ => unexpected_error(err),
        })?;
        Ok(DescriptorSetLayout::new(self.weak(), handle, desc))
    }

    fn new_set_layout(
        &self,
        desc: DescriptorSetLayoutDesc,
    ) -> Result<DescriptorSetLayout, OutOfMemory> {
        let mut set_layouts = self.inner.set_layouts.lock();

        match set_layouts.entry(desc) {
            hashbrown::hash_map::Entry::Occupied(entry) => match entry.get().upgrade() {
                Some(set_layout) => Ok(set_layout.clone()),
                None => {
                    let set_layout = self.new_set_layout_slow(entry.key().clone())?;
                    entry.replace_entry(set_layout.downgrade());
                    Ok(set_layout)
                }
            },
            hashbrown::hash_map::Entry::Vacant(entry) => {
                let set_layout = self.new_set_layout_slow(entry.key().clone())?;
                entry.insert(set_layout.downgrade());
                Ok(set_layout)
            }
        }
    }

    fn new_pipeline_layout_slow(
        &self,
        desc: PipelineLayoutDesc,
    ) -> Result<PipelineLayout, OutOfMemory> {
        let set_layouts = desc
            .groups
            .iter()
            .map(|group| {
                self.new_set_layout(DescriptorSetLayoutDesc {
                    arguments: group.clone(),
                })
            })
            .collect::<Result<Vec<_>, OutOfMemory>>()?;

        let handles = set_layouts
            .iter()
            .map(|set_layout| set_layout.handle())
            .collect::<Vec<_>>();

        let result = unsafe {
            self.ash().create_pipeline_layout(
                &ash::vk::PipelineLayoutCreateInfo::builder().set_layouts(&handles),
                None,
            )
        };
        let handle = result.map_err(|err| match err {
            ash::vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
            ash::vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => OutOfMemory,
            _ => unexpected_error(err),
        })?;
        Ok(PipelineLayout::new(self.weak(), handle, desc))
    }

    fn new_pipeline_layout(&self, desc: PipelineLayoutDesc) -> Result<PipelineLayout, OutOfMemory> {
        let mut pipeline_layouts = self.inner.pipeline_layouts.lock();

        match pipeline_layouts.entry(desc) {
            hashbrown::hash_map::Entry::Occupied(entry) => match entry.get().upgrade() {
                Some(pipeline_layout) => Ok(pipeline_layout.clone()),
                None => {
                    let pipeline_layout = self.new_pipeline_layout_slow(entry.key().clone())?;
                    entry.replace_entry(pipeline_layout.downgrade());
                    Ok(pipeline_layout)
                }
            },
            hashbrown::hash_map::Entry::Vacant(entry) => {
                let pipeline_layout = self.new_pipeline_layout_slow(entry.key().clone())?;
                entry.insert(pipeline_layout.downgrade());
                Ok(pipeline_layout)
            }
        }
    }
}

#[hidden_trait::expose]
impl crate::traits::Device for Device {
    fn get_queue(&self, family: usize, idx: usize) -> Queue {
        let family = &self.inner.families[family];
        let handle = family.queues[idx];
        Queue::new(self.clone(), handle, family.flags)
    }

    fn new_shader_library(&self, desc: LibraryDesc) -> Result<Library, CreateLibraryError> {
        let me = &*self.inner;
        match desc.input {
            LibraryInput::Source(source) => {
                let compiled: Box<[u32]>;
                let code = match source.language {
                    ShaderLanguage::SpirV => unsafe {
                        let (left, words, right) = source.code.align_to::<u32>();

                        if left.is_empty() && right.is_empty() {
                            words
                        } else {
                            let mut code = &*source.code;
                            let mut words = Vec::with_capacity(code.len() / 4);

                            while let [a, b, c, d, tail @ ..] = code {
                                words.push(u32::from_ne_bytes([*a, *b, *c, *d]));
                                code = tail;
                            }

                            compiled = words.into();
                            &*compiled
                        }
                    },
                    _ => {
                        compiled = compile_shader(&source.code, source.filename, source.language)
                            .map_err(|err| CreateLibraryError(err.into()))?;
                        &*compiled
                    }
                };
                let result = unsafe {
                    me.device.create_shader_module(
                        &vk::ShaderModuleCreateInfo::builder().code(code),
                        None,
                    )
                };
                let module = result.map_err(|err| match err {
                    vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
                    vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => {
                        CreateLibraryError(OutOfMemory.into())
                    }
                    _ => unexpected_error(err),
                })?;

                #[cfg(any(debug_assertions, feature = "debug"))]
                self.set_object_name(vk::ObjectType::SHADER_MODULE, module.as_raw(), desc.name);

                Ok(Library::new(module))
            }
        }
    }

    fn new_render_pipeline(
        &self,
        desc: RenderPipelineDesc,
    ) -> Result<RenderPipeline, CreatePipelineError> {
        let layout_desc = PipelineLayoutDesc {
            groups: desc
                .arguments
                .iter()
                .map(|group| group.arguments.to_vec())
                .collect(),
        };

        let layout = self
            .new_pipeline_layout(layout_desc)
            .map_err(|err| CreatePipelineError(err.into()))?;

        let vertex_attributes = desc
            .vertex_attributes
            .iter()
            .enumerate()
            .map(|(idx, attr)| vk::VertexInputAttributeDescription {
                location: idx as u32,
                binding: attr.buffer_index,
                format: attr.format.try_into_ash().expect("Unsupported on Vulkan"),
                offset: attr.offset,
            })
            .collect::<Vec<_>>();

        let vertex_bindings = desc
            .vertex_layouts
            .iter()
            .enumerate()
            .map(|(idx, attr)| vk::VertexInputBindingDescription {
                binding: idx as u32,
                stride: attr.stride,
                input_rate: match attr.step_mode {
                    VertexStepMode::Vertex => vk::VertexInputRate::VERTEX,
                    VertexStepMode::Instance { rate: 1 } => vk::VertexInputRate::INSTANCE,
                    VertexStepMode::Instance { rate } => {
                        panic!(
                            "Instance vertex step mode with rate {rate} is not supported on Vulkan"
                        )
                    }
                    VertexStepMode::Constant => {
                        panic!("Constant vertex step mode is not supported on Vulkan")
                    }
                },
            })
            .collect::<Vec<_>>();

        let mut names = Vec::with_capacity(2);

        let mut stages = vec![vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(desc.vertex_shader.library.module())
            .name({
                let entry = std::ffi::CString::new(&*desc.vertex_shader.entry).unwrap();
                names.push(entry);
                names.last().unwrap() // Here CStr will not outlive `stages`, but we only use a raw pointer from it, which will be valid until end of this function.
            })
            .build()];

        let mut raster_state = vk::PipelineRasterizationStateCreateInfo::builder();
        let mut depth_state = vk::PipelineDepthStencilStateCreateInfo::builder();
        let mut attachments = Vec::new();

        if let Some(raster) = &desc.raster {
            if let Some(fragment_shader) = &raster.fragment_shader {
                stages.push(
                    vk::PipelineShaderStageCreateInfo::builder()
                        .stage(vk::ShaderStageFlags::FRAGMENT)
                        .module(fragment_shader.library.module())
                        .name({
                            let entry = std::ffi::CString::new(&*fragment_shader.entry).unwrap();
                            names.push(entry);
                            names.last().unwrap() // Here CStr will not outlive `stages`, but we only use a raw pointer from it, which will be valid until end of this function.
                        })
                        .build(),
                );
            }

            raster_state = raster_state
                .depth_clamp_enable(true)
                .rasterizer_discard_enable(false)
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::BACK)
                .front_face(vk::FrontFace::CLOCKWISE);

            if let Some(depth) = &raster.depth_stencil {
                depth_state = depth_state
                    .depth_test_enable(depth.format.is_depth())
                    .depth_compare_op(depth.compare.into_ash())
                    .depth_write_enable(depth.write_enabled)
                    .stencil_test_enable(depth.format.is_stencil());
            }

            for color in &raster.color_targets {
                let mut blend_state = vk::PipelineColorBlendAttachmentState::builder();
                if let Some(blend) = color.blend {
                    blend_state = blend_state
                        .blend_enable(true)
                        .src_color_blend_factor(blend.color.src.into_ash())
                        .dst_color_blend_factor(blend.color.dst.into_ash())
                        .color_blend_op(blend.color.op.into_ash())
                        .src_alpha_blend_factor(blend.alpha.src.into_ash())
                        .dst_alpha_blend_factor(blend.alpha.dst.into_ash())
                        .alpha_blend_op(blend.alpha.op.into_ash())
                        .color_write_mask(blend.mask.into_ash());
                }
                attachments.push(blend_state.build());
            }
        } else {
            raster_state = raster_state.rasterizer_discard_enable(true);
        }

        let result = unsafe {
            self.inner.device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                std::slice::from_ref(
                    &vk::GraphicsPipelineCreateInfo::builder()
                        .stages(&stages)
                        .vertex_input_state(
                            &vk::PipelineVertexInputStateCreateInfo::builder()
                                .vertex_attribute_descriptions(&vertex_attributes)
                                .vertex_binding_descriptions(&vertex_bindings),
                        )
                        .input_assembly_state(
                            &vk::PipelineInputAssemblyStateCreateInfo::builder().topology(
                                match desc.primitive_topology {
                                    PrimitiveTopology::Point => vk::PrimitiveTopology::POINT_LIST,
                                    PrimitiveTopology::Line => vk::PrimitiveTopology::LINE_LIST,
                                    PrimitiveTopology::Triangle => {
                                        vk::PrimitiveTopology::TRIANGLE_LIST
                                    }
                                },
                            ),
                        )
                        .rasterization_state(&raster_state)
                        .multisample_state(
                            &vk::PipelineMultisampleStateCreateInfo::builder()
                                .rasterization_samples(vk::SampleCountFlags::TYPE_1),
                        )
                        .depth_stencil_state(&depth_state)
                        .color_blend_state(
                            &vk::PipelineColorBlendStateCreateInfo::builder()
                                .attachments(&attachments)
                                .blend_constants([0.0; 4]),
                        )
                        .dynamic_state(
                            &vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&[
                                vk::DynamicState::VIEWPORT,
                                vk::DynamicState::SCISSOR,
                            ]),
                        )
                        .layout(layout.handle()),
                ),
                None,
            )
        };

        let pipelines = result.map_err(|(_, err)| match err {
            vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
            vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => CreatePipelineError(OutOfMemory.into()),
            _ => unexpected_error(err),
        })?;
        let pipeline = pipelines[0];

        #[cfg(any(debug_assertions, feature = "debug"))]
        self.set_object_name(vk::ObjectType::PIPELINE, pipeline.as_raw(), desc.name);

        let idx = self.inner.pipelines.lock().insert(pipeline);

        Ok(RenderPipeline::new(self.weak(), pipeline, idx, layout))
    }

    fn new_buffer(&self, desc: BufferDesc) -> Result<Buffer, OutOfMemory> {
        let size = u64::try_from(desc.layout.size()).map_err(|_| OutOfMemory)?;
        let align = u64::try_from(desc.layout.align()).map_err(|_| OutOfMemory)?;

        let buffer = unsafe {
            self.inner.device.create_buffer(
                &vk::BufferCreateInfo::builder()
                    .size(size)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .usage(desc.usage.into_ash()),
                None,
            )
        }
        .map_err(|err| match err {
            vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
            vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => OutOfMemory,
            err => unexpected_error(err),
        })?;

        let requirements = unsafe { self.inner.device.get_buffer_memory_requirements(buffer) };
        let align_mask = requirements.alignment.max(align) - 1;

        let block = unsafe {
            self.inner.allocator.lock().alloc(
                AshMemoryDevice::wrap(&self.inner.device),
                gpu_alloc::Request {
                    size: requirements.size,
                    align_mask,
                    usage: memory_to_usage_flags(desc.memory),
                    memory_types: requirements.memory_type_bits,
                },
            )
        }
        .map_err(|err| match err {
            gpu_alloc::AllocationError::OutOfDeviceMemory => OutOfMemory,
            gpu_alloc::AllocationError::OutOfHostMemory => handle_host_oom(),
            gpu_alloc::AllocationError::NoCompatibleMemoryTypes => OutOfMemory,
            gpu_alloc::AllocationError::TooManyObjects => OutOfMemory,
        })?;

        #[cfg(any(debug_assertions, feature = "debug"))]
        self.set_object_name(vk::ObjectType::BUFFER, buffer.as_raw(), desc.name);

        let idx = self.inner.buffers.lock().insert(buffer);

        let buffer = Buffer::new(self.weak(), buffer, desc.layout, desc.usage, block, idx);
        Ok(buffer)
    }

    fn new_image(&self, desc: ImageDesc) -> Result<Image, ImageError> {
        let image = unsafe {
            self.inner.device.create_image(
                &vk::ImageCreateInfo::builder()
                    .image_type(desc.dimensions.into_ash())
                    .format(
                        desc.format
                            .try_into_ash()
                            .ok_or(ImageError::InvalidFormat)?,
                    )
                    .array_layers(desc.layers)
                    .mip_levels(desc.levels)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage((desc.usage, desc.format).into_ash())
                    .initial_layout(vk::ImageLayout::GENERAL),
                None,
            )
        }
        .map_err(|err| match err {
            vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
            vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => ImageError::OutOfMemory,
            err => unexpected_error(err),
        })?;

        let requirements = unsafe { self.inner.device.get_image_memory_requirements(image) };
        let align_mask = requirements.alignment - 1;

        let block = unsafe {
            self.inner.allocator.lock().alloc(
                AshMemoryDevice::wrap(&self.inner.device),
                gpu_alloc::Request {
                    size: requirements.size,
                    align_mask,
                    usage: memory_to_usage_flags(Memory::Device),
                    memory_types: requirements.memory_type_bits,
                },
            )
        }
        .map_err(|err| match err {
            gpu_alloc::AllocationError::OutOfDeviceMemory => OutOfMemory,
            gpu_alloc::AllocationError::OutOfHostMemory => handle_host_oom(),
            gpu_alloc::AllocationError::NoCompatibleMemoryTypes => OutOfMemory,
            gpu_alloc::AllocationError::TooManyObjects => OutOfMemory,
        })?;

        #[cfg(any(debug_assertions, feature = "debug"))]
        self.set_object_name(vk::ObjectType::IMAGE, image.as_raw(), desc.name);

        let idx = self.inner.images.lock().insert(image);

        let image = Image::new(
            self.weak(),
            image,
            desc.dimensions,
            desc.format,
            desc.usage,
            desc.layers,
            desc.levels,
            block,
            idx,
        );
        Ok(image)
    }

    fn new_sampler(&self, desc: SamplerDesc) -> Result<Sampler, OutOfMemory> {
        let mut samplers = self.inner.samplers.lock();
        let len = samplers.len();
        match samplers.entry(desc) {
            hashbrown::hash_map::Entry::Occupied(entry) => match entry.get().upgrade() {
                Some(sampler) => Ok(sampler),
                None => {
                    let sampler = self.new_sampler_slow(len, desc)?;
                    entry.replace_entry(sampler.downgrade());
                    Ok(sampler)
                }
            },
            hashbrown::hash_map::Entry::Vacant(entry) => {
                let sampler = self.new_sampler_slow(len, desc)?;
                entry.insert(sampler.downgrade());
                Ok(sampler)
            }
        }
    }

    fn new_surface(
        &self,
        window: &impl HasRawWindowHandle,
        display: &impl HasRawDisplayHandle,
    ) -> Result<Surface, SurfaceError> {
        let me = &*self.inner;
        assert!(
            me.features.contains(Features::SURFACE),
            "Surface feature is not enabled"
        );

        let window = window.raw_window_handle();
        let display = display.raw_display_handle();

        match (window, display) {
            #[cfg(target_os = "windows")]
            (RawWindowHandle::Win32(window), RawDisplayHandle::Windows(_)) => unsafe {
                let win32_surface = me.win32_surface.as_ref().unwrap();

                let surface = win32_surface
                    .create_win32_surface(
                        &ash::vk::Win32SurfaceCreateInfoKHR::builder()
                            // .hinstance(hinstance)
                            .hwnd(window.hwnd),
                        None,
                    )
                    .map_err(|err| match err {
                        vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
                        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => SurfaceError(OutOfMemory.into()),
                        err => unexpected_error(err),
                    })?;

                let result = unsafe {
                    self.surface()
                        .get_physical_device_surface_formats(self.physical_device(), surface)
                };
                let formats = result.map_err(|err| match err {
                    ash::vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
                    ash::vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => SurfaceError(OutOfMemory.into()),
                    ash::vk::Result::ERROR_SURFACE_LOST_KHR => {
                        SurfaceError(SurfaceErrorKind::SurfaceLost)
                    }
                    _ => unexpected_error(err),
                })?;

                let result = unsafe {
                    self.surface()
                        .get_physical_device_surface_present_modes(self.physical_device(), surface)
                };
                let modes = result.map_err(|err| match err {
                    ash::vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
                    ash::vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => SurfaceError(OutOfMemory.into()),
                    ash::vk::Result::ERROR_SURFACE_LOST_KHR => {
                        SurfaceError(SurfaceErrorKind::SurfaceLost)
                    }
                    _ => unexpected_error(err),
                })?;

                let family_supports =
                    self.queue_families()
                        .try_fold(Vec::new(), |mut supports, idx| {
                            let result = unsafe {
                                self.surface().get_physical_device_surface_support(
                                    self.physical_device(),
                                    idx,
                                    surface,
                                )
                            };
                            let support = result.map_err(|err| match err {
                                ash::vk::Result::ERROR_OUT_OF_HOST_MEMORY => handle_host_oom(),
                                ash::vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => {
                                    SurfaceError(OutOfMemory.into())
                                }
                                ash::vk::Result::ERROR_SURFACE_LOST_KHR => {
                                    SurfaceError(SurfaceErrorKind::SurfaceLost)
                                }
                                _ => unexpected_error(err),
                            })?;
                            supports.push(support);
                            Ok(supports)
                        })?;

                Ok(Surface::new(
                    self.weak(),
                    surface,
                    formats,
                    modes,
                    family_supports,
                ))
            },
            (RawWindowHandle::Win32(_), _) => {
                panic!("Mismatched window and display type")
            }
            _ => {
                unreachable!("Unsupported window type for this platform")
            }
        }
    }
}

fn memory_to_usage_flags(memory: Memory) -> gpu_alloc::UsageFlags {
    match memory {
        Memory::Device => gpu_alloc::UsageFlags::FAST_DEVICE_ACCESS,
        Memory::Shared => gpu_alloc::UsageFlags::HOST_ACCESS,
        Memory::Upload => gpu_alloc::UsageFlags::HOST_ACCESS | gpu_alloc::UsageFlags::UPLOAD,
        Memory::Download => gpu_alloc::UsageFlags::HOST_ACCESS | gpu_alloc::UsageFlags::DOWNLOAD,
    }
}
