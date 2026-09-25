use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Protocol, Registry, Step};

pub struct Protocol1_15_1To1_15;

impl Protocol for Protocol1_15_1To1_15 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_15_1,
            to: JavaMinecraftVersion::V_1_15,
        }
    }

    fn register(&self, _reg: &mut Registry) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minor_boundary_is_exact_and_has_no_packet_rewrites() {
        let step = Protocol1_15_1To1_15.step();
        assert_eq!(step.from, JavaMinecraftVersion::V_1_15_1);
        assert_eq!(step.to, JavaMinecraftVersion::V_1_15);
        let mut registry = Registry::default();
        Protocol1_15_1To1_15.register(&mut registry);
        assert_eq!(registry.clientbound_keys().count(), 0);
        assert_eq!(registry.serverbound_keys().count(), 0);
    }
}
