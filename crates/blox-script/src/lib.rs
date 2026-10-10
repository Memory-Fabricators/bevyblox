//! Runs legacy `Script`/`LocalScript` sources in Luau against the loaded instance tree.
//!
//! Each script and each event handler runs in its own coroutine, as in ScriptContext.cpp.
//! `wait(n)` yields; the host resumes the coroutine once `n` seconds of host time have passed.
//! Clicks and hovers resolve a camera ray to a `ClickDetector` (ClickDetector.cpp) and fire its
//! connected handlers.

pub mod bridge;
pub mod world;

use std::{cell::RefCell, rc::Rc};

use blox_model::{Model, Property};
use glam::Vec3;
use mlua::{Function, IntoLuaMulti, Lua, Result as LuaResult, Thread, Value, thread::ThreadStatus};

pub use world::{Edit, Node};

use world::World;

struct Task {
    name: String,
    thread: Thread,
    wake_at: f64,
}

pub struct ScriptHost {
    lua: Lua,
    world: bridge::Shared,
    tasks: Vec<Task>,
    now: f64,
    hovered: Option<usize>,
}

impl ScriptHost {
    pub fn new(model: Model) -> LuaResult<Self> {
        let lua = Lua::new();
        let world = Rc::new(RefCell::new(World::new(model)));
        bridge::install(&lua, &world)?;
        lua.load(bridge::PRELUDE).exec()?;
        Ok(Self {
            lua,
            world,
            tasks: Vec::new(),
            now: 0.0,
            hovered: None,
        })
    }

    /// Starts every enabled `Script` and `LocalScript` in model order. Script errors are reported
    /// and do not stop the other scripts.
    pub fn start_scripts(&mut self) {
        let scripts: Vec<(usize, String, String)> = {
            let w = self.world.borrow();
            w.model
                .instances
                .iter()
                .enumerate()
                .filter(|(i, inst)| {
                    matches!(inst.class.as_str(), "Script" | "LocalScript")
                        && !matches!(w.property(*i, "Disabled"), Some(Property::Bool(true)))
                })
                .filter_map(|(i, inst)| {
                    let source = match w.property(i, "Source")? {
                        Property::Text(s) => s.clone(),
                        Property::Binary(b) => String::from_utf8_lossy(b).into_owned(),
                        _ => return None,
                    };
                    Some((i, inst.name.clone(), source))
                })
                .collect()
        };
        for (index, name, source) in scripts {
            if let Err(error) = self.start_script(index, &name, &source) {
                eprintln!("Script {name}: {error}");
            }
        }
    }

    fn start_script(&mut self, index: usize, name: &str, source: &str) -> LuaResult<()> {
        // `script` is the running script's instance; the line offset of one keeps error lines
        // matching the source file.
        let chunk = format!("local script = ...\n{source}");
        // Each script gets its own globals, falling back to the shared ones (as each script has
        // its own environment in the original). Without this, `door = ...` in one door script
        // overwrites the `door` used by every other door script.
        let environment = self.lua.create_table()?;
        let metatable = self.lua.create_table()?;
        metatable.set("__index", self.lua.globals())?;
        environment.set_metatable(Some(metatable))?;
        let function = self
            .lua
            .load(chunk)
            .set_name(name)
            .set_environment(environment)
            .into_function()?;
        let instance = bridge::instance_value(&self.lua, &self.world, Node::Instance(index))?;
        let thread = self.lua.create_thread(function)?;
        self.step(name.to_owned(), thread, instance);
        Ok(())
    }

    /// Advances host time and resumes every coroutine whose wait has elapsed.
    pub fn tick(&mut self, dt: f32) {
        self.now += f64::from(dt);
        let (due, waiting): (Vec<Task>, Vec<Task>) = std::mem::take(&mut self.tasks)
            .into_iter()
            .partition(|task| task.wake_at <= self.now);
        self.tasks = waiting;
        for task in due {
            self.step(task.name, task.thread, ());
        }
    }

    /// Resumes a coroutine. If it yields again, it is queued to resume after its delay.
    fn step(&mut self, name: String, thread: Thread, args: impl IntoLuaMulti) {
        match thread.resume::<Value>(args) {
            Ok(value) => {
                if thread.status() == ThreadStatus::Resumable {
                    let delay = match value {
                        Value::Number(n) => n,
                        Value::Integer(n) => n as f64,
                        _ => 0.0,
                    };
                    self.tasks.push(Task {
                        name,
                        thread,
                        wake_at: self.now + delay.max(0.0),
                    });
                }
            }
            Err(error) => eprintln!("Script {name}: {error}"),
        }
    }

    /// Runs every handler connected to `event` on `detector`, each in its own coroutine.
    fn fire(&mut self, detector: usize, event: &'static str) {
        let handlers: Vec<Function> = {
            let w = self.world.borrow();
            w.connections
                .iter()
                .filter(|c| c.connected && c.node == detector && c.event == event)
                .filter_map(|c| self.lua.registry_value::<Function>(&c.handler).ok())
                .collect()
        };
        for handler in handlers {
            match self.lua.create_thread(handler) {
                Ok(thread) => self.step(event.to_owned(), thread, ()),
                Err(error) => eprintln!("{event}: {error}"),
            }
        }
    }

    /// A primary click along a ray (`direction` is unit length). Returns whether a
    /// `ClickDetector` was clicked.
    pub fn click(&mut self, origin: Vec3, direction: Vec3) -> bool {
        let target = self.world.borrow().click_target(origin, direction);
        match target {
            Some(detector) => {
                self.fire(detector, "MouseClick");
                true
            }
            None => false,
        }
    }

    /// Updates the hovered `ClickDetector` for the ray and fires enter/leave on change.
    pub fn hover(&mut self, origin: Vec3, direction: Vec3) {
        let target = self.world.borrow().click_target(origin, direction);
        if target == self.hovered {
            return;
        }
        if let Some(old) = self.hovered {
            self.fire(old, "MouseHoverLeave");
        }
        if let Some(new) = target {
            self.fire(new, "MouseHoverEnter");
        }
        self.hovered = target;
    }

    /// Drains the changes scripts made since the last call.
    pub fn take_edits(&mut self) -> Vec<Edit> {
        std::mem::take(&mut self.world.borrow_mut().edits)
    }
}
