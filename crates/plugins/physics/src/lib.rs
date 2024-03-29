use amity::flip_queue::FlipQueue;
use arcana::edict::{self, Component, EntityId, ResMut, State, View};

use rapier2d::{
    dynamics::{
        CCDSolver, ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet,
        RigidBody, RigidBodySet,
    },
    geometry::{
        BroadPhase, Collider, ColliderHandle, ColliderSet, CollisionEvent, ContactPair, NarrowPhase,
    },
    pipeline::PhysicsPipeline,
};
use scene::Global2;

pub use rapier2d::{dynamics, geometry};

arcana::export_arcana_plugin! {
    PhysicsPlugin {
        dependencies: [scene ...],
        resources: [PhysicsResource::new()],
        components: [Body],
        systems: [physics_system],
    }
}

/// Component that represents a physics body.
/// Use `PhysicsResource` to create bodies.
/// Use `PhysicsResource::add_collider` to add colliders to bodies.
#[derive(Debug, Component)]
#[edict(name = "Body")]
pub struct Body {
    handle: rapier2d::dynamics::RigidBodyHandle,
}

/// Payload of the collistion event.
/// Contains collider index, other body entity id and other collider index.
#[derive(Debug)]
pub struct Collision {
    collider: usize,
    other_entity: EntityId,
    other_collider: usize,
}

#[derive(Debug)]
pub enum CollisionState {
    Started,
    Stopped,
}

#[derive(Debug, Component)]
#[edict(name = "CollisionEvents")]
pub struct CollisionEvents {
    events: Vec<(CollisionState, Collision)>,
}

pub struct PhysicsResource {
    pipeline: PhysicsPipeline,
    parameters: IntegrationParameters,
    islands: IslandManager,
    broad_phase: BroadPhase,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    this_is_valid: &'static str,
}

impl PhysicsResource {
    fn new() -> Self {
        PhysicsResource {
            pipeline: PhysicsPipeline::new(),
            parameters: IntegrationParameters::default(),
            islands: IslandManager::new(),
            broad_phase: BroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            this_is_valid: "this is valid",
        }
    }

    pub fn new_fixed_body(&mut self) -> Body {
        Body {
            handle: self.bodies.insert(
                rapier2d::dynamics::RigidBodyBuilder::fixed()
                    .translation(na::Vector2::zeros())
                    .build(),
            ),
        }
    }

    pub fn new_position_body(&mut self) -> Body {
        Body {
            handle: self.bodies.insert(
                rapier2d::dynamics::RigidBodyBuilder::kinematic_position_based()
                    .translation(na::Vector2::zeros())
                    .build(),
            ),
        }
    }

    pub fn new_velocity_body(&mut self) -> Body {
        Body {
            handle: self.bodies.insert(
                rapier2d::dynamics::RigidBodyBuilder::kinematic_velocity_based()
                    .translation(na::Vector2::zeros())
                    .build(),
            ),
        }
    }

    pub fn new_dynamic_body(&mut self) -> Body {
        Body {
            handle: self.bodies.insert(
                rapier2d::dynamics::RigidBodyBuilder::dynamic()
                    .translation(na::Vector2::zeros())
                    .build(),
            ),
        }
    }

    pub fn add_collider(&mut self, body: &Body, collider: impl Into<Collider>) -> ColliderHandle {
        self.colliders
            .insert_with_parent(collider, body.handle, &mut self.bodies)
    }

    pub fn get_body(&self, body: &Body) -> &RigidBody {
        self.bodies.get(body.handle).unwrap()
    }

    pub fn get_body_mut(&mut self, body: &Body) -> &mut RigidBody {
        self.bodies.get_mut(body.handle).unwrap()
    }
}

#[derive(Default)]
pub struct PhysicsState {
    new_events: FlipQueue<CollisionEvent>,
}

fn physics_system(
    mut res: ResMut<PhysicsResource>,
    mut bodies: View<(&Body, &mut Global2)>,
    mut state: State<PhysicsState>,
) {
    for (body, global) in bodies.iter_mut() {
        let rb = res.bodies.get_mut(body.handle).unwrap();
        rb.set_position(global.iso, true);
    }

    let res = &mut *res;
    res.pipeline.step(
        &na::Vector2::new(0.0, -9.81),
        &res.parameters,
        &mut res.islands,
        &mut res.broad_phase,
        &mut res.narrow_phase,
        &mut res.bodies,
        &mut res.colliders,
        &mut res.impulse_joints,
        &mut res.multibody_joints,
        &mut res.ccd_solver,
        None,
        &(),
        &EventHandler {
            new_events: &state.new_events,
        },
    );

    for (body, global) in bodies.iter_mut() {
        let rb = res.bodies.get_mut(body.handle).unwrap();
        global.iso = *rb.position();
        rb.reset_forces(false);
    }

    for collision in state.new_events.drain() {
        todo!();
    }
}

struct EventHandler<'a> {
    new_events: &'a FlipQueue<CollisionEvent>,
}

impl<'a> rapier2d::pipeline::EventHandler for EventHandler<'a> {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        self.new_events.push(event);
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: f32,
    ) {
    }
}
