use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_14_1To1_14;

impl Protocol for Protocol1_14_1To1_14 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_14_1,
            to: V::V_1_14,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
