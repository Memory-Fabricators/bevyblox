//! Humanoid-style walking on Rapier colliders. Constants come from the legacy source:
//! `Humanoid::walkSpeed` 16 studs/s (Humanoid.cpp:309), `Jumping::kJumpVelocityGrid` 50 studs/s
//! (Jumping.h:37), gravity 9.81 m/s^2 at 20 studs/m = 196.2 studs/s^2 (Constants.h, Units.h).
//! The 2x5-stud character (1-stud head, 2-stud torso, 2-stud legs) is modelled as one capsule.
use bevy::{prelude::*, transform::TransformSystems};
use bevy_rapier3d::prelude::*;

use crate::first_person::{FirstPersonCamera, Locomotion};

pub const WALK_SPEED: f32 = 16.0;
pub const JUMP_VELOCITY: f32 = 50.0;
pub const GRAVITY: f32 = 196.2;
/// Head center above the feet: legs 2 + torso 2 + half the 1-stud head.
pub const EYE_HEIGHT: f32 = 4.5;
const CAPSULE_RADIUS: f32 = 1.0;
const CAPSULE_HALF_SEGMENT: f32 = 1.5;
/// Capsule center above the feet (total height 5).
const CENTER_HEIGHT: f32 = CAPSULE_HALF_SEGMENT + CAPSULE_RADIUS;
/// Longest step integrated at once, so a stalled frame cannot tunnel through thin parts.
const MAX_STEP: f32 = 1.0 / 20.0;

pub struct WalkPlugin;

impl Plugin for WalkPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
            .add_systems(
                Update,
                drive_walker.after(crate::first_person::control_camera),
            )
            .add_systems(
                PostUpdate,
                follow_walker
                    .after(PhysicsSet::Writeback)
                    .before(TransformSystems::Propagate),
            );
    }
}

#[derive(Component, Default)]
pub struct Walker {
    vertical_velocity: f32,
    active: bool,
}

pub fn spawn_walker(world: &mut World) {
    world.spawn((
        Name::new("Walker"),
        Walker::default(),
        Transform::default(),
        RigidBody::KinematicPositionBased,
        Collider::capsule_y(CAPSULE_HALF_SEGMENT, CAPSULE_RADIUS),
        KinematicCharacterController {
            // Roblox humanoids climb up to a 89 degree slope; sliding is never forced.
            max_slope_climb_angle: 89.0_f32.to_radians(),
            min_slope_slide_angle: 89.0_f32.to_radians(),
            // A step the height of a stair; the legacy humanoid raycasts its legs instead.
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(1.0),
                min_width: CharacterLength::Absolute(0.25),
                include_dynamic_bodies: false,
            }),
            snap_to_ground: Some(CharacterLength::Absolute(0.5)),
            ..default()
        },
    ));
}

/// Unit-size static collider for a part, or `None` if its mesh cannot produce one. The part's
/// size goes on the entity's `Transform::scale`; bevy_rapier re-derives the collider scale from
/// it every frame, so scaling the shape directly would be silently undone.
pub fn part_collider(geometry: &blox_scene::Geometry, mesh: &Mesh) -> Option<Collider> {
    use blox_scene::Geometry;
    Some(match geometry {
        Geometry::Block => Collider::cuboid(0.5, 0.5, 0.5),
        Geometry::Ball => Collider::ball(0.5),
        // Mesh parts are arbitrary shapes; everything else is a convex solid.
        Geometry::File(_) => {
            Collider::from_bevy_mesh(mesh, &ComputedColliderShape::TriMesh(Default::default()))?
        }
        _ => Collider::from_bevy_mesh(mesh, &ComputedColliderShape::ConvexHull)?,
    })
}

fn drive_walker(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mode: Res<Locomotion>,
    camera: Single<(&FirstPersonCamera, &Transform), Without<Walker>>,
    walker: Single<(
        &mut Walker,
        &mut Transform,
        &mut KinematicCharacterController,
        Option<&KinematicCharacterControllerOutput>,
    )>,
) {
    let (view, view_pose) = *camera;
    let (mut walker, mut pose, mut controller, output) = walker.into_inner();
    if *mode != Locomotion::Walk {
        walker.active = false;
        controller.translation = None;
        return;
    }
    if !walker.active {
        // Entering walk mode (or starting): stand where the camera is, at rest.
        pose.translation = view_pose.translation - Vec3::Y * (EYE_HEIGHT - CENTER_HEIGHT);
        walker.vertical_velocity = 0.0;
        walker.active = true;
    }
    let dt = time.delta_secs().min(MAX_STEP);
    let grounded = output.is_some_and(|o| o.grounded);
    let input = |positive: &[KeyCode], negative: &[KeyCode]| {
        if !view.captured() {
            return 0.0;
        }
        f32::from(u8::from(keys.any_pressed(positive.iter().copied())))
            - f32::from(u8::from(keys.any_pressed(negative.iter().copied())))
    };
    let wish = Vec3::new(
        input(
            &[KeyCode::KeyD, KeyCode::ArrowRight],
            &[KeyCode::KeyA, KeyCode::ArrowLeft],
        ),
        0.0,
        input(
            &[KeyCode::KeyS, KeyCode::ArrowDown],
            &[KeyCode::KeyW, KeyCode::ArrowUp],
        ),
    );
    // Humanoid movement is unit length: diagonals are not faster.
    let direction = Quat::from_rotation_y(view.yaw()) * wish.normalize_or_zero();
    // Head bump: stop rising when the last step was blocked from above.
    if output.is_some_and(|o| o.effective_translation.y < o.desired_translation.y - 1e-4)
        && walker.vertical_velocity > 0.0
    {
        walker.vertical_velocity = 0.0;
    }
    if grounded && walker.vertical_velocity < 0.0 {
        walker.vertical_velocity = 0.0;
    }
    if grounded && view.captured() && keys.pressed(KeyCode::Space) {
        walker.vertical_velocity = JUMP_VELOCITY;
    }
    walker.vertical_velocity -= GRAVITY * dt;
    controller.translation =
        Some((direction * WALK_SPEED + Vec3::Y * walker.vertical_velocity) * dt);
    // Rapier writes the collision-resolved movement into `pose` during this frame's step.
}

