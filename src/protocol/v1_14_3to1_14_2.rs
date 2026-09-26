use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_14_3To1_14_2;

impl Protocol for Protocol1_14_3To1_14_2 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_14_3,
            to: V::V_1_14_2,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
