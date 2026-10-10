use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use blox_model::{Model, ModelError, ModelInstance, ModelResult, Property};

// Camera.cpp::onMousePan / tiltRadians / doFly and util/NavKeys.h.
const YAW_PER_PIXEL: f32 = -0.4_f32.to_radians();
const PITCH_PER_PIXEL: f32 = -0.3_f32.to_radians();
const PITCH_LIMIT: f32 = std::f32::consts::PI * 9.0 / 20.0;
// The legacy Workspace heartbeat runs at Constants::uiStepsPerSec() = 2 * 30 = 60 Hz and calls
// doFly once per beat, so 1.5 studs per call is 90 studs/second. Held-key acceleration counts
// those calls (steps > 60 means after one second; the multiplier is steps / 60, capped at 30).
const BASE_SPEED: f32 = 90.0;
const ACCELERATION_DELAY: f32 = 1.0;
const ACCELERATION_CAP: f32 = 30.0;
const SHIFT_FACTOR: f32 = 0.2;

/// `F` toggles between the humanoid-style walker (`walk.rs`) and the legacy `Camera::doFly`.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locomotion {
    #[default]
    Walk,
    Fly,
}

pub struct FirstPersonPlugin;

impl Plugin for FirstPersonPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Locomotion>()
            .add_systems(Update, control_camera);
    }
}

#[derive(Component)]
pub struct FirstPersonCamera {
    yaw: f32,
    pitch: f32,
    held_seconds: f32,
    captured: bool,
    center: Vec3,
    radius: f32,
}

impl FirstPersonCamera {
    pub fn new(pose: &Transform, center: Vec3, radius: f32) -> Self {
        let forward = pose.rotation * Vec3::NEG_Z;
        Self {
            yaw: (-forward.x).atan2(-forward.z),
            pitch: forward.y.clamp(-1.0, 1.0).asin(),
            held_seconds: 0.0,
            captured: false,
            center,
            radius,
        }
    }

    fn advance(&mut self, pose: &mut Transform, mouse: Vec2, movement: Vec3, slow: bool, dt: f32) {
        if mouse != Vec2::ZERO {
            self.yaw = (self.yaw + mouse.x * YAW_PER_PIXEL).rem_euclid(std::f32::consts::TAU);
            self.pitch = (self.pitch + mouse.y * PITCH_PER_PIXEL).clamp(-PITCH_LIMIT, PITCH_LIMIT);
            pose.rotation = Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch);
        }
        if movement == Vec3::ZERO {
            self.held_seconds = 0.0;
            return;
        }
        // Integrate the held-key speed curve analytically so travel is independent of frame rate.
        let start = self.held_seconds;
        self.held_seconds = (start + dt).min(3600.0);
        let distance = if slow {
            BASE_SPEED * SHIFT_FACTOR * dt
        } else {
            BASE_SPEED * (travel_curve(start + dt) - travel_curve(start))
        };
        // Legacy doFly adds the camera's look/right/up independently (no diagonal normalization).
        pose.translation += pose.rotation * movement * distance;
    }

    #[cfg(test)]
    pub fn set_captured(&mut self, captured: bool) {
        self.captured = captured;
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn captured(&self) -> bool {
        self.captured
    }

    pub fn far(&self, position: Vec3) -> f32 {
        (position.distance(self.center) + self.radius * 2.0).max(1000.0)
    }
}

/// Distance in base-speed units after holding a key for `t` seconds: multiplier 1 for the first
/// second, then `steps / 60 == t`, capped at `ACCELERATION_CAP`.
fn travel_curve(t: f32) -> f32 {
    let capped_at = ACCELERATION_CAP;
    if t <= ACCELERATION_DELAY {
        t
    } else if t <= capped_at {
        ACCELERATION_DELAY + (t * t - ACCELERATION_DELAY * ACCELERATION_DELAY) * 0.5
    } else {
        travel_curve(capped_at) + (t - capped_at) * ACCELERATION_CAP
    }
}

fn axis(positive: bool, negative: bool) -> f32 {
    f32::from(u8::from(positive)) - f32::from(u8::from(negative))
}

pub fn control_camera(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    mut mode: ResMut<Locomotion>,
    mut window: Single<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut camera: Single<(&mut FirstPersonCamera, &mut Transform, &mut Projection)>,
) {
    let (window, cursor) = &mut *window;
    let (controller, pose, projection) = &mut *camera;
    let was_captured = controller.captured;
    if !window.focused || keys.just_pressed(KeyCode::Escape) {
        controller.captured = false;
    } else if buttons.just_pressed(MouseButton::Left) {
        controller.captured = true;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        *mode = match *mode {
            Locomotion::Walk => Locomotion::Fly,
            Locomotion::Fly => Locomotion::Walk,
        };
    }
    if was_captured != controller.captured {
        cursor.visible = !controller.captured;
        cursor.grab_mode = if controller.captured {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        controller.held_seconds = 0.0;
    }
    if !controller.captured {
        controller.held_seconds = 0.0;
        return;
    }
    let movement = if *mode == Locomotion::Fly {
        Vec3::new(
            axis(
                keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight),
                keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft),
            ),
            axis(keys.pressed(KeyCode::KeyE), keys.pressed(KeyCode::KeyQ)),
            axis(
                keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown),
                keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp),
            ),
        )
    } else {
        Vec3::ZERO
    };
    // Do not apply the click-to-lock frame's accumulated motion (which predates capture).
    let mouse = if was_captured {
        motion.delta
    } else {
        Vec2::ZERO
    };
    if mouse == Vec2::ZERO && movement == Vec3::ZERO {
        controller.held_seconds = 0.0;
        return;
    }
    controller.advance(
        pose,
        mouse,
        movement,
        keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        time.delta_secs(),
    );
    if let Projection::Perspective(projection) = &mut **projection {
        projection.far = controller.far(pose.translation);
    }
}

