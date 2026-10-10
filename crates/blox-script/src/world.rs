//! The instance tree scripts see: the loaded `Model` plus the parent links and property writes
//! that scripts make at runtime, and the ray picking used for `ClickDetector`.

use std::collections::HashMap;

use blox_model::{Model, Property};
use glam::{Mat3, Vec3};
use mlua::RegistryKey;

/// Default `ClickDetector.MaxActivationDistance` (ClickDetector.cpp).
const DEFAULT_ACTIVATION_DISTANCE: f32 = 32.0;

/// Change a script made that the renderer or audio backend must apply. Indices are model
/// instance indices.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// A part's `CFrame` was assigned. `position` is the CFrame position (before any mesh
    /// offset) and `rotation` the orientation matrix.
    PartMoved {
        index: usize,
        position: Vec3,
        rotation: Mat3,
    },
    SoundPlay(usize),
    SoundStop(usize),
}

/// A script-visible instance. `Game` is the DataModel: its children are the top-level
/// instances in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Game,
    Instance(usize),
}

pub struct Connection {
    pub id: u64,
    pub node: usize,
    pub event: &'static str,
    pub handler: RegistryKey,
    pub connected: bool,
}

pub struct World {
    pub model: Model,
    parent: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
    removed: Vec<bool>,
    referents: HashMap<String, usize>,
    pub connections: Vec<Connection>,
    next_connection: u64,
    pub edits: Vec<Edit>,
}

impl World {
    pub fn new(model: Model) -> Self {
        let parent: Vec<Option<usize>> = {
            let index: HashMap<&str, usize> = model
                .instances
                .iter()
                .enumerate()
                .map(|(i, inst)| (inst.referent.as_str(), i))
                .collect();
            model
                .instances
                .iter()
                .map(|inst| {
                    inst.parent
                        .as_deref()
                        .and_then(|parent| index.get(parent).copied())
                })
                .collect()
        };
        let count = model.instances.len();
        let mut children = vec![Vec::new(); count];
        let mut roots = Vec::new();
        for (i, parent) in parent.iter().enumerate() {
            match parent {
                Some(p) => children[*p].push(i),
                None => roots.push(i),
            }
        }
        let referents = model
            .instances
            .iter()
            .enumerate()
            .map(|(i, inst)| (inst.referent.clone(), i))
            .collect();
        Self {
            model,
            parent,
            children,
            roots,
            removed: vec![false; count],
            referents,
            connections: Vec::new(),
            next_connection: 0,
            edits: Vec::new(),
        }
    }

    pub fn class(&self, i: usize) -> &str {
        &self.model.instances[i].class
    }

    pub fn name(&self, i: usize) -> &str {
        &self.model.instances[i].name
    }

    pub fn set_name(&mut self, i: usize, name: String) {
        self.model.instances[i].name = name;
    }

    pub fn index_of_referent(&self, referent: &str) -> Option<usize> {
        self.referents.get(referent).copied()
    }

    pub fn referent(&self, i: usize) -> &str {
        &self.model.instances[i].referent
    }

    /// Whether the instance and all its ancestors are still in the tree.
    pub fn is_live(&self, i: usize) -> bool {
        let mut current = Some(i);
        while let Some(c) = current {
            if self.removed[c] {
                return false;
            }
            current = self.parent[c];
        }
        true
    }

    /// Parent as a script sees it. `None` means the instance was removed.
    pub fn parent_node(&self, i: usize) -> Option<Node> {
        if self.removed[i] {
            return None;
        }
        Some(match self.parent[i] {
            Some(p) => Node::Instance(p),
            None => Node::Game,
        })
    }

    pub fn children_of(&self, node: Node) -> Vec<usize> {
        let list = match node {
            Node::Game => &self.roots,
            Node::Instance(i) => &self.children[i],
        };
        list.iter().copied().filter(|&c| !self.removed[c]).collect()
    }

    pub fn find_child(&self, node: Node, name: &str) -> Option<usize> {
        self.children_of(node)
            .into_iter()
            .find(|&c| self.name(c) == name)
    }

    pub fn set_parent(&mut self, i: usize, new_parent: Option<usize>) -> Result<(), String> {
        let mut ancestor = new_parent;
        while let Some(a) = ancestor {
            if a == i {
                return Err(format!(
                    "cannot set Parent of {} to its own descendant",
                    self.name(i)
                ));
            }
            ancestor = self.parent[a];
        }
        match self.parent[i] {
            Some(old) => self.children[old].retain(|&c| c != i),
            None => self.roots.retain(|&c| c != i),
        }
        match new_parent {
            Some(p) => self.children[p].push(i),
            None => self.roots.push(i),
        }
        self.parent[i] = new_parent;
        self.removed[i] = false;
        Ok(())
    }

    /// `Instance:Remove()`: detach from the tree; the instance and its children stop being
    /// reachable from `game`.
    pub fn remove(&mut self, i: usize) {
        if self.removed[i] {
            return;
        }
        match self.parent[i] {
            Some(old) => self.children[old].retain(|&c| c != i),
            None => self.roots.retain(|&c| c != i),
        }
        self.removed[i] = true;
    }