fn follow_walker(
    mode: Res<Locomotion>,
    walker: Single<&Transform, (With<Walker>, Without<FirstPersonCamera>)>,
    mut camera: Single<&mut Transform, (With<FirstPersonCamera>, Without<Walker>)>,
) {
    if *mode == Locomotion::Walk {
        camera.translation = walker.translation + Vec3::Y * (EYE_HEIGHT - CENTER_HEIGHT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    const STEP: f64 = 1.0 / 60.0;

    fn world(extra_boxes: &[(Vec3, Vec3)]) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            STEP,
        )))
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(Locomotion::Walk)
        .add_plugins(WalkPlugin);
        let mut boxes = vec![(Vec3::new(0.0, -0.5, 0.0), Vec3::new(200.0, 1.0, 200.0))];
        boxes.extend_from_slice(extra_boxes);
        for (center, size) in boxes {
            app.world_mut().spawn((
                Transform::from_translation(center).with_scale(size),
                Collider::cuboid(0.5, 0.5, 0.5),
            ));
        }
        let mut view = FirstPersonCamera::new(&Transform::IDENTITY, Vec3::ZERO, 10.0);
        view.set_captured(true);
        let camera = app
            .world_mut()
            .spawn((view, Transform::from_xyz(0.0, 10.0, 0.0)))
            .id();
        spawn_walker(app.world_mut());
        (app, camera)
    }

    fn walker(app: &mut App) -> Vec3 {
        app.world_mut()
            .query_filtered::<&Transform, With<Walker>>()
            .single(app.world())
            .unwrap()
            .translation
    }

    fn run(app: &mut App, seconds: f64) {
        for _ in 0..(seconds / STEP).round() as usize {
            app.update();
        }
    }

    #[test]
    fn falls_onto_ground_and_camera_rides_at_head_height() {
        let (mut app, camera) = world(&[]);
        run(&mut app, 3.0);
        let feet = walker(&mut app) - Vec3::Y * CENTER_HEIGHT;
        assert!(feet.y.abs() < 0.1, "feet rest on the ground, got {feet}");
        let eye = app.world().get::<Transform>(camera).unwrap().translation;
        assert!((eye.y - EYE_HEIGHT).abs() < 0.1, "eye height {}", eye.y);
    }

    #[test]
    fn walks_at_sixteen_studs_per_second_without_diagonal_boost() {
        let (mut app, _) = world(&[]);
        run(&mut app, 3.0);
        let start = walker(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        run(&mut app, 1.0);
        let forward = walker(&mut app) - start;
        assert!(
            (forward.z + WALK_SPEED).abs() < 0.5,
            "forward travel {forward}"
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyD);
        let start = walker(&mut app);
        run(&mut app, 1.0);
        let diagonal = walker(&mut app) - start;
        assert!(
            (diagonal.xz().length() - WALK_SPEED).abs() < 0.5,
            "diagonal travel {diagonal}"
        );
    }

    #[test]
    fn jump_reaches_the_height_implied_by_legacy_constants_and_walls_block() {
        let (mut app, _) = world(&[(Vec3::new(0.0, 5.0, -12.0), Vec3::new(20.0, 10.0, 2.0))]);
        run(&mut app, 3.0);
        let ground = walker(&mut app).y;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        let mut peak = ground;
        for _ in 0..90 {
            app.update();
            peak = peak.max(walker(&mut app).y);
        }
        let expected = JUMP_VELOCITY * JUMP_VELOCITY / (2.0 * GRAVITY);
        assert!(
            (peak - ground - expected).abs() < 0.7,
            "peak {} expected {expected}",
            peak - ground
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::Space);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        run(&mut app, 3.0);
        // The wall's near face is at z = -11; the capsule radius keeps the center 1 stud away.
        let z = walker(&mut app).z;
        assert!(z > -10.5 && z < -9.5, "stopped at {z}");
    }
}
