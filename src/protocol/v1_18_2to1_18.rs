use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_18_2To1_18;

impl Protocol for Protocol1_18_2To1_18 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_18_2,
            to: JavaMinecraftVersion::V_1_18,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
