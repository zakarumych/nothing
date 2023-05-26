use std::{marker::PhantomData, mem::size_of_val, rc::Rc};

pub use ::egui::*;

use blink_alloc::{Blink, BlinkAlloc};
use edict::World;
use egui::epaint::{textures::TexturesDelta, ClippedShape, Primitive, Vertex};
use hashbrown::{hash_map::Entry, HashMap};
use nix::{Arguments as _, ClearColor, Constants as _};
use winit::{event::WindowEvent, event_loop::EventLoopWindowTarget, window::WindowId};

use crate::{
    events::Event,
    funnel::Filter,
    render::{Render, RenderBuilderContext, RenderContext, RenderError, RenderGraph, TargetId},
};

#[derive(Clone, Copy)]
enum Sampler {
    NearestNearest = 0,
    NearestLinear = 1,
    LinearNearest = 2,
    LinearLinear = 3,
}

impl Sampler {
    fn from_options(options: TextureOptions) -> Self {
        match (options.minification, options.magnification) {
            (TextureFilter::Nearest, TextureFilter::Nearest) => Sampler::NearestNearest,
            (TextureFilter::Nearest, TextureFilter::Linear) => Sampler::NearestLinear,
            (TextureFilter::Linear, TextureFilter::Nearest) => Sampler::LinearNearest,
            (TextureFilter::Linear, TextureFilter::Linear) => Sampler::LinearLinear,
        }
    }
}

fn default_scale() -> f32 {
    1.5
}

struct EguiInstance {
    cx: Context,
    state: egui_winit::State,
    textures_delta: TexturesDelta,
    shapes: Vec<ClippedShape>,
    textures: HashMap<TextureId, (nix::Image, Sampler)>,
    scale: f32,
}

pub struct EguiResource {
    instances: HashMap<WindowId, EguiInstance>,
    _local: PhantomData<Rc<u32>>,
}

impl EguiResource {
    pub fn new() -> Self {
        EguiResource {
            instances: HashMap::new(),
            _local: PhantomData,
        }
    }

    pub fn add_window<T>(
        &mut self,
        window: &winit::window::Window,
        event_loop: &EventLoopWindowTarget<T>,
    ) {
        let mut state = egui_winit::State::new(event_loop);
        state.set_pixels_per_point(default_scale());

        let cx = Context::default();

        self.instances.insert(
            window.id(),
            EguiInstance {
                cx,
                state,
                textures_delta: epaint::textures::TexturesDelta::default(),
                shapes: Vec::new(),
                textures: HashMap::new(),
                scale: default_scale(),
            },
        );
    }

    pub fn run<R>(
        &mut self,
        window: &winit::window::Window,
        run_ui: impl FnOnce(&Context) -> R,
    ) -> Option<R> {
        let Some(instance) = self.instances.get_mut(&window.id()) else { return None; };

        let raw_input = instance.state.take_egui_input(window);

        instance.cx.begin_frame(raw_input);
        let ret = run_ui(&instance.cx);
        let output = instance.cx.end_frame();

        instance
            .state
            .handle_platform_output(window, &instance.cx, output.platform_output);

        instance.textures_delta.append(output.textures_delta);
        instance.shapes = output.shapes;
        Some(ret)
    }

    pub fn handle_event(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let Some(instance) = self.instances.get_mut(&window_id) else { return false; };

        let response = instance.state.on_event(&instance.cx, event);

        if let WindowEvent::Destroyed = event {
            self.instances.remove(&window_id);
        }

        response.consumed
    }
}

#[derive(nix::Arguments)]
struct EguiArguments {
    #[nix(fragment)]
    sampler: nix::Sampler,
    #[nix(fragment)]
    texture: nix::Image,
}

#[derive(nix::Constants)]
struct EguiConstants {
    width: u32,
    height: u32,
    scale: f32,
}

