use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::connection::GameTimeStorage;
use crate::api::types::{F64, I64, VAR_INT, VAR_LONG, WireType};
use crate::api::{PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::{clientbound, serverbound};
use pumpkin_protocol::codec::var_long::VarLong;

const TICKS_PER_SECOND: i64 = 50;
const BREATH_OF_NAUTILUS_EFFECT_ID: i32 = 39;

pub struct Protocol1_21_11To1_21_9;

impl Protocol for Protocol1_21_11To1_21_9 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21_11,
            to: JavaMinecraftVersion::V_1_21_9,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound(&clientbound::play::SET_BORDER_LERP_SIZE, border_lerp_size);
        reg.clientbound(&clientbound::play::INITIALIZE_BORDER, border_initialize);
        reg.clientbound(&clientbound::play::SET_TIME, set_time);
        reg.clientbound(&clientbound::play::UPDATE_MOB_EFFECT, remove_new_mob_effect);
        reg.clientbound(&clientbound::play::REMOVE_MOB_EFFECT, remove_new_mob_effect);
        reg.serverbound(&serverbound::play::CLIENT_TICK_END, client_tick_end);
    }
}

/// 1.21.11 stores border interpolation time in seconds; 1.21.9 expects ticks.
fn border_lerp_size(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &crate::api::Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&F64)?;
    wrapper.passthrough(&F64)?;
    let seconds = wrapper.read(&VAR_LONG)?;
    wrapper.write(
        &VAR_LONG,
        &VarLong(seconds.0.wrapping_mul(TICKS_PER_SECOND)),
    )?;
    wrapper.passthrough_all();
    Ok(())
}

fn border_initialize(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &crate::api::Ctx,
) -> Result<(), TranslateError> {
    for _ in 0..4 {
        wrapper.passthrough(&F64)?;
    }
    let seconds = wrapper.read(&VAR_LONG)?;
    wrapper.write(
        &VAR_LONG,
        &VarLong(seconds.0.wrapping_mul(TICKS_PER_SECOND)),
    )?;
    wrapper.passthrough_all();
    Ok(())
}

fn set_time(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &crate::api::Ctx,
) -> Result<(), TranslateError> {
    let game_time = wrapper.passthrough(&I64)?;
    if let Some(storage) = connection.get_mut::<GameTimeStorage>() {
        storage.game_time = game_time;
    } else {
        connection.put(GameTimeStorage { game_time });
    }
    wrapper.passthrough_all();
    Ok(())
}

fn client_tick_end(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &crate::api::Ctx,
) -> Result<(), TranslateError> {
    if let Some(storage) = connection.get_mut::<GameTimeStorage>() {
        storage.game_time = storage.game_time.wrapping_add(1);
    } else {
        connection.put(GameTimeStorage { game_time: 1 });
    }
    wrapper.passthrough_all();
    Ok(())
}

fn remove_new_mob_effect(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &crate::api::Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let effect = wrapper.passthrough(&VAR_INT)?;
    if effect.0 == BREATH_OF_NAUTILUS_EFFECT_ID {
        wrapper.cancel();
    } else {
        wrapper.passthrough_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::pipeline::{translate_clientbound, translate_serverbound};
    use pumpkin_protocol::codec::var_int::VarInt;

    const PLAY: u8 = 5;
    const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_1_21_9;

    #[test]
    fn border_lerp_seconds_become_ticks() {
        let key = 0x2111_0001;
        let mut payload = Vec::new();
        F64.write(&mut payload, &1.0).unwrap();
        F64.write(&mut payload, &2.0).unwrap();
        VAR_LONG.write(&mut payload, &VarLong(7)).unwrap();

        let translated = translate_clientbound(
            key,
            VERSION,
            PLAY,
            clientbound::play::SET_BORDER_LERP_SIZE.v26_3,
            &payload,
        )
        .unwrap();

        let mut read = translated.payload.as_slice();
        assert_eq!(F64.read(&mut read).unwrap(), 1.0);
        assert_eq!(F64.read(&mut read).unwrap(), 2.0);
        assert_eq!(VAR_LONG.read(&mut read).unwrap(), VarLong(350));
        assert!(read.is_empty());
        remove_connection(key);
    }

    #[test]
    fn initialize_border_converts_only_the_lerp_duration() {
        let key = 0x2111_0002;
        let mut payload = Vec::new();
        for value in [1.0, 2.0, 3.0, 4.0] {
            F64.write(&mut payload, &value).unwrap();
        }
        VAR_LONG.write(&mut payload, &VarLong(9)).unwrap();
        for value in [10, 11, 12] {
            VAR_INT.write(&mut payload, &VarInt(value)).unwrap();
        }

        let translated = translate_clientbound(
            key,
            VERSION,
            PLAY,
            clientbound::play::INITIALIZE_BORDER.v26_3,
            &payload,
        )
        .unwrap();

        let mut read = translated.payload.as_slice();
        for expected in [1.0, 2.0, 3.0, 4.0] {
            assert_eq!(F64.read(&mut read).unwrap(), expected);
        }
        assert_eq!(VAR_LONG.read(&mut read).unwrap(), VarLong(450));
        for expected in [10, 11, 12] {
            assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(expected));
        }
        assert!(read.is_empty());
        remove_connection(key);
    }

    #[test]
    fn set_time_and_client_tick_end_track_game_time() {
        let key = 0x2111_0003;
        let mut payload = Vec::new();
        I64.write(&mut payload, &1_000).unwrap();
        I64.write(&mut payload, &24_000).unwrap();
        payload.push(1);

        let translated = translate_clientbound(
            key,
            VERSION,
            PLAY,
            clientbound::play::SET_TIME.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(translated.payload, payload);

        let tick = translate_serverbound(
            key,
            VERSION,
            PLAY,
            serverbound::play::CLIENT_TICK_END.to_id(VERSION),
            &[],
        )
        .unwrap();
        assert_eq!(tick.packet.v26_3, serverbound::play::CLIENT_TICK_END.v26_3);
        let current = crate::api::with_connection(key, VERSION, |connection| {
            connection
                .get::<GameTimeStorage>()
                .map(|storage| storage.game_time)
        });
        assert_eq!(current, Some(1_001));
        remove_connection(key);
    }

    #[test]
    fn unsupported_nautilus_effect_is_dropped_but_known_effects_pass() {
        let key = 0x2111_0004;
        let unsupported = [1, 39, 2, 3];
        assert!(
            translate_clientbound(
                key,
                VERSION,
                PLAY,
                clientbound::play::UPDATE_MOB_EFFECT.v26_3,
                &unsupported,
            )
            .is_none()
        );

        let supported = [1, 38, 2, 3];
        let translated = translate_clientbound(
            key,
            VERSION,
            PLAY,
            clientbound::play::UPDATE_MOB_EFFECT.v26_3,
            &supported,
        )
        .unwrap();
        assert_eq!(translated.payload, supported);
        remove_connection(key);
    }
}
