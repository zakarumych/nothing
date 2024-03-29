use std::collections::VecDeque;

use arcana::{
    blink_alloc::Blink,
    edict::{EntityId, NoSuchEntity, World},
    events::{
        DeviceId, ElementState, Event, EventFilter, KeyboardInput, MouseButton, ScanCode,
        ViewportEvent, VirtualKeyCode,
    },
};
use hashbrown::HashMap;

use crate::ActionQueue;

pub struct InputFilter {
    /// Dispatch events from this device to this controller.
    device: HashMap<DeviceId, Box<dyn Controller>>,

    /// Dispatch events from this window to this controller.
    viewport: HashMap<EntityId, Box<dyn Controller>>,

    /// Dispatch any input event to this controller if
    /// no more specific controller is found for it.
    global: Option<Box<dyn Controller>>,
}

impl EventFilter for InputFilter {
    fn filter(&mut self, _: &Blink, world: &mut World, event: &Event) -> bool {
        self.add_controllers(world);
        self.handle(world, event)
    }
}

impl InputFilter {
    pub fn new() -> Self {
        InputFilter {
            device: HashMap::new(),
            viewport: HashMap::new(),
            global: None,
        }
    }

    pub fn add_controllers(&mut self, world: &mut World) {
        let mut handler = world.expect_resource_mut::<InputHandler>();

        for (bind, controller) in handler.add_controller.drain() {
            match bind {
                ControllerBind::Global => self.global = Some(controller),
                ControllerBind::Device(device) => {
                    self.device.insert(device, controller);
                }
                ControllerBind::Viewport(viewport) => {
                    self.viewport.insert(viewport, controller);
                }
            }
        }
    }

    pub fn handle(&mut self, world: &mut World, event: &Event) -> bool {
        match *event {
            Event::ViewportEvent {
                viewport,
                ref event,
            } => match *event {
                ViewportEvent::KeyboardInput {
                    device_id, input, ..
                } => {
                    if let Some(controller) = self.device.get_mut(&device_id) {
                        controller.on_keyboard_input(world, &input);
                        return true;
                    } else if let Some(controller) = self.viewport.get_mut(&viewport) {
                        controller.on_keyboard_input(world, &input);
                        return true;
                    } else if let Some(controller) = &mut self.global {
                        controller.on_keyboard_input(world, &input);
                        return true;
                    }
                }
                _ => {}
            },
            _ => {}
        }
        false
    }
}

/// Choses which controller to dispatch events to.
pub struct InputHandler {
    add_controller: HashMap<ControllerBind, Box<dyn Controller>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControllerBind {
    Global,
    Device(DeviceId),
    Viewport(EntityId),
}

impl InputHandler {
    #[inline(never)]
    pub fn new() -> Self {
        InputHandler {
            add_controller: HashMap::new(),
        }
    }

    #[inline(never)]
    pub fn add_controller(&mut self, controller: Box<dyn Controller>, bind: ControllerBind) {
        self.add_controller.insert(bind, controller);
    }

    #[inline(never)]
    pub fn add_global_controller(&mut self, controller: Box<dyn Controller>) {
        self.add_controller
            .insert(ControllerBind::Global, controller);
    }

    #[inline(never)]
    pub fn add_device_controller(&mut self, device: DeviceId, controller: Box<dyn Controller>) {
        self.add_controller
            .insert(ControllerBind::Device(device), controller);
    }

    #[inline(never)]
    pub fn add_viewport_controller(&mut self, viewport: EntityId, controller: Box<dyn Controller>) {
        self.add_controller
            .insert(ControllerBind::Viewport(viewport), controller);
    }
}

/// Consumer of input events.
/// When added to InputHandler it may be associated with
/// a specific device or window.
pub trait Controller: Send {
    fn on_keyboard_input(&mut self, world: &mut World, input: &KeyboardInput) {
        let _ = (world, input);
    }
    fn on_mouse_button(&mut self, world: &mut World, button: MouseButton, state: ElementState) {
        let _ = (world, button, state);
    }
    fn on_mouse_move(&mut self, world: &mut World, x: f64, y: f64) {
        let _ = (world, x, y);
    }
}

pub trait Translator: Send {
    type Action;