    pub fn property(&self, i: usize, name: &str) -> Option<&Property> {
        self.model.instances[i].properties.get(name)
    }

    pub fn set_property(&mut self, i: usize, name: &str, value: Property) {
        self.model.instances[i]
            .properties
            .insert(name.to_owned(), value);
    }

    pub fn cframe(&self, i: usize) -> Option<(Vec3, Mat3)> {
        match self
            .property(i, "CFrame")
            .or(self.property(i, "CoordinateFrame"))
        {
            Some(Property::CFrame { position, rotation }) => Some((*position, *rotation)),
            _ => None,
        }
    }

    /// Writes the CFrame and records the move for the renderer. Only parts with a size are
    /// rendered, so only those produce a `PartMoved` edit.
    pub fn set_cframe(&mut self, i: usize, position: Vec3, rotation: Mat3) {
        let key = if self.property(i, "CFrame").is_none()
            && self.property(i, "CoordinateFrame").is_some()
        {
            "CoordinateFrame"
        } else {
            "CFrame"
        };
        self.set_property(i, key, Property::CFrame { position, rotation });
        if self.size(i).is_some() {
            self.edits.push(Edit::PartMoved {
                index: i,
                position,
                rotation,
            });
        }
    }

    pub fn size(&self, i: usize) -> Option<Vec3> {
        match self.property(i, "size").or(self.property(i, "Size")) {
            Some(Property::Vector3(v)) => Some(*v),
            _ => None,
        }
    }

    fn transparency(&self, i: usize) -> f32 {
        match self.property(i, "Transparency") {
            Some(Property::Float(v)) => *v as f32,
            _ => 0.0,
        }
    }

    fn can_collide(&self, i: usize) -> bool {
        match self.property(i, "CanCollide") {
            Some(Property::Bool(v)) => *v,
            _ => true,
        }
    }

    /// The first live `ClickDetector` child of an instance, if any.
    pub fn click_detector(&self, part: usize) -> Option<usize> {
        self.children_of(Node::Instance(part))
            .into_iter()
            .find(|&c| self.class(c) == "ClickDetector")
    }

    fn activation_distance(&self, detector: usize) -> f32 {
        match self.property(detector, "MaxActivationDistance") {
            Some(Property::Float(v)) => *v as f32,
            _ => DEFAULT_ACTIVATION_DISTANCE,
        }
    }

    /// Nearest live part under the ray that the mouse can hit. Parts with a `ClickDetector` are
    /// always hit-testable; otherwise fully transparent, non-colliding parts are ignored, as in
    /// Filters.cpp (`HitTestFilter::IGNORE_PRIM`).
    pub fn pick(&self, origin: Vec3, direction: Vec3) -> Option<(usize, f32)> {
        let mut nearest: Option<(usize, f32)> = None;
        for i in 0..self.model.instances.len() {
            let Some((center, rotation)) = self.cframe(i) else {
                continue;
            };
            let Some(size) = self.size(i) else {
                continue;
            };
            if !self.is_live(i) {
                continue;
            }
            if self.transparency(i) > 0.95
                && !self.can_collide(i)
                && self.click_detector(i).is_none()
            {
                continue;
            }
            let Some(t) = ray_box(origin, direction, center, rotation, size * 0.5) else {
                continue;
            };
            if nearest.is_none_or(|(_, best)| t < best) {
                nearest = Some((i, t));
            }
        }
        nearest
    }

    /// The `ClickDetector` that a click or hover at this ray resolves to. The detector must be
    /// within `MaxActivationDistance` of the origin, measured to the part's center.
    pub fn click_target(&self, origin: Vec3, direction: Vec3) -> Option<usize> {
        let (part, _) = self.pick(origin, direction)?;
        let detector = self.click_detector(part)?;
        let (center, _) = self.cframe(part)?;
        (origin.distance(center) < self.activation_distance(detector)).then_some(detector)
    }

    pub fn add_connection(
        &mut self,
        node: usize,
        event: &'static str,
        handler: RegistryKey,
    ) -> u64 {
        let id = self.next_connection;
        self.next_connection += 1;
        self.connections.push(Connection {
            id,
            node,
            event,
            handler,
            connected: true,
        });
        id
    }

    pub fn disconnect(&mut self, id: u64) {
        if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
            c.connected = false;
        }
    }
}

/// Slab test of a ray (unit `direction`) against an oriented box. Returns the entry distance
/// clamped to the origin, so a ray that starts inside the box hits at 0.
fn ray_box(origin: Vec3, direction: Vec3, center: Vec3, rotation: Mat3, half: Vec3) -> Option<f32> {
    let local_origin = rotation.transpose() * (origin - center);
    let local_direction = rotation.transpose() * direction;
    let (mut t_min, mut t_max) = (0.0_f32, f32::INFINITY);
    for axis in 0..3 {
        let o = local_origin[axis];
        let d = local_direction[axis];
        let h = half[axis];
        if d.abs() < 1e-9 {
            if o.abs() > h {
                return None;
            }
            continue;
        }
        let a = (-h - o) / d;
        let b = (h - o) / d;
        t_min = t_min.max(a.min(b));
        t_max = t_max.min(a.max(b));
        if t_min > t_max {
            return None;
        }
    }
    Some(t_min)
}