#[derive(Resource)]
pub struct InitialView {
    pub pose: Transform,
    pub fov: f32,
    /// Eye position above the first `SpawnLocation`, where the walker starts.
    pub spawn_eye: Option<Vec3>,
}

impl InitialView {
    pub fn from_model(model: &Model, center: Vec3) -> ModelResult<Self> {
        let selected = model
            .instances
            .iter()
            .find(|i| i.class == "Workspace")
            .and_then(|workspace| workspace.properties.get("CurrentCamera"))
            .and_then(|property| match property {
                Property::Ref(Some(id)) => Some(id),
                _ => None,
            })
            .and_then(|id| {
                model
                    .instances
                    .iter()
                    .find(|i| i.referent == *id && i.class == "Camera")
            });
        if let Some(camera) =
            selected.or_else(|| model.instances.iter().find(|i| i.class == "Camera"))
        {
            if let Some(pose) = instance_pose(camera)? {
                let fov = match camera.properties.get("FieldOfView") {
                    Some(Property::Float(v)) => *v as f32,
                    Some(Property::Int(v)) => *v as f32,
                    Some(Property::Uint(v)) => *v as f32,
                    None => 70.0,
                    _ => {
                        return Err(ModelError(
                            "Camera FieldOfView has an incompatible type".into(),
                        ));
                    }
                };
                if !fov.is_finite() || !(1.0..=120.0).contains(&fov) {
                    return Err(ModelError(
                        "Camera FieldOfView must be between 1 and 120 degrees".into(),
                    ));
                }
                return Ok(Self {
                    pose,
                    fov: fov.to_radians(),
                    spawn_eye: spawn_eye(model)?,
                });
            }
        }
        // No serialized view: start near a real spawn, not at a map-sized orbit distance.
        for spawn in model
            .instances
            .iter()
            .filter(|i| i.class == "SpawnLocation")
        {
            if let Some(mut pose) = instance_pose(spawn)? {
                pose.translation += Vec3::Y * 5.0;
                return Ok(Self {
                    pose,
                    fov: 70.0_f32.to_radians(),
                    spawn_eye: spawn_eye(model)?,
                });
            }
        }
        // Camera.cpp constructor's (0,20,20) offset, relative to a standalone model's center.
        Ok(Self {
            pose: Transform::from_translation(center + Vec3::new(0.0, 20.0, 20.0))
                .looking_at(center, Vec3::Y),
            fov: 70.0_f32.to_radians(),
            spawn_eye: None,
        })
    }
}

/// Feet rest on the spawn's top face; the eye is at head height (Humanoid head center).
fn spawn_eye(model: &Model) -> ModelResult<Option<Vec3>> {
    for spawn in model
        .instances
        .iter()
        .filter(|i| i.class == "SpawnLocation")
    {
        let Some(pose) = instance_pose(spawn)? else {
            continue;
        };
        let height = match spawn
            .properties
            .get("size")
            .or_else(|| spawn.properties.get("Size"))
        {
            Some(Property::Vector3(size)) => size[1],
            _ => 1.2,
        };
        return Ok(Some(
            pose.translation + Vec3::Y * (height * 0.5 + crate::walk::EYE_HEIGHT + 0.5),
        ));
    }
    Ok(None)
}

