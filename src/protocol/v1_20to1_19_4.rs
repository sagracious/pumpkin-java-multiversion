use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{I32, VAR_INT};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;

pub struct Protocol1_20To1_19_4;

/// The opponent 1.19.4 and below expect. Vanilla stopped sending a real one
/// long before it dropped the field, and ViaBackwards writes the same.
const NO_OPPONENT: i32 = -1;

impl Protocol for Protocol1_20To1_19_4 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_20,
            to: JavaMinecraftVersion::V_1_19_4,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound_layout(&clientbound::play::PLAYER_COMBAT_END, combat_end);
        reg.clientbound_layout(&clientbound::play::PLAYER_COMBAT_KILL, combat_kill);
    }
}

fn combat_end(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    wrapper.write(&I32, &NO_OPPONENT)
}

fn combat_kill(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    wrapper.write(&I32, &NO_OPPONENT)?;
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::pipeline::translate_clientbound;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::java::client::play::{CCombatDeath, CCombatEnd};
    use pumpkin_util::text::TextComponent;

    const PLAY: u8 = 5;
    const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_1_19_4;

    /// minecraft-data 1.19.4 `packet_end_combat_event`: a duration varint and
    /// an entity id int, and `packet_death_combat_event`: a player varint, an
    /// entity id int and the message.
    #[test]
    fn combat_end_regains_its_opponent() {
        let mut payload = Vec::new();
        CCombatEnd::new(VarInt(7))
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();

        let out = translate_clientbound(
            70,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_COMBAT_END.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(out.payload, [0x07, 0xff, 0xff, 0xff, 0xff]);
        remove_connection(70);
    }

    #[test]
    fn combat_kill_regains_its_killer_before_the_message() {
        let message = TextComponent::text("ouch");
        let mut payload = Vec::new();
        CCombatDeath::new(VarInt(3), &message)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();

        let out = translate_clientbound(
            71,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_COMBAT_KILL.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(&out.payload[..5], &[0x03, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(&out.payload[5..], &payload[1..]);
        remove_connection(71);
    }
}
