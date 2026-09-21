use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{F32T, OptionalT, STRING, VAR_INT};
use crate::api::{ComposedMappings, PacketWrapper, TranslateError, UserConnection};

/// 1.19.3 turned the sound event into an id or an inline event.
const FIRST_HOLDER: JavaMinecraftVersion = JavaMinecraftVersion::V_1_19_3;

fn mapped(id: i32, ids: &ComposedMappings) -> Option<i32> {
    u32::try_from(id)
        .ok()
        .and_then(|id| ids.sounds.map(id))
        .and_then(|id| i32::try_from(id).ok())
}

/// Copies the sound event across, renumbering it. `false` when the client has
/// no such sound, which drops the packet.
pub fn rewrite_holder(
    wrapper: &mut PacketWrapper,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<bool, TranslateError> {
    let id = wrapper.read(&VAR_INT)?.0;
    if layout < FIRST_HOLDER {
        let Some(id) = mapped(id, ids) else {
            return Ok(false);
        };
        wrapper.write(&VAR_INT, &VarInt(id))?;
        return Ok(true);
    }
    if id == 0 {
        // An inline event carries its own name, which needs no table.
        wrapper.write(&VAR_INT, &VarInt(0))?;
        wrapper.passthrough(&STRING)?;
        wrapper.passthrough(&OptionalT(F32T))?;
        return Ok(true);
    }
    let Some(id) = mapped(id - 1, ids) else {
        return Ok(false);
    };
    wrapper.write(&VAR_INT, &VarInt(id + 1))?;
    Ok(true)
}

/// The sound holder both `SOUND` and `SOUND_ENTITY` open with.
pub fn sound(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if rewrite_holder(wrapper, layout, ids)? {
        wrapper.passthrough_all();
    } else {
        wrapper.cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::packet::mappings::clientbound::play::SOUND;
    use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};

    /// Category, position, volume, pitch and seed: the rest of the packet,
    /// which the pass only copies.
    fn tail() -> Vec<u8> {
        let mut tail = vec![0u8; 1 + 12 + 4 + 4];
        tail.extend_from_slice(&42i64.to_be_bytes());
        tail
    }

    fn payload_for(id: i32, version: JavaMinecraftVersion) -> Vec<u8> {
        let mut payload = Vec::new();
        let wire = if version >= FIRST_HOLDER { id + 1 } else { id };
        payload.write_var_int(&VarInt(wire)).unwrap();
        payload.extend_from_slice(&tail());
        payload
    }

    fn run(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
        let ids = MappingData::get().composed(version);
        let mut wrapper = PacketWrapper::new(&SOUND, payload);
        let mut connection = UserConnection::new(0, version);
        sound(&mut wrapper, &mut connection, version, ids).unwrap();
        wrapper.finish().unwrap().map(|out| out.payload)
    }

    /// 26.2 and 1.21.4 take the holder, 1.18.2 the plain registry id;
    /// `sound_effect.rs` branches at 1.19.3 for exactly that.
    #[test]
    fn the_sound_id_is_renumbered_in_both_forms() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21_4,
            JavaMinecraftVersion::V_1_18_2,
        ] {
            let ids = MappingData::get().composed(version);
            let expected = i32::try_from(ids.sounds.map(700).unwrap()).unwrap();
            let out = run(&payload_for(700, version), version).unwrap();

            let mut cursor = out.as_slice();
            let wire = cursor.get_var_int().unwrap().0;
            let offset = i32::from(version >= FIRST_HOLDER);
            assert_eq!(wire - offset, expected, "{version}");
            assert_eq!(cursor, tail(), "{version}");
        }
    }

    #[test]
    fn an_inline_event_keeps_its_name() {
        let mut payload = vec![0, 4];
        payload.extend_from_slice(b"ping");
        payload.push(0);
        payload.extend_from_slice(&tail());
        assert_eq!(
            run(&payload, JavaMinecraftVersion::V_1_21_4).unwrap(),
            payload
        );
    }

    /// Sound 0 is one of the 103 the 1.16.2 registry has no stand in for, so
    /// the packet is dropped rather than sent under a wrong id.
    #[test]
    fn a_sound_the_client_lacks_drops_the_packet() {
        let version = JavaMinecraftVersion::V_1_16_2;
        assert!(MappingData::get().composed(version).sounds.map(0).is_none());
        assert!(run(&payload_for(0, version), version).is_none());
    }
}