fn instance_pose(instance: &ModelInstance) -> ModelResult<Option<Transform>> {
    let property = instance
        .properties
        .get("CFrame")
        .or_else(|| instance.properties.get("CoordinateFrame"));
    let Some(property) = property else {
        return Ok(None);
    };
    let Property::CFrame {
        position,
        rotation: matrix,
    } = property
    else {
        return Err(ModelError(format!(
            "{} CFrame has an incompatible type",
            instance.class
        )));
    };
    let position = *position;
    let matrix = *matrix;
    if !position.is_finite() || !matrix.is_finite() || (matrix.determinant() - 1.0).abs() > 0.02 {
        return Err(ModelError(format!(
            "{} has an invalid camera/spawn CFrame",
            instance.class
        )));
    }
    Ok(Some(
        Transform::from_translation(position).with_rotation(Quat::from_mat3(&matrix).normalize()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, time::Duration};

    fn controller() -> (FirstPersonCamera, Transform) {
        let pose = Transform::IDENTITY;
        (FirstPersonCamera::new(&pose, Vec3::ZERO, 10.0), pose)
    }

    #[test]
    fn looking_rotates_in_place_with_native_directions_and_pitch_limits() {
        let (mut camera, mut pose) = controller();
        pose.translation = Vec3::new(10.0, 20.0, 30.0);
        camera.advance(&mut pose, Vec2::new(20.0, -10.0), Vec3::ZERO, false, 0.1);
        assert_eq!(pose.translation, Vec3::new(10.0, 20.0, 30.0));
        let forward = pose.rotation * Vec3::NEG_Z;
        assert!(forward.x > 0.0 && forward.y > 0.0 && forward.z < 0.0);
        camera.advance(&mut pose, Vec2::new(0.0, -10000.0), Vec3::ZERO, false, 0.1);
        assert!((camera.pitch - PITCH_LIMIT).abs() < 1e-6);
        camera.advance(&mut pose, Vec2::new(0.0, 10000.0), Vec3::ZERO, false, 0.1);
        assert!((camera.pitch + PITCH_LIMIT).abs() < 1e-6);
    }

    #[test]
    fn flight_is_camera_relative_and_shift_is_precision_not_sprint() {
        let (mut camera, mut pose) = controller();
        pose.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        camera.advance(&mut pose, Vec2::ZERO, Vec3::NEG_Z, false, 1.0);
        assert!(
            pose.translation
                .abs_diff_eq(Vec3::new(-90.0, 0.0, 0.0), 1e-4)
        );
        let (mut camera, mut pose) = controller();
        camera.advance(&mut pose, Vec2::ZERO, Vec3::NEG_Z, true, 1.0);
        assert_eq!(pose.translation, Vec3::new(0.0, 0.0, -18.0));
        camera.advance(&mut pose, Vec2::ZERO, Vec3::Y, true, 1.0);
        assert_eq!(pose.translation, Vec3::new(0.0, 18.0, -18.0));
    }

    #[test]
    fn acceleration_crossings_are_independent_of_frame_partition_and_reset_on_idle() {
        let (mut whole, mut a) = controller();
        let (mut split, mut b) = controller();
        whole.advance(&mut a, Vec2::ZERO, Vec3::NEG_Z, false, 65.0);
        for _ in 0..260 {
            split.advance(&mut b, Vec2::ZERO, Vec3::NEG_Z, false, 0.25);
        }
        assert!(a.translation.abs_diff_eq(b.translation, 0.01));
        split.advance(&mut b, Vec2::ZERO, Vec3::ZERO, false, 0.25);
        let start = b.translation;
        split.advance(&mut b, Vec2::ZERO, Vec3::NEG_Z, false, 1.0);
        assert_eq!(b.translation - start, Vec3::new(0.0, 0.0, -90.0));
    }

    fn instance(class: &str, id: &str, properties: HashMap<String, Property>) -> ModelInstance {
        ModelInstance {
            class: class.into(),
            referent: id.into(),
            name: class.into(),
            parent: None,
            properties,
        }
    }

    #[test]
    fn workspace_current_camera_wins_and_legacy_pose_is_not_reframed() {
        let frame = Property::CFrame {
            position: Vec3::new(123.0, 4.0, -567.0),
            rotation: Mat3::from_cols(
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(1.0, 0.0, 0.0),
            ),
        };
        let model = Model {
            instances: vec![
                instance("Camera", "unused", HashMap::new()),
                instance(
                    "Workspace",
                    "workspace",
                    HashMap::from([("CurrentCamera".into(), Property::Ref(Some("active".into())))]),
                ),
                instance(
                    "Camera",
                    "active",
                    HashMap::from([
                        ("CoordinateFrame".into(), frame),
                        ("FieldOfView".into(), Property::Float(80.0)),
                    ]),
                ),
            ],
        };
        let view = InitialView::from_model(&model, Vec3::splat(1e6)).unwrap();
        assert_eq!(view.pose.translation, Vec3::new(123.0, 4.0, -567.0));
        assert!((view.fov.to_degrees() - 80.0).abs() < 1e-4);
        assert!((view.pose.rotation * Vec3::NEG_Z).abs_diff_eq(Vec3::NEG_X, 1e-5));
    }

    #[test]
    fn escape_and_focus_loss_release_capture_and_stop_movement() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .insert_resource(Locomotion::Fly)
            .insert_resource(Time::<()>::default())
            .add_plugins(FirstPersonPlugin);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(0.1));
        let window = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                CursorOptions::default(),
                PrimaryWindow,
            ))
            .id();
        let (camera, pose) = controller();
        let entity = app
            .world_mut()
            .spawn((camera, pose, Projection::default()))
            .id();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::ZERO
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        let moved = app.world().get::<Transform>(entity).unwrap().translation;
        assert!(moved.z < 0.0);
        assert_eq!(
            app.world().get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::Locked
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            moved
        );
        assert_eq!(
            app.world().get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::None
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        let moved = app.world().get::<Transform>(entity).unwrap().translation;
        app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            moved
        );
        assert!(app.world().get::<CursorOptions>(window).unwrap().visible);
        assert!(
            !app.world()
                .get::<FirstPersonCamera>(entity)
                .unwrap()
                .captured
        );
    }
}
