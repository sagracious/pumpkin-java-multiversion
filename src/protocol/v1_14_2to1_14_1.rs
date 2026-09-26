use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_14_2To1_14_1;

impl Protocol for Protocol1_14_2To1_14_1 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_14_2,
            to: V::V_1_14_1,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
