use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_15_2To1_15_1;

impl Protocol for Protocol1_15_2To1_15_1 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_15_2,
            to: JavaMinecraftVersion::V_1_15_1,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patch_boundary_is_exact_and_has_no_packet_rewrites() {
        let step = Protocol1_15_2To1_15_1.step();
        assert_eq!(step.from, JavaMinecraftVersion::V_1_15_2);
        assert_eq!(step.to, JavaMinecraftVersion::V_1_15_1);
        let mut registry = Registry::default();
        Protocol1_15_2To1_15_1.register(&mut registry);
        assert_eq!(registry.clientbound_keys().count(), 0);
        assert_eq!(registry.serverbound_keys().count(), 0);
    }
}
