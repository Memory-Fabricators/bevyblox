//! Lua-facing objects: `Instance`, events, connections, `Vector3`, and `CFrame`, plus the
//! globals (`game`, `workspace`, `Vector3`, `CFrame`) and the `wait` prelude.
//!
//! Member lookup follows Roblox: built-in members first, then events, methods, properties, and
//! finally children by name. Unsupported members are nil, so script errors name what is missing.

use std::rc::Rc;

use blox_model::Property;
use glam::{Mat3, Vec3};
use mlua::{
    Function, Lua, MetaMethod, MultiValue, Result as LuaResult, UserData, UserDataFields,
    UserDataMethods, UserDataRef, Value,
};

use crate::world::{Edit, Node, World};

pub type Shared = Rc<std::cell::RefCell<World>>;

/// Runs before any script. `wait` yields the coroutine with the delay; the host resumes it later.
pub const PRELUDE: &str = "\
function wait(seconds)
	local delay = seconds or 0.03
	if delay < 0 then delay = 0 end
	return coroutine.yield(delay)
end
";

fn runtime(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}

#[derive(Clone, Copy, Debug)]
pub struct LuaVec3(pub Vec3);

impl UserData for LuaVec3 {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("X", |_, v| Ok(v.0.x));
        fields.add_field_method_get("Y", |_, v| Ok(v.0.y));
        fields.add_field_method_get("Z", |_, v| Ok(v.0.z));
        fields.add_field_method_get("magnitude", |_, v| Ok(v.0.length()));
        fields.add_field_method_get("unit", |_, v| Ok(LuaVec3(v.0.normalize_or_zero())));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Add, |_, a, b: UserDataRef<LuaVec3>| {
            Ok(LuaVec3(a.0 + b.0))
        });
        methods.add_meta_method(MetaMethod::Sub, |_, a, b: UserDataRef<LuaVec3>| {
            Ok(LuaVec3(a.0 - b.0))
        });
        methods.add_meta_method(MetaMethod::Unm, |_, a, _: Value| Ok(LuaVec3(-a.0)));
        methods.add_meta_method(MetaMethod::Mul, |_, a, b: Value| match b {
            Value::Number(s) => Ok(LuaVec3(a.0 * s as f32)),
            Value::Integer(s) => Ok(LuaVec3(a.0 * s as f32)),
            Value::UserData(ud) => Ok(LuaVec3(a.0 * ud.borrow::<LuaVec3>()?.0)),
            _ => Err(runtime(
                "Vector3 can only be multiplied by a number or Vector3",
            )),
        });
        methods.add_meta_method(MetaMethod::Div, |_, a, s: f64| Ok(LuaVec3(a.0 / s as f32)));
        methods.add_meta_method(MetaMethod::Eq, |_, a, b: UserDataRef<LuaVec3>| {
            Ok(a.0 == b.0)
        });
        methods.add_meta_method(MetaMethod::ToString, |_, a, ()| {
            Ok(format!("{}, {}, {}", a.0.x, a.0.y, a.0.z))
        });
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LuaCFrame {
    pub position: Vec3,
    pub rotation: Mat3,
}

