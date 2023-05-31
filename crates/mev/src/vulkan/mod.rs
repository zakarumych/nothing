use std::{alloc::Layout, fmt};

use ash::vk;

mod access;
mod arguments;
mod buffer;
mod command;
mod device;
mod from;
mod image;
mod instance;
mod layout;
mod queue;
mod refs;
mod render_pipeline;
mod sampler;
mod shader;
mod surface;

use crate::generic::PixelFormat;

pub use self::{
    buffer::Buffer,
    command::{CommandBuffer, CommandEncoder, CopyCommandEncoder, RenderCommandEncoder},
    device::Device,
    image::Image,
    instance::Instance,
    queue::Queue,
    render_pipeline::RenderPipeline,
    sampler::Sampler,
    shader::Library,
    surface::{Frame, Surface},
};

pub(crate) use self::{
    instance::{CreateErrorKind, LoadErrorKind},
    render_pipeline::CreatePipelineErrorKind,
    shader::CreateLibraryErrorKind,
    surface::SurfaceErrorKind,
};

#[track_caller]
fn handle_host_oom() -> ! {
    std::alloc::handle_alloc_error(Layout::new::<()>())
}

#[track_caller]
fn unexpected_error(err: vk::Result) -> ! {
    unreachable!("unexpected error: {err:?}")
}

/// Version of the API.
/// For internal use only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    const V1_0: Self = Self {
        major: 1,
        minor: 0,
        patch: 0,
    };

    const V1_1: Self = Self {
        major: 1,
        minor: 1,
        patch: 0,
    };

    const V1_2: Self = Self {
        major: 1,
        minor: 2,
        patch: 0,
    };

    const V1_3: Self = Self {
        major: 1,
        minor: 3,
        patch: 0,
    };

    fn api_version(&self) -> u32 {
        vk::make_api_version(0, self.major, self.minor, self.patch)
    }
}

impl fmt::Display for Version {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[inline(always)]
fn format_aspect(format: PixelFormat) -> vk::ImageAspectFlags {
    let mut aspect = vk::ImageAspectFlags::empty();
    if format.is_color() {
        aspect |= vk::ImageAspectFlags::COLOR;
    }
    if format.is_depth() {
        aspect |= vk::ImageAspectFlags::DEPTH;
    }
    if format.is_stencil() {
        aspect |= vk::ImageAspectFlags::STENCIL;
    }
    aspect
}

pub mod for_macro {
    pub use crate::generic::Constants;

    pub use super::{
        arguments::{descriptor_type, Arguments, ArgumentsField},
        refs::Refs,
    };
    pub use ash::vk::DescriptorUpdateTemplateEntry;
    pub use bytemuck::{Pod, Zeroable};
    pub use std::{
        mem::{align_of, size_of, MaybeUninit},
        ptr::addr_of,
    };

    pub const fn pad_for<T: Constants>(end: usize) -> usize {
        let align = align_of::<T>();
        pad_align(end, align)
    }

    pub const fn pad_align(end: usize, align: usize) -> usize {
        ((end + (align - 1)) & !(align - 1)) - end
    }
}