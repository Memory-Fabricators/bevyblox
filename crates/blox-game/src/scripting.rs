//! Connects the Luau script host to the viewer: starts scripts once the scene is up, sends clicks
//! and hovers along the camera ray, advances `wait`s, and applies the part moves scripts make.

use std::collections::HashMap;

use bevy::{prelude::*, window::PrimaryWindow};
use blox_scene::PartSnapshot;
use blox_script::{Edit, ScriptHost};

use crate::first_person::{FirstPersonCamera, control_camera};

/// Marks an entity drawn from snapshot part `.0`, so a script move reaches the mesh, collider,
/// and decals of that part.
#[derive(Component, Clone, Copy, Debug)]
pub struct PartLink(pub usize);

/// Maps model instance indices (what scripts edit) to snapshot parts.
#[derive(Resource)]
pub struct PartLinks {
    part_of_instance: HashMap<usize, usize>,
    mesh_offsets: Vec<Vec3>,
}

impl PartLinks {
    pub fn new(parts: &[PartSnapshot]) -> Self {
        Self {
            part_of_instance: parts
                .iter()
                .enumerate()
                .map(|(part, p)| ((p.instance_id - 1) as usize, part))
                .collect(),
            mesh_offsets: parts.iter().map(|p| p.mesh_offset).collect(),
        }
    }
}

pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        // Runs before `control_camera` so a click that captures the mouse does not also fire a
        // ClickDetector: clicks only count while the mouse was already captured.
        app.add_systems(Startup, start_scripts)
            .add_systems(Update, script_frame.before(control_camera));
    }
}

fn start_scripts(mut host: NonSendMut<ScriptHost>) {
    host.start_scripts();
}

