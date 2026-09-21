use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol26_1To1_21_11;

impl Protocol for Protocol26_1To1_21_11 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_26_1,
            to: JavaMinecraftVersion::V_1_21_11,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
