use arcana::{
    edict::{
        self, spawn_block, ActionEncoder, Component, Entities, EntityId, Res, Scheduler, View,
        World,
    },
    egui::{EguiRender, EguiResource},
    events::{ElementState, KeyboardInput, VirtualKeyCode},
    flow::sleep,
    gametime::{timespan, TimeSpan, TimeSpanNumExt},
    na,
    plugin::{ArcanaPlugin, PluginInit},
    project::{ident, Ident},
    render::RenderGraph,
    winit::window::Window,
};
use camera::Camera2;
use cursor::MainCursor;
use input::{insert_global_entity_controller, ActionQueue, InputHandler, Translator};
use motion::{Motion2, Motor2, MoveAfter2, MoveTo2};
use physics::{geometry::ColliderBuilder, PhysicsResource};
use scene::Global2;
use sdf::SdfRender;

arcana::export_arcana_plugin!(ArcanoidPlugin);

pub struct ArcanoidPlugin;

impl ArcanaPlugin for ArcanoidPlugin {
    arcana::feature_ed! {
        fn dependencies(&self) -> Vec<(&Ident, arcana::project::Dependency)> {
            vec![
                scene::path_dependency(),
                physics::path_dependency(),
                sdf::path_dependency(),
                input::path_dependency(),
                motion::path_dependency(),
                cursor::path_dependency(),
            ]
        }
    }

    fn systems(&self) -> Vec<&Ident> {
        vec![ident!(target_cursor), ident!(paddle_system)]
    }

    fn init(&self, world: &mut World) -> PluginInit {
        let mut init = PluginInit::new();

        let camera = world
            .spawn((Global2::identity(), Camera2::new().with_fovy(15.0)))
            .id();

        {
            let world = world.local();
            let window = world.expect_resource::<Window>().id();
            let mut graph = world.expect_resource_mut::<RenderGraph>();

            // Create main pass.
            // It returns target id that it renders to.
            let mut target = SdfRender::build(camera, &mut graph);

            if world.get_resource::<EguiResource>().is_some() {
                target = EguiRender::build_overlay(target, &mut graph, window);
            }

            // Use window's surface for the render target.
            graph.present(target, window);
        }

        let body = {
            let mut physics = world.expect_resource_mut::<PhysicsResource>();
            let body = physics.new_dynamic_body();

            physics.add_collider(&body, ColliderBuilder::ball(1.0));
            body
        };
        let mut target = world
            .spawn((
                sdf::Shape::circle(1.0).with_color([
                    rand::random(),
                    rand::random(),
                    rand::random(),
                    1.0,
                ]),
                Global2::identity(),
                body,
                MoveTo2::new(na::Point2::new(0.0, 0.0)),
                Motor2::new(30.0, 100.0),
            ))
            .id();

        let init = init.with_system(
            ident!(target_cursor),
            move |cursor: Res<MainCursor>,
                  window: Res<Window>,
                  mut move_to: View<&mut MoveTo2>,
                  cameras: View<(&Camera2, &Global2)>| {
                let inner_position = window.inner_size();
                let ratio = inner_position.width as f32 / inner_position.height as f32;

                let (camera, camera_global) = cameras.try_get(camera).unwrap();

                let position = camera
                    .viewport
                    .transform(1.0, ratio)
                    .transform_point(&cursor.position());

                let position = camera_global.iso.transform_point(&position);
                move_to.try_get_mut(target).unwrap().target = position;
            },
        );

        // insert_global_entity_controller(PaddleTranslator, paddle, world).unwrap();

        let wall_body = {
            let mut physics = world.expect_resource_mut::<PhysicsResource>();
            let body = physics.new_velocity_body();

            physics.add_collider(&body, ColliderBuilder::cuboid(0.2, 5.0));
            // physics.get_body_mut(&body).set_angvel(1.0, false);
            body
        };
        world.spawn((
            sdf::Shape::rect(0.4, 10.0).with_color([0.3, 0.2, 0.1, 1.0]),
            Global2::identity().translated(na::Vector2::new(4.0, 0.0)),
            wall_body,
        ));

        let init = init.with_system(ident!(paddle_system), paddle_system);

        let mut new_node = move |world: &mut World| {
            let body = {
                let mut physics = world.expect_resource_mut::<PhysicsResource>();
                let body = physics.new_dynamic_body();

                physics.add_collider(&body, ColliderBuilder::ball(1.0));
                body
            };
            target = world
                .spawn((
                    sdf::Shape::circle(1.0).with_color([
                        rand::random(),
                        rand::random(),
                        rand::random(),
                        1.0,
                    ]),
                    Global2::from_position(na::Point2::new(
                        rand::random::<f32>() * -10.0,
                        rand::random::<f32>() * 10.0,
                    )),
                    body,
                    MoveAfter2::new(target).with_distance(2.0),
                    Motor2::new(10.0, 100.0),
                ))
                .id();
        };

        spawn_block!(in ref world -> {
            sleep(timespan!(2 seconds), world).await;
            loop {
                sleep(timespan!(1 s), world).await;
                world.with_sync(|world| new_node(world));
            }
        });

        init
    }
}

