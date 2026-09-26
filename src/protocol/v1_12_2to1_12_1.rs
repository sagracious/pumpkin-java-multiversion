use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_12_2To1_12_1;

impl Protocol for Protocol1_12_2To1_12_1 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_12_2,
            to: V::V_1_12_1,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
