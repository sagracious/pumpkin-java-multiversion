use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_21_11To1_21_9;

impl Protocol for Protocol1_21_11To1_21_9 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21_11,
            to: JavaMinecraftVersion::V_1_21_9,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