impl UserData for LuaCFrame {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Position", |lua, c| {
            lua.create_userdata(LuaVec3(c.position))
        });
        fields.add_field_method_get("X", |_, c| Ok(c.position.x));
        fields.add_field_method_get("Y", |_, c| Ok(c.position.y));
        fields.add_field_method_get("Z", |_, c| Ok(c.position.z));
        fields.add_field_method_get("RightVector", |lua, c| {
            lua.create_userdata(LuaVec3(c.rotation.x_axis))
        });
        fields.add_field_method_get("UpVector", |lua, c| {
            lua.create_userdata(LuaVec3(c.rotation.y_axis))
        });
        fields.add_field_method_get("LookVector", |lua, c| {
            lua.create_userdata(LuaVec3(-c.rotation.z_axis))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Mul, |lua, a, b: Value| {
            let Value::UserData(ud) = b else {
                return Err(runtime(
                    "CFrame can only be multiplied by a CFrame or Vector3",
                ));
            };
            if let Ok(v) = ud.borrow::<LuaVec3>() {
                let moved = a.rotation * v.0 + a.position;
                return Ok(Value::UserData(lua.create_userdata(LuaVec3(moved))?));
            }
            let other = ud.borrow::<LuaCFrame>()?;
            let composed = LuaCFrame {
                position: a.position + a.rotation * other.position,
                rotation: a.rotation * other.rotation,
            };
            Ok(Value::UserData(lua.create_userdata(composed)?))
        });
        methods.add_method("Inverse", |_, c, ()| Ok(inverse(c)));
        methods.add_method("inverse", |_, c, ()| Ok(inverse(c)));
    }
}

fn inverse(c: &LuaCFrame) -> LuaCFrame {
    let rotation = c.rotation.transpose();
    LuaCFrame {
        position: -(rotation * c.position),
        rotation,
    }
}

/// An instance or the DataModel.
#[derive(Clone)]
pub struct LuaInstance {
    pub world: Shared,
    pub node: Node,
}

impl UserData for LuaInstance {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: String| {
            member(lua, this, &key)
        });
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, this, (key, value): (String, Value)| assign(this, &key, value),
        );
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            let world = this.world.borrow();
            Ok(match this.node {
                Node::Game => "Game".to_owned(),
                Node::Instance(i) => world.name(i).to_owned(),
            })
        });
        methods.add_meta_method(
            MetaMethod::Eq,
            |_, this, other: UserDataRef<LuaInstance>| {
                Ok(Rc::ptr_eq(&this.world, &other.world) && this.node == other.node)
            },
        );
    }
}

/// `ClickDetector` events a script can connect to. Returns the canonical event name.
fn event_name(class: &str, key: &str) -> Option<&'static str> {
    match (class, key) {
        ("ClickDetector", "MouseClick" | "mouseClick") => Some("MouseClick"),
        ("ClickDetector", "MouseHoverEnter") => Some("MouseHoverEnter"),
        ("ClickDetector", "MouseHoverLeave") => Some("MouseHoverLeave"),
        _ => None,
    }
}

#[derive(Clone)]
pub struct LuaEvent {
    pub world: Shared,
    pub node: usize,
    pub event: &'static str,
}

impl UserData for LuaEvent {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("connect", |lua, this, handler: Function| {
            connect(lua, this, handler)
        });
        methods.add_method("Connect", |lua, this, handler: Function| {
            connect(lua, this, handler)
        });
    }
}

fn connect(lua: &Lua, event: &LuaEvent, handler: Function) -> LuaResult<LuaConnection> {
    let key = lua.create_registry_value(handler)?;
    let id = event
        .world
        .borrow_mut()
        .add_connection(event.node, event.event, key);
    Ok(LuaConnection {
        world: event.world.clone(),
        id,
    })
}

#[derive(Clone)]
pub struct LuaConnection {
    pub world: Shared,
    pub id: u64,
}

impl UserData for LuaConnection {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Disconnect", |_, this, ()| {
            this.world.borrow_mut().disconnect(this.id);
            Ok(())
        });
        methods.add_method("disconnect", |_, this, ()| {
            this.world.borrow_mut().disconnect(this.id);
            Ok(())
        });
    }
}

pub(crate) fn instance_value(lua: &Lua, world: &Shared, node: Node) -> LuaResult<Value> {
    Ok(Value::UserData(lua.create_userdata(LuaInstance {
        world: world.clone(),
        node,
    })?))
}

fn cframe_value(lua: &Lua, position: Vec3, rotation: Mat3) -> LuaResult<Value> {
    Ok(Value::UserData(
        lua.create_userdata(LuaCFrame { position, rotation })?,
    ))
}