pub struct PaddleTranslator;

pub enum PaddleAction {
    // PaddleLeft,
    // PaddleRight,
    // PaddleUnLeft,
    // PaddleUnRight,
    Switch,
}

impl Translator for PaddleTranslator {
    type Action = PaddleAction;

    fn on_keyboard_input(&mut self, input: &KeyboardInput) -> Option<PaddleAction> {
        match (input.virtual_keycode, input.state) {
            // (Some(VirtualKeyCode::A), ElementState::Pressed) => Some(PaddleAction::PaddleLeft),
            // (Some(VirtualKeyCode::D), ElementState::Pressed) => Some(PaddleAction::PaddleRight),
            // (Some(VirtualKeyCode::A), ElementState::Released) => Some(PaddleAction::PaddleUnLeft),
            // (Some(VirtualKeyCode::D), ElementState::Released) => Some(PaddleAction::PaddleUnRight),
            (Some(VirtualKeyCode::Space), ElementState::Released) => Some(PaddleAction::Switch),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Component)]
struct PaddleState {
    // left: bool,
    // right: bool,
    chasing: MoveAfter2,
}

fn paddle_system(
    paddles: View<(
        Entities,
        &mut PaddleState,
        Option<&MoveAfter2>,
        &mut ActionQueue<PaddleAction>,
    )>,
    mut encoder: ActionEncoder,
) {
    for (e, state, ma, queue) in paddles {
        if let Some(ma) = ma {
            state.chasing = *ma;
        }

        for action in queue.drain() {
            match action {
                // PaddleAction::PaddleLeft => state.left = true,
                // PaddleAction::PaddleRight => state.right = true,
                // PaddleAction::PaddleUnLeft => state.left = false,
                // PaddleAction::PaddleUnRight => state.right = false,
                PaddleAction::Switch => match ma {
                    None => encoder.insert(e, state.chasing),
                    Some(_) => encoder.drop::<MoveAfter2>(e),
                },
            }
        }

        // let target = match (state.left, state.right) {
        //     (true, true) | (false, false) => {
        //         if move_to.get_mut(e).is_some() {
        //             encoder.drop_bundle::<(MoveTo2, Motion2)>(e);
        //         }
        //         continue;
        //     }
        //     (true, false) => na::Point2::new(-15.0, 12.0),
        //     (false, true) => na::Point2::new(15.0, 12.0),
        // };

        // let m = MoveTo2 {
        //     target,
        //     acceleration: 1.0,
        //     max_velocity: 3.0,
        //     threshold: 0.1,
        // };

        // match move_to.get_mut(e) {
        //     Some(motion) => *motion = m,
        //     None => encoder.insert(e, m),
        // }

        // if global.iso.translation.x < -15.0 {
        //     global.iso.translation.x = -15.0;
        // }
        // if global.iso.translation.x > 15.0 {
        //     global.iso.translation.x = 15.0;
        // }
    }
}
