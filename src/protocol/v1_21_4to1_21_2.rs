use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_21_4To1_21_2;

impl Protocol for Protocol1_21_4To1_21_2 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21_4,
            to: JavaMinecraftVersion::V_1_21_2,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