fn property_value(lua: &Lua, world: &Shared, property: &Property) -> LuaResult<Value> {
    Ok(match property {
        Property::Text(s) => Value::String(lua.create_string(s)?),
        Property::Bool(b) => Value::Boolean(*b),
        Property::Int(n) => Value::Integer(*n),
        Property::Uint(n) => Value::Integer(i64::from(*n)),
        Property::Float(n) => Value::Number(*n),
        Property::Vector3(v) => Value::UserData(lua.create_userdata(LuaVec3(*v))?),
        Property::CFrame { position, rotation } => cframe_value(lua, *position, *rotation)?,
        Property::Ref(Some(referent)) => {
            let target = world.borrow().index_of_referent(referent);
            match target {
                Some(i) => instance_value(lua, world, Node::Instance(i))?,
                None => Value::Nil,
            }
        }
        _ => Value::Nil,
    })
}

/// Converts a Lua value to a property, keeping the numeric kind of an existing property.
fn value_property(
    value: &Value,
    world: &Shared,
    existing: Option<&Property>,
) -> LuaResult<Property> {
    Ok(match value {
        Value::Nil => Property::Ref(None),
        Value::Boolean(b) => Property::Bool(*b),
        Value::Integer(n) => match existing {
            Some(Property::Int(_)) => Property::Int(*n),
            _ => Property::Float(*n as f64),
        },
        Value::Number(n) => match existing {
            Some(Property::Int(_)) => Property::Int(*n as i64),
            _ => Property::Float(*n),
        },
        Value::String(s) => Property::Text(s.to_string_lossy()),
        Value::UserData(ud) => {
            if let Ok(v) = ud.borrow::<LuaVec3>() {
                Property::Vector3(v.0)
            } else if let Ok(c) = ud.borrow::<LuaCFrame>() {
                Property::CFrame {
                    position: c.position,
                    rotation: c.rotation,
                }
            } else if let Ok(inst) = ud.borrow::<LuaInstance>() {
                match inst.node {
                    Node::Instance(i) => Property::Ref(Some(world.borrow().referent(i).to_owned())),
                    Node::Game => return Err(runtime("the DataModel cannot be a property value")),
                }
            } else {
                return Err(runtime("unsupported userdata value"));
            }
        }
        _ => return Err(runtime("unsupported Lua value for a property")),
    })
}

fn expect_vec3(value: &Value) -> LuaResult<Vec3> {
    match value {
        Value::UserData(ud) => Ok(ud.borrow::<LuaVec3>()?.0),
        _ => Err(runtime("expected a Vector3")),
    }
}

fn expect_cframe(value: &Value) -> LuaResult<LuaCFrame> {
    match value {
        Value::UserData(ud) => Ok(*ud.borrow::<LuaCFrame>()?),
        _ => Err(runtime("expected a CFrame")),
    }
}

fn class_is_method(class: &str, node: Node, key: &str) -> bool {
    match key {
        "FindFirstChild" | "findFirstChild" | "WaitForChild" | "GetChildren" | "children"
        | "Remove" | "Destroy" | "remove" => true,
        "Play" | "Stop" => class == "Sound",
        "GetService" => node == Node::Game,
        _ => false,
    }
}