    fn on_keyboard_input(&mut self, input: &KeyboardInput) -> Option<Self::Action> {
        let _ = input;
        None
    }
    fn on_mouse_button(
        &mut self,
        button: MouseButton,
        state: ElementState,
    ) -> Option<Self::Action> {
        let _ = (button, state);
        None
    }
    fn on_mouse_move(&mut self, x: f64, y: f64) -> Option<Self::Action> {
        let _ = (x, y);
        None
    }
}

pub struct Mapper<A> {
    keyboard_map: HashMap<(VirtualKeyCode, ElementState), A>,
    scancode_map: HashMap<(ScanCode, ElementState), A>,
    mouse_map: HashMap<(MouseButton, ElementState), A>,
    move_map: fn(f64, f64) -> Option<A>,
}

impl<A> Translator for Mapper<A>
where
    A: Clone + Send,
{
    type Action = A;

    fn on_keyboard_input(&mut self, input: &KeyboardInput) -> Option<A> {
        if let Some(action) = input
            .virtual_keycode
            .and_then(|code| self.keyboard_map.get(&(code, input.state)))
        {
            return Some(action.clone());
        }
        if let Some(action) = self.scancode_map.get(&(input.scancode, input.state)) {
            return Some(action.clone());
        }
        None
    }

    fn on_mouse_button(&mut self, button: MouseButton, state: ElementState) -> Option<A> {
        self.mouse_map.get(&(button, state)).cloned()
    }

    fn on_mouse_move(&mut self, x: f64, y: f64) -> Option<A> {
        (self.move_map)(x, y)
    }
}

struct Commander<T> {
    translator: T,
    entity: EntityId,
}

impl<T> Commander<T>
where
    T: Translator,
    T::Action: Send + 'static,
{
    fn send(&self, world: &mut World, action: T::Action) {
        if let Ok(queue) = world.get::<&mut ActionQueue<T::Action>>(self.entity) {
            queue.actions.push_back(action);
            if let Some(waker) = queue.waker.take() {
                waker.wake();
            }
        }
    }
}

impl<T> Controller for Commander<T>
where
    T: Translator,
    T::Action: Send + 'static,
{
    fn on_keyboard_input(&mut self, world: &mut World, input: &KeyboardInput) {
        if let Some(action) = self.translator.on_keyboard_input(input) {
            self.send(world, action);
        }
    }

    fn on_mouse_button(&mut self, world: &mut World, button: MouseButton, state: ElementState) {
        if let Some(action) = self.translator.on_mouse_button(button, state) {
            self.send(world, action);
        }
    }

    fn on_mouse_move(&mut self, world: &mut World, x: f64, y: f64) {
        if let Some(action) = self.translator.on_mouse_move(x, y) {
            self.send(world, action);
        }
    }
}

/// Inserts controller for entity into the world.
///
/// It will use provided translator to convert input events to actions
/// that will be sent to the command queue component of the entity.
pub fn insert_entity_controller<T>(
    translator: T,
    entity: EntityId,
    bind: ControllerBind,
    world: &mut World,
) -> Result<(), NoSuchEntity>
where
    T: Translator + 'static,
    T::Action: Send + 'static,
{
    let commander = Commander { translator, entity };
    let queue = ActionQueue::<T::Action> {
        actions: VecDeque::new(),
        waker: None,
    };
    world.insert(entity, queue)?;
    world
        .expect_resource_mut::<InputHandler>()
        .add_controller(Box::new(commander), bind);
    Ok(())
}

/// Inserts controller for entity into the world.
///
/// It will use provided translator to convert input events to actions
/// that will be sent to the command queue component of the entity.
pub fn insert_global_entity_controller<T>(
    translator: T,
    entity: EntityId,
    world: &mut World,
) -> Result<(), NoSuchEntity>
where
    T: Translator + 'static,
    T::Action: Send + 'static,
{
    insert_entity_controller(translator, entity, ControllerBind::Global, world)
}

pub fn init_world(world: &mut World) {
    world.insert_resource(InputHandler::new());
}
