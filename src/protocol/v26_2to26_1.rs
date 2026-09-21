use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol26_2To26_1;

impl Protocol for Protocol26_2To26_1 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_26_2,
            to: JavaMinecraftVersion::V_26_1,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}