fn member(lua: &Lua, this: &LuaInstance, key: &str) -> LuaResult<Value> {
    let world = &this.world;
    let node = this.node;
    let class = match node {
        Node::Game => "DataModel".to_owned(),
        Node::Instance(i) => world.borrow().class(i).to_owned(),
    };
    if let Node::Instance(i) = node {
        match key {
            "Parent" => {
                let parent = world.borrow().parent_node(i);
                return match parent {
                    Some(p) => instance_value(lua, world, p),
                    None => Ok(Value::Nil),
                };
            }
            "Name" => {
                let name = world.borrow().name(i).to_owned();
                return Ok(Value::String(lua.create_string(&name)?));
            }
            "ClassName" | "className" => {
                return Ok(Value::String(lua.create_string(&class)?));
            }
            "CFrame" | "CoordinateFrame" => {
                let cf = world.borrow().cframe(i);
                return match cf {
                    Some((p, r)) => cframe_value(lua, p, r),
                    None => Ok(Value::Nil),
                };
            }
            "Position" => {
                let cf = world.borrow().cframe(i);
                return match cf {
                    Some((p, _)) => Ok(Value::UserData(lua.create_userdata(LuaVec3(p))?)),
                    None => Ok(Value::Nil),
                };
            }
            _ => {}
        }
        if let Some(event) = event_name(&class, key) {
            return Ok(Value::UserData(lua.create_userdata(LuaEvent {
                world: world.clone(),
                node: i,
                event,
            })?));
        }
    } else {
        match key {
            "Parent" => return Ok(Value::Nil),
            "Name" => return Ok(Value::String(lua.create_string("Game")?)),
            "ClassName" | "className" => return Ok(Value::String(lua.create_string("DataModel")?)),
            _ => {}
        }
    }
    if class_is_method(&class, node, key) {
        let world = world.clone();
        let method = key.to_owned();
        let function = lua.create_function(move |lua, args: MultiValue| {
            call_method(lua, &world, node, &method, args)
        })?;
        return Ok(Value::Function(function));
    }
    if let Node::Instance(i) = node {
        let property = world.borrow().property(i, key).cloned();
        if let Some(property) = property {
            return property_value(lua, world, &property);
        }
    }
    let child = world.borrow().find_child(node, key);
    match child {
        Some(c) => instance_value(lua, world, Node::Instance(c)),
        None => Ok(Value::Nil),
    }
}

fn call_method(
    lua: &Lua,
    world: &Shared,
    node: Node,
    method: &str,
    args: MultiValue,
) -> LuaResult<Value> {
    // Method calls with `:` pass the receiver first; drop it when it is this instance.
    let mut args: Vec<Value> = args.into_vec();
    if let Some(Value::UserData(ud)) = args.first() {
        if ud
            .borrow::<LuaInstance>()
            .is_ok_and(|inst| inst.node == node)
        {
            args.remove(0);
        }
    }
    match method {
        "FindFirstChild" | "findFirstChild" | "WaitForChild" => {
            let Some(Value::String(name)) = args.first() else {
                return Err(runtime(format!("{method} expects a name")));
            };
            let name = name.to_string_lossy();
            let child = world.borrow().find_child(node, &name);
            match child {
                Some(c) => instance_value(lua, world, Node::Instance(c)),
                None => Ok(Value::Nil),
            }
        }
        "GetChildren" | "children" => {
            let children = world.borrow().children_of(node);
            let values = children
                .into_iter()
                .map(|c| instance_value(lua, world, Node::Instance(c)))
                .collect::<LuaResult<Vec<_>>>()?;
            Ok(Value::Table(lua.create_sequence_from(values)?))
        }
        "Remove" | "Destroy" | "remove" => {
            if let Node::Instance(i) = node {
                world.borrow_mut().remove(i);
            }
            Ok(Value::Nil)
        }
        "Play" | "Stop" => {
            let Node::Instance(i) = node else {
                return Err(runtime(format!("{method} is not a Sound method")));
            };
            let edit = if method == "Play" {
                Edit::SoundPlay(i)
            } else {
                Edit::SoundStop(i)
            };
            world.borrow_mut().edits.push(edit);
            Ok(Value::Nil)
        }
        "GetService" => {
            let Some(Value::String(name)) = args.first() else {
                return Err(runtime("GetService expects a service name"));
            };
            let name = name.to_string_lossy();
            let service = world
                .borrow()
                .children_of(Node::Game)
                .into_iter()
                .find(|&c| world.borrow().class(c) == name);
            match service {
                Some(c) => instance_value(lua, world, Node::Instance(c)),
                None => Err(runtime(format!("GetService: no service {name}"))),
            }
        }
        _ => Err(runtime(format!("unknown method {method}"))),
    }
}