pub struct EguiRender {
    target: TargetId<nix::Image>,
    window: WindowId,
    samplers: Option<[nix::Sampler; 4]>,
    linear_pipeline: Option<nix::RenderPipeline>,
    srgb_pipeline: Option<nix::RenderPipeline>,

    vertex_buffer: Option<nix::Buffer>,
    index_buffer: Option<nix::Buffer>,
    load_op: nix::LoadOp<nix::ClearColor>,
}

impl EguiRender {
    fn new(
        target: TargetId<nix::Image>,
        window: WindowId,
        load_op: nix::LoadOp<nix::ClearColor>,
    ) -> Self {
        EguiRender {
            target,
            window,
            samplers: None,
            linear_pipeline: None,
            srgb_pipeline: None,
            vertex_buffer: None,
            index_buffer: None,
            load_op,
        }
    }

    pub fn build_overlay(
        target: TargetId<nix::Image>,
        graph: &mut RenderGraph,
        window: WindowId,
    ) -> TargetId<nix::Image> {
        let mut builder = RenderBuilderContext::new("egui", graph);
        let new_target = builder.write_target(target, nix::PipelineStages::COLOR_OUTPUT);
        builder.build(EguiRender::new(new_target, window, nix::LoadOp::Load));
        new_target
    }

    pub fn build(
        graph: &mut RenderGraph,
        window: WindowId,
        color: ClearColor,
    ) -> TargetId<nix::Image> {
        let mut builder = RenderBuilderContext::new("egui", graph);
        let new_target = builder.create_target("egui-surface", nix::PipelineStages::COLOR_OUTPUT);
        builder.build(EguiRender::new(
            new_target,
            window,
            nix::LoadOp::Clear(color),
        ));
        new_target
    }
}

