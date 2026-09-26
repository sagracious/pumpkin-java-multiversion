use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_13_1To1_13;

impl Protocol for Protocol1_13_1To1_13 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_13_1,
            to: V::V_1_13,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
