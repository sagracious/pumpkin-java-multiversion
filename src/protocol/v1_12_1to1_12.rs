use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_12_1To1_12;

impl Protocol for Protocol1_12_1To1_12 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_12_1,
            to: V::V_1_12,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