impl Render for EguiRender {
    fn render(
        &mut self,
        mut cx: RenderContext<'_, '_>,
        world: &World,
        _blink: &BlinkAlloc,
    ) -> Result<(), RenderError> {
        // Safety:
        // This code does not touch thread-local parts of the resource.
        let egui = unsafe { world.get_local_resource_mut::<EguiResource>() };
        let Some(mut egui) = egui else { return Ok(()); };

        let Some(instance) = egui.instances.get_mut(&self.window) else { return Ok(()); };

        let samplers = match &mut self.samplers {
            Some(samplers) => &*samplers,
            none => {
                let sampler_nn = cx.device().new_sampler(nix::SamplerDesc {
                    min_filter: nix::Filter::Nearest,
                    mag_filter: nix::Filter::Nearest,
                    address_mode: [nix::AddressMode::ClampToEdge; 3],
                    ..nix::SamplerDesc::new()
                })?;
                let sampler_nl = cx.device().new_sampler(nix::SamplerDesc {
                    min_filter: nix::Filter::Nearest,
                    mag_filter: nix::Filter::Linear,
                    address_mode: [nix::AddressMode::ClampToEdge; 3],
                    ..nix::SamplerDesc::new()
                })?;
                let sampler_ln = cx.device().new_sampler(nix::SamplerDesc {
                    min_filter: nix::Filter::Linear,
                    mag_filter: nix::Filter::Nearest,
                    address_mode: [nix::AddressMode::ClampToEdge; 3],
                    ..nix::SamplerDesc::new()
                })?;
                let sampler_ll = cx.device().new_sampler(nix::SamplerDesc {
                    min_filter: nix::Filter::Linear,
                    mag_filter: nix::Filter::Linear,
                    address_mode: [nix::AddressMode::ClampToEdge; 3],
                    ..nix::SamplerDesc::new()
                })?;
                none.get_or_insert([sampler_nn, sampler_nl, sampler_ln, sampler_ll])
            }
        };

        let mut encoder = cx.new_command_encoder()?;

        let target = cx.write_target(self.target, &mut encoder).clone();

        let mut copy_encoder = encoder.copy();

        copy_encoder.barrier(
            nix::PipelineStages::VERTEX_INPUT | nix::PipelineStages::FRAGMENT_SHADER,
            nix::PipelineStages::TRANSFER,
        );

        if !instance.textures_delta.set.is_empty() {
            let delta_size = instance
                .textures_delta
                .set
                .iter()
                .fold(0, |acc, (_, delta)| {
                    acc + match &delta.image {
                        ImageData::Color(color) => std::mem::size_of_val(&color.pixels[..]),
                        ImageData::Font(font) => std::mem::size_of_val(&font.pixels[..]),
                    }
                });

            let mut upload_buffer = cx.device().new_buffer(nix::BufferDesc {
                size: delta_size,
                usage: nix::BufferUsage::TRANSFER_SRC,
                memory: nix::Memory::Upload,
                name: "texture-delta-upload",
            })?;

            let mut offset = 0usize;
            for (_, delta) in instance.textures_delta.set.iter() {
                match &delta.image {
                    ImageData::Color(color) => unsafe {
                        upload_buffer
                            .write_unchecked(offset, bytemuck::cast_slice(&color.pixels[..]));
                        offset += std::mem::size_of_val(&color.pixels[..]);
                    },
                    ImageData::Font(font) => unsafe {
                        upload_buffer
                            .write_unchecked(offset, bytemuck::cast_slice(&font.pixels[..]));
                        offset += std::mem::size_of_val(&font.pixels[..]);
                    },
                }
            }

            let mut offset = 0usize;
            for (id, delta) in instance.textures_delta.set.iter() {
                let region = delta.image.size();
                let pos = delta.pos.unwrap_or([0; 2]);
                let size = [pos[0] + region[0], pos[1] + region[1]];

                let format = match &delta.image {
                    ImageData::Color(_) => nix::PixelFormat::Rgba8Srgb,
                    ImageData::Font(_) => nix::PixelFormat::R32Float,
                };

                let mut image: nix::Image;
                match instance.textures.entry(*id) {
                    Entry::Vacant(entry) => {
                        let mut new_image = cx
                            .device()
                            .new_image(nix::ImageDesc {
                                dimensions: nix::ImageDimensions::D2(
                                    size[0] as u32,
                                    size[1] as u32,
                                ),
                                format,
                                usage: nix::ImageUsage::SAMPLED | nix::ImageUsage::TRANSFER_DST,
                                layers: 1,
                                levels: 1,
                                name: &format!("egui-texture-{id:?}"),
                            })
                            .map_err(|err| match err {
                                nix::ImageError::InvalidFormat => unimplemented!(),
                                nix::ImageError::OutOfMemory => {
                                    RenderError::OutOfMemory(nix::OutOfMemory)
                                }
                            })?;

                        copy_encoder.init_image(
                            nix::PipelineStages::empty(),
                            nix::PipelineStages::TRANSFER,
                            &new_image,
                        );

                        if let ImageData::Font(_) = &delta.image {
                            new_image = new_image.view(
                                cx.device(),
                                nix::ViewDesc::new(format).swizzle(nix::Swizzle::RRRR),
                            )?;
                        }

                        image = entry
                            .insert((new_image, Sampler::from_options(delta.options)))
                            .0
                            .clone();
                    }
                    Entry::Occupied(mut entry) => {
                        entry.get_mut().1 = Sampler::from_options(delta.options);
                        image = entry.get().0.clone();
                        let extent = image.dimensions().to_2d();
                        if (extent.width() as usize) < size[0]
                            || (extent.height() as usize) < size[1]
                        {
                            let mut new_image = cx
                                .device()
                                .new_image(nix::ImageDesc {
                                    dimensions: nix::ImageDimensions::D2(
                                        size[0] as u32,
                                        size[1] as u32,
                                    ),
                                    format,
                                    usage: nix::ImageUsage::SAMPLED | nix::ImageUsage::TRANSFER_DST,
                                    layers: 1,
                                    levels: 1,
                                    name: &format!("egui-texture-{id:?}"),
                                })
                                .map_err(|err| match err {
                                    nix::ImageError::InvalidFormat => unimplemented!(),
                                    nix::ImageError::OutOfMemory => {
                                        RenderError::OutOfMemory(nix::OutOfMemory)
                                    }
                                })?;

                            if let ImageData::Font(_) = &delta.image {
                                new_image = new_image.view(
                                    cx.device(),
                                    nix::ViewDesc::new(format).swizzle(nix::Swizzle::RRRR),
                                )?;
                            }

                            copy_encoder.copy_image_region(
                                &image,
                                nix::Offset3::ZERO,
                                0,
                                &new_image,
                                nix::Offset3::ZERO,
                                0,
                                image.dimensions().to_3d(),
                                1,
                            );

                            entry.get_mut().0 = new_image.clone();
                            image = new_image;
                        }
                    }
                }

                copy_encoder.copy_buffer_to_image(
                    &upload_buffer,
                    offset,
                    4 * region[0],
                    0,
                    &image,
                    nix::Offset3::new(pos[0] as u32, pos[1] as u32, 0),
                    nix::Extent3::new(region[0] as u32, region[1] as u32, 1),
                    0..1,
                    0,
                );

                match &delta.image {
                    ImageData::Color(color) => {
                        offset += std::mem::size_of_val(&color.pixels[..]);
                    }
                    ImageData::Font(font) => {
                        offset += std::mem::size_of_val(&font.pixels[..]);
                    }
                }
            }

            instance.textures_delta.set.clear();
        }

        if !instance.shapes.is_empty() {
            let primitives = instance.cx.tessellate(std::mem::take(&mut instance.shapes));

            if !primitives.is_empty() {
                let mut total_vertex_size = 0;
                let mut total_index_size = 0;

                for primitive in &primitives {
                    match &primitive.primitive {
                        Primitive::Mesh(mesh) => {
                            total_vertex_size += size_of_val(&mesh.vertices[..]);
                            total_vertex_size = (total_vertex_size + 31) & !31;
                            total_index_size += size_of_val(&mesh.indices[..]);
                            total_index_size = (total_index_size + 31) & !31;
                        }
                        Primitive::Callback(_) => todo!(),
                    }
                }

                let vertex_buffer = match &mut self.vertex_buffer {
                    Some(buffer) if buffer.size() >= total_vertex_size => buffer,
                    slot => {
                        *slot = None;
                        slot.get_or_insert(cx.device().new_buffer(nix::BufferDesc {
                            size: total_vertex_size,
                            usage: nix::BufferUsage::VERTEX | nix::BufferUsage::TRANSFER_DST,
                            memory: nix::Memory::Device,
                            name: "egui-vertex-buffer",
                        })?)
                    }
                };

                let index_buffer = match &mut self.index_buffer {
                    Some(buffer) if buffer.size() >= total_index_size => buffer,
                    slot => {
                        *slot = None;
                        slot.get_or_insert(cx.device().new_buffer(nix::BufferDesc {
                            size: total_index_size,
                            usage: nix::BufferUsage::INDEX | nix::BufferUsage::TRANSFER_DST,
                            memory: nix::Memory::Device,
                            name: "egui-index-buffer",
                        })?)
                    }
                };

                let mut vertex_buffer_offset = 0;
                let mut index_buffer_offset = 0;

                for primitive in &primitives {
                    match &primitive.primitive {
                        Primitive::Mesh(mesh) => {
                            copy_encoder.write_buffer(
                                &vertex_buffer,
                                vertex_buffer_offset,
                                bytemuck::cast_slice(&mesh.vertices[..]),
                            );
                            copy_encoder.write_buffer(
                                &index_buffer,
                                index_buffer_offset,
                                bytemuck::cast_slice(&mesh.indices[..]),
                            );
                            vertex_buffer_offset += size_of_val(&mesh.vertices[..]);
                            vertex_buffer_offset = (vertex_buffer_offset + 31) & !31;
                            index_buffer_offset += size_of_val(&mesh.indices[..]);
                            index_buffer_offset = (index_buffer_offset + 31) & !31;
                        }
                        Primitive::Callback(_) => todo!(),
                    }
                }

                copy_encoder.barrier(
                    nix::PipelineStages::TRANSFER,
                    nix::PipelineStages::VERTEX_INPUT | nix::PipelineStages::FRAGMENT_SHADER,
                );

                drop(copy_encoder);

                let pipeline = if target.format().is_srgb() {
                    self.srgb_pipeline.get_or_insert_with(|| {
                        let library = cx
                            .device()
                            .new_shader_library(nix::LibraryDesc {
                                name: "egui",
                                input: nix::include_library!(
                                    "shaders/egui.wgsl" as nix::ShaderLanguage::Wgsl
                                ),
                            })
                            .unwrap();

                        cx.device()
                            .new_render_pipeline(nix::RenderPipelineDesc {
                                name: "egui",
                                vertex_shader: nix::Shader {
                                    library: library.clone(),
                                    entry: "vs_main".into(),
                                },
                                vertex_attributes: vec![
                                    nix::VertexAttributeDesc {
                                        format: nix::VertexFormat::Float32x2,
                                        offset: offset_of!(Vertex.pos) as u32,
                                        buffer_index: 0,
                                    },
                                    nix::VertexAttributeDesc {
                                        format: nix::VertexFormat::Float32x2,
                                        offset: offset_of!(Vertex.uv) as u32,
                                        buffer_index: 0,
                                    },
                                    nix::VertexAttributeDesc {
                                        format: nix::VertexFormat::Unorm8x4,
                                        offset: offset_of!(Vertex.color) as u32,
                                        buffer_index: 0,
                                    },
                                ],
                                vertex_layouts: vec![nix::VertexLayoutDesc {
                                    stride: std::mem::size_of::<Vertex>() as u32,
                                    step_mode: nix::VertexStepMode::Vertex,
                                }],
                                primitive_topology: nix::PrimitiveTopology::Triangle,
                                raster: Some(nix::RasterDesc {
                                    fragment_shader: Some(nix::Shader {
                                        library: library,
                                        entry: "fs_main_srgb".into(),
                                    }),
                                    color_targets: vec![nix::ColorTargetDesc {
                                        format: target.format(),
                                        blend: Some(nix::BlendDesc::default()),
                                    }],
                                    depth_stencil: None,
                                    front_face: nix::FrontFace::default(),
                                    culling: nix::Culling::None,
                                }),
                                arguments: &[EguiArguments::LAYOUT],
                                constants: EguiConstants::SIZE,
                            })
                            .unwrap()
                    })
                } else {
                    self.linear_pipeline.get_or_insert_with(|| {
                        let library = cx
                            .device()
                            .new_shader_library(nix::LibraryDesc {
                                name: "egui",
                                input: nix::include_library!(
                                    "shaders/egui.wgsl" as nix::ShaderLanguage::Wgsl
                                ),
                            })
                            .unwrap();

                        cx.device()
                            .new_render_pipeline(nix::RenderPipelineDesc {
                                name: "egui",
                                vertex_shader: nix::Shader {
                                    library: library.clone(),
                                    entry: "vs_main".into(),
                                },
                                vertex_attributes: vec![
                                    nix::VertexAttributeDesc {
                                        format: nix::VertexFormat::Float32x2,
                                        offset: offset_of!(Vertex.pos) as u32,
                                        buffer_index: 0,
                                    },
                                    nix::VertexAttributeDesc {
                                        format: nix::VertexFormat::Float32x2,
                                        offset: offset_of!(Vertex.uv) as u32,
                                        buffer_index: 0,
                                    },
                                    nix::VertexAttributeDesc {
                                        format: nix::VertexFormat::Unorm8x4,
                                        offset: offset_of!(Vertex.color) as u32,
                                        buffer_index: 0,
                                    },
                                ],
                                vertex_layouts: vec![nix::VertexLayoutDesc {
                                    stride: std::mem::size_of::<Vertex>() as u32,
                                    step_mode: nix::VertexStepMode::Vertex,
                                }],
                                primitive_topology: nix::PrimitiveTopology::Triangle,
                                raster: Some(nix::RasterDesc {
                                    fragment_shader: Some(nix::Shader {
                                        library: library,
                                        entry: "fs_main_linear".into(),
                                    }),
                                    color_targets: vec![nix::ColorTargetDesc {
                                        format: target.format(),
                                        blend: Some(nix::BlendDesc::default()),
                                    }],
                                    depth_stencil: None,
                                    front_face: nix::FrontFace::default(),
                                    culling: nix::Culling::None,
                                }),
                                arguments: &[EguiArguments::LAYOUT],
                                constants: EguiConstants::SIZE,
                            })
                            .unwrap()
                    })
                };

                let dims = target.dimensions().to_2d();

                let mut render = encoder.render(nix::RenderPassDesc {
                    color_attachments: &[nix::AttachmentDesc::new(&target).load_op(self.load_op)],
                    ..Default::default()
                });

                render.with_pipeline(pipeline);
                render.with_viewport(
                    nix::Offset3::ZERO,
                    nix::Extent3::new(dims.width() as f32, dims.height() as f32, 1.0),
                );
                render.with_constants(&EguiConstants {
                    width: dims.width(),
                    height: dims.height(),
                    scale: instance.cx.pixels_per_point(),
                });

                let mut vertex_buffer_offset = 0;
                let mut index_buffer_offset = 0;

                for primitive in primitives {
                    match primitive.primitive {
                        Primitive::Mesh(mesh) => {
                            render.with_scissor(
                                nix::Offset2::new(
                                    (primitive.clip_rect.left() * instance.scale) as i32,
                                    (primitive.clip_rect.top() * instance.scale) as i32,
                                ),
                                nix::Extent2::new(
                                    (primitive.clip_rect.width() * instance.scale) as u32,
                                    (primitive.clip_rect.height() * instance.scale) as u32,
                                ),
                            );

                            let (ref image, sampler) = instance.textures[&mesh.texture_id];

                            render.with_arguments(
                                0,
                                &EguiArguments {
                                    sampler: samplers[sampler as usize].clone(),
                                    texture: image.clone(),
                                },
                            );

                            render
                                .bind_vertex_buffers(0, &[(&vertex_buffer, vertex_buffer_offset)]);
                            render.bind_index_buffer(&index_buffer, index_buffer_offset);
                            render.draw_indexed(0, 0..mesh.indices.len() as u32, 0..1);

                            vertex_buffer_offset += size_of_val(&mesh.vertices[..]);
                            vertex_buffer_offset = (vertex_buffer_offset + 31) & !31;
                            index_buffer_offset += size_of_val(&mesh.indices[..]);
                            index_buffer_offset = (index_buffer_offset + 31) & !31;
                        }
                        Primitive::Callback(_) => todo!(),
                    }
                }
            }
        }

        for id in instance.textures_delta.free.iter() {
            instance.textures.remove(id);
        }
        instance.textures_delta.free.clear();

        cx.commit(encoder.finish()?);

        Ok(())
    }
}

pub struct EguiFilter;

impl Filter for EguiFilter {
    fn filter(&mut self, _blink: &Blink, world: &mut World, event: Event) -> Option<Event> {
        let world = world.local();
        let egui = &mut *world.expect_resource_mut::<EguiResource>();

        if let Event::WindowEvent { window_id, event } = &event {
            if egui.handle_event(*window_id, event) {
                return None;
            }
        }

        Some(event)
    }
}
