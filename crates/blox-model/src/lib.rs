use glam::{Mat3, Vec2, Vec3};
use std::{collections::HashMap, fmt, path::Path};

mod binary;
pub mod brick_color;
mod xml;

#[derive(Debug, Clone, PartialEq)]
pub enum Property {
    Text(String),
    /// `BinaryString` payload: terrain grids and other blobs that are not valid UTF-8.
    Binary(Vec<u8>),
    Bool(bool),
    Int(i64),
    Uint(u32),
    Float(f64),
    Vector2(Vec2),
    Vector3(Vec3),
    Color3(Vec3),
    CFrame {
        position: Vec3,
        rotation: Mat3,
    },
    Ref(Option<String>),
    Udim {
        scale: f32,
        offset: i32,
    },
    Udim2 {
        x_scale: f32,
        x_offset: i32,
        y_scale: f32,
        y_offset: i32,
    },
    Ray {
        origin: Vec3,
        direction: Vec3,
    },
    NumberSequence(Vec<Vec3>),
    ColorSequence(Vec<[f32; 5]>),
    NumberRange(Vec2),
    PhysicalProperties {
        custom: bool,
        values: [f32; 5],
    },
    Other(String),
}

#[derive(Debug, Clone)]
pub struct ModelInstance {
    pub referent: String,
    pub class: String,
    pub name: String,
    pub parent: Option<String>,
    pub properties: HashMap<String, Property>,
}

#[derive(Debug)]
pub struct Model {
    pub instances: Vec<ModelInstance>,
}

#[derive(Debug)]
pub struct ModelError(pub String);

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ModelError {}
impl From<std::io::Error> for ModelError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}
pub type ModelResult<T> = Result<T, ModelError>;

impl Model {
    pub fn load(path: impl AsRef<Path>) -> ModelResult<Self> {
        Self::decode(&std::fs::read(path)?)
    }

    pub fn decode(bytes: &[u8]) -> ModelResult<Self> {
        // The repository's legacy .rbxm files can contain XML despite their extension.
        if bytes.starts_with(b"<roblox!") {
            binary::decode(bytes)
        } else {
            xml::decode(bytes)
        }
    }
}