fn script_frame(
    mut host: NonSendMut<ScriptHost>,
    time: Res<Time>,
    links: Res<PartLinks>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform, &FirstPersonCamera), With<Camera3d>>,
    mut parts: Query<(&PartLink, &mut Transform)>,
) {
    let (camera, pose, controller) = &*camera;
    let clicked = buttons.just_pressed(MouseButton::Left);
    if controller.captured() {
        // Captured: the view center is the pointer, as in the original's mouse-look.
        let (origin, direction) = (pose.translation(), Vec3::from(pose.forward()));
        if clicked {
            host.click(origin, direction);
        }
        host.hover(origin, direction);
    } else if let Some(ray) = window
        .cursor_position()
        .and_then(|p| camera.viewport_to_world(pose, p).ok())
    {
        // Free cursor, as in the original: a click on a ClickDetector under the pointer fires
        // it. Consuming the press keeps control_camera from capturing the mouse for it.
        let (origin, direction) = (ray.origin, Vec3::from(ray.direction));
        if clicked && host.click(origin, direction) {
            buttons.clear_just_pressed(MouseButton::Left);
        }
        host.hover(origin, direction);
    }
    host.tick(time.delta_secs());
    for edit in host.take_edits() {
        match edit {
            Edit::PartMoved {
                index,
                position,
                rotation,
            } => {
                let Some(&part) = links.part_of_instance.get(&index) else {
                    continue;
                };
                let translation = position + rotation * links.mesh_offsets[part];
                let rotation = Quat::from_mat3(&rotation).normalize();
                for (link, mut transform) in &mut parts {
                    if link.0 == part {
                        transform.translation = translation;
                        transform.rotation = rotation;
                    }
                }
            }
            // No audio backend yet. The map's music Sounds have an empty SoundId, so there is
            // nothing to play; report the call rather than dropping it.
            Edit::SoundPlay(index) => {
                eprintln!("Sound {index}: Play ignored, audio playback is not implemented")
            }
            Edit::SoundStop(index) => {
                eprintln!("Sound {index}: Stop ignored, audio playback is not implemented")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::first_person::FirstPersonCamera;
    use blox_model::{Model, Property};
    use blox_scene::SceneSnapshot;
    use std::path::PathBuf;

    /// Center, rotation and size of the first part with a ClickDetector and a door script.
    fn door_button(model: &Model) -> Option<(Vec3, Mat3, Vec3)> {
        let all = &model.instances;
        all.iter().find_map(|d| {
            if d.class != "ClickDetector" {
                return None;
            }
            let part_ref = d.parent.as_deref()?;
            let has_door = all.iter().any(|s| {
                s.class == "Script"
                    && s.parent.as_deref() == Some(part_ref)
                    && matches!(s.properties.get("Source"), Some(Property::Text(t)) if t.contains("door"))
            });
            if !has_door {
                return None;
            }
            let part = all.iter().find(|i| i.referent == part_ref)?;
            let Some(Property::CFrame { position, rotation }) = part.properties.get("CFrame") else {
                return None;
            };
            let Some(Property::Vector3(size)) = part.properties.get("size") else {
                return None;
            };
            Some((*position, *rotation, *size))
        })
    }

    #[test]
    fn captured_click_moves_door_part() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let model = Model::load(root.join("rust/examples/area51_slenderman.rbxl")).unwrap();
        let snapshot = SceneSnapshot::from_model(&model, &root.join("content")).unwrap();
        let (center, rotation, size) = door_button(&model).expect("door button");
        let links = PartLinks::new(&snapshot.parts);
        let host = ScriptHost::new(model).unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(links)
            .insert_non_send(host)
            .add_systems(Startup, start_scripts)
            .add_systems(Update, script_frame);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        for (k, part) in snapshot.parts.iter().enumerate() {
            let transform = Transform {
                translation: part.position,
                rotation: part.rotation,
                scale: Vec3::ONE,
            };
            app.world_mut().spawn((PartLink(k), transform));
        }
        let pose = Transform::IDENTITY;
        let mut controller = FirstPersonCamera::new(&pose, Vec3::ZERO, 10.0);
        controller.set_captured(true);
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera::default(),
                pose,
                GlobalTransform::from(pose),
                controller,
            ))
            .id();

        let half = size * 0.5;
        let mut moved_axis = None;
        for axis in [
            Vec3::Z,
            Vec3::NEG_Z,
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
        ] {
            let extent = (rotation.transpose() * axis).abs().dot(half);
            let origin = center + axis * (extent + 3.0);
            let up = if axis.y.abs() > 0.5 { Vec3::Z } else { Vec3::Y };
            let pose = Transform::from_translation(origin).looking_to(-axis, up);
            let mut controller = FirstPersonCamera::new(&pose, Vec3::ZERO, 10.0);
            controller.set_captured(true);
            app.world_mut().entity_mut(camera).insert((
                pose,
                GlobalTransform::from(pose),
                controller,
            ));
            let before: Vec<Vec3> = app
                .world_mut()
                .query::<(&PartLink, &Transform)>()
                .iter(app.world())
                .map(|(_, t)| t.translation)
                .collect();
            {
                let mut buttons = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
                buttons.clear();
                buttons.press(MouseButton::Left);
            }
            app.update();
            let after: Vec<Vec3> = app
                .world_mut()
                .query::<(&PartLink, &Transform)>()
                .iter(app.world())
                .map(|(_, t)| t.translation)
                .collect();
            if before != after {
                moved_axis = Some(axis);
                // Let the script's wait() steps run on real frame time, as in the viewer.
                app.world_mut()
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .clear();
                for _ in 0..120 {
                    std::thread::sleep(std::time::Duration::from_millis(16));
                    app.update();
                }
                let end: Vec<Vec3> = app
                    .world_mut()
                    .query::<(&PartLink, &Transform)>()
                    .iter(app.world())
                    .map(|(_, t)| t.translation)
                    .collect();
                let sweep = before
                    .iter()
                    .zip(&end)
                    .map(|(a, b)| a.distance(*b))
                    .fold(0.0_f32, f32::max);
                println!("click moved a part from {axis:?}; largest sweep {sweep:.2} studs");
                assert!(sweep > 2.0, "door sweep too small: {sweep:.2} studs");
                break;
            }
        }
        assert!(
            moved_axis.is_some(),
            "no face of the door button produced a part move"
        );
    }
}
