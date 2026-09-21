use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_21_6To1_21_5;

impl Protocol for Protocol1_21_6To1_21_5 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21_6,
            to: JavaMinecraftVersion::V_1_21_5,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
