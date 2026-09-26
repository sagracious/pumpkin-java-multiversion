use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_16_1To1_16;

impl Protocol for Protocol1_16_1To1_16 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_16_1,
            to: V::V_1_16,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