fn assign(this: &LuaInstance, key: &str, value: Value) -> LuaResult<()> {
    let world = &this.world;
    let Node::Instance(i) = this.node else {
        return Err(runtime(format!("cannot set {key} on the DataModel")));
    };
    match key {
        "Parent" => {
            let target = match &value {
                Value::UserData(ud) => match ud.borrow::<LuaInstance>()?.node {
                    Node::Game => None,
                    Node::Instance(p) => Some(p),
                },
                _ => {
                    return Err(runtime(
                        "Parent must be an instance; setting nil is not supported",
                    ));
                }
            };
            world.borrow_mut().set_parent(i, target).map_err(runtime)
        }
        "Name" => match &value {
            Value::String(s) => {
                world.borrow_mut().set_name(i, s.to_string_lossy());
                Ok(())
            }
            _ => Err(runtime("Name must be a string")),
        },
        "CFrame" | "CoordinateFrame" => {
            let cf = expect_cframe(&value)?;
            world.borrow_mut().set_cframe(i, cf.position, cf.rotation);
            Ok(())
        }
        "Position" => {
            let position = expect_vec3(&value)?;
            let rotation = world.borrow().cframe(i).map_or(Mat3::IDENTITY, |(_, r)| r);
            world.borrow_mut().set_cframe(i, position, rotation);
            Ok(())
        }
        _ => {
            let existing = world.borrow().property(i, key).cloned();
            let property = value_property(&value, world, existing.as_ref())?;
            world.borrow_mut().set_property(i, key, property);
            Ok(())
        }
    }
}

/// Defines `game`, `workspace`, `Vector3`, and `CFrame` in the global environment.
pub fn install(lua: &Lua, world: &Shared) -> LuaResult<()> {
    let globals = lua.globals();

    let vector3 = lua.create_table()?;
    vector3.set(
        "new",
        lua.create_function(|_, (x, y, z): (Option<f32>, Option<f32>, Option<f32>)| {
            Ok(LuaVec3(Vec3::new(
                x.unwrap_or(0.0),
                y.unwrap_or(0.0),
                z.unwrap_or(0.0),
            )))
        })?,
    )?;
    vector3.set("zero", lua.create_userdata(LuaVec3(Vec3::ZERO))?)?;
    vector3.set("one", lua.create_userdata(LuaVec3(Vec3::ONE))?)?;
    globals.set("Vector3", vector3)?;

    let cframe = lua.create_table()?;
    cframe.set(
        "new",
        lua.create_function(|lua, args: MultiValue| {
            let args = args.into_vec();
            let position = match args.first() {
                None => Vec3::ZERO,
                Some(Value::UserData(ud)) => ud.borrow::<LuaVec3>()?.0,
                Some(first) => {
                    let number = |v: Option<&Value>| match v {
                        Some(Value::Number(n)) => Ok(*n as f32),
                        Some(Value::Integer(n)) => Ok(*n as f32),
                        None => Ok(0.0),
                        _ => Err(runtime("CFrame.new expects numbers or a Vector3")),
                    };
                    Vec3::new(
                        number(Some(first))?,
                        number(args.get(1))?,
                        number(args.get(2))?,
                    )
                }
            };
            cframe_value(lua, position, Mat3::IDENTITY)
        })?,
    )?;
    globals.set("CFrame", cframe)?;

    globals.set("game", instance_value(lua, world, Node::Game)?)?;
    let workspace = {
        let w = world.borrow();
        w.children_of(Node::Game)
            .into_iter()
            .find(|&c| w.class(c) == "Workspace")
    };
    let workspace = match workspace {
        Some(i) => instance_value(lua, world, Node::Instance(i))?,
        None => Value::Nil,
    };
    globals.set("workspace", workspace.clone())?;
    globals.set("Workspace", workspace)?;
    Ok(())
}
