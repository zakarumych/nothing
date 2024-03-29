use crate::generic::{DeviceError, OutOfMemory};

use super::{CommandBuffer, CommandEncoder};

pub struct Queue {
    device: metal::Device,
    queue: metal::CommandQueue,
}

unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

impl Queue {
    pub(super) fn new(device: metal::Device, queue: metal::CommandQueue) -> Self {
        Queue { device, queue }
    }
}

#[hidden_trait::expose]
impl crate::traits::Queue for Queue {
    fn family(&self) -> u32 {
        0
    }

    fn new_command_encoder(&mut self) -> Result<CommandEncoder, OutOfMemory> {
        Ok(CommandEncoder::new(
            self.device.clone(),
            self.queue.new_command_buffer().to_owned(),
        ))
    }

    fn submit<I>(
        &mut self,
        command_buffers: I,
        _check_point: bool,
    ) -> Result<(), DeviceError<Vec<CommandBuffer>>>
    where
        I: IntoIterator<Item = CommandBuffer>,
    {
        for buffer in command_buffers {
            buffer.commit();
        }
        Ok(())
    }

    /// Drop command buffers without submitting them to the queue.
    fn drop_command_buffer<I>(&mut self, command_buffers: I)
    where
        I: IntoIterator<Item = CommandBuffer>,
    {
        command_buffers.into_iter().for_each(drop);
    }
}
