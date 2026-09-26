use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_14_4To1_14_3;

impl Protocol for Protocol1_14_4To1_14_3 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_14_4,
            to: V::V_1_14_3,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
