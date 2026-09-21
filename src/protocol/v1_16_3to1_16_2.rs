use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_16_3To1_16_2;

impl Protocol for Protocol1_16_3To1_16_2 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_16_3,
            to: JavaMinecraftVersion::V_1_16_2,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
