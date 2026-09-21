use std::io::Cursor;

use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{I64T, U8T, VAR_INT, WireType};
use crate::api::{ComposedMappings, MappingData, PacketWrapper, TranslateError, UserConnection};

/// The block entity type became a registry id in 1.18; below it the field is
/// an action id core writes itself.
const FIRST_TYPE_ID: JavaMinecraftVersion = JavaMinecraftVersion::V_1_18;

fn mapped(id: i32, ids: &ComposedMappings) -> Option<i32> {
    u32::try_from(id)
        .ok()
        .and_then(|id| ids.blockentities.map(id))
        .and_then(|id| i32::try_from(id).ok())
}

pub fn block_entity_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I64T)?;
    if layout >= FIRST_TYPE_ID {
        let Some(id) = mapped(wrapper.read(&VAR_INT)?.0, ids) else {
            wrapper.cancel();
            return Ok(());
        };
        wrapper.write(&VAR_INT, &VarInt(id))?;
    }
    wrapper.passthrough_all();
    Ok(())
}

pub fn block_event(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I64T)?;
    wrapper.passthrough(&U8T)?;
    wrapper.passthrough(&U8T)?;
    let block = wrapper.read(&VAR_INT)?.0;
    let mapped = u32::try_from(block)
        .ok()
        .and_then(|id| ids.blocks.map(id))
        .and_then(|id| i32::try_from(id).ok());
    match mapped {
        Some(block) => wrapper.write(&VAR_INT, &VarInt(block))?,
        None => wrapper.cancel(),
    }
    Ok(())
}

/// One network NBT tag as it stands: named below 1.20.2, unnamed from it.
#[derive(Clone, Copy, Debug)]
pub struct RawNbtT {
    version: JavaMinecraftVersion,
}

impl RawNbtT {
    #[must_use]
    pub const fn for_version(version: JavaMinecraftVersion) -> Self {
        Self { version }
    }
}

impl WireType for RawNbtT {
    type Value = Vec<u8>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        let start = *r;
        let tag_id = r.get_u8()?;
        if tag_id != 0 {
            if self.version < JavaMinecraftVersion::V_1_20_2 {
                let name_len = usize::try_from(r.get_i16_be()?)
                    .map_err(|_| ReadingError::Message("negative nbt name".into()))?;
                *r = r
                    .get(name_len..)
                    .ok_or_else(|| ReadingError::Incomplete("nbt name".into()))?;
            }
            let body = *r;
            let mut cursor = Cursor::new(body);
            let mut reader = NbtReadHelperJava::new(&mut cursor);
            NbtTag::skip_data(&mut reader, tag_id)
                .map_err(|error| ReadingError::Message(error.to_string()))?;
            let used = usize::try_from(cursor.position())
                .map_err(|_| ReadingError::Message("nbt too long".into()))?;
            // A compound that ends on EOF rather than on its TAG_End was truncated.
            if tag_id == 10 && used.checked_sub(1).and_then(|last| body.get(last)) != Some(&0) {
                return Err(ReadingError::Incomplete("nbt".into()));
            }
            *r = &r[used..];
        }
        let consumed = start.len() - r.len();
        Ok(start[..consumed].to_vec())
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_slice(v)
    }
}

/// Renumbers the block entities a 1.18 and newer chunk packet carries and
/// leaves out the ones the client has no type for. Everything after them, the
/// light data, is copied as it stands.
#[must_use]
pub fn rewrite_chunk_block_entities(
    payload: &[u8],
    version: JavaMinecraftVersion,
) -> Option<Vec<u8>> {
    let ids = MappingData::get().composed(version);
    let nbt = RawNbtT::for_version(version);
    let mut cursor = payload;
    let count = cursor.get_var_int().ok()?.0;

    let mut kept = 0i32;
    let mut entries = Vec::new();
    for _ in 0..count {
        let packed_xz = cursor.get_u8().ok()?;
        let y = cursor.get_i16_be().ok()?;
        let id = mapped(cursor.get_var_int().ok()?.0, ids);

        let mut entry = Vec::new();
        entry.write_u8(packed_xz).ok()?;
        entry.write_i16_be(y).ok()?;
        entry.write_var_int(&VarInt(id.unwrap_or(0))).ok()?;
        entry.extend_from_slice(&nbt.read(&mut cursor).ok()?);
        if id.is_some() {
            kept += 1;
            entries.extend_from_slice(&entry);
        }
    }

    let mut out = Vec::with_capacity(payload.len());
    out.write_var_int(&VarInt(kept)).ok()?;
    out.extend_from_slice(&entries);
    out.extend_from_slice(cursor);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::mappings::clientbound::play::{BLOCK_ENTITY_DATA, BLOCK_EVENT};

    fn run(
        pass: fn(
            &mut PacketWrapper,
            &mut UserConnection,
            JavaMinecraftVersion,
            &ComposedMappings,
        ) -> Result<(), TranslateError>,
        packet: &'static crate::packet::mappings::PacketId,
        payload: &[u8],
        version: JavaMinecraftVersion,
    ) -> Option<Vec<u8>> {
        let ids = MappingData::get().composed(version);
        let mut wrapper = PacketWrapper::new(packet, payload);
        let mut connection = UserConnection::new(0, version);
        pass(&mut wrapper, &mut connection, version, ids).unwrap();
        wrapper.finish().unwrap().map(|out| out.payload)
    }

    fn block_entity_payload(id: u8) -> Vec<u8> {
        let mut payload = vec![0u8; 8];
        payload.push(id);
        // An empty compound, the shortest tag core writes.
        payload.push(0);
        payload
    }

    /// The type is a varint registry id from 1.18 (`block_entity_data.rs`
    /// branches there) and an action byte below it, which core owns.
    #[test]
    fn the_block_entity_type_is_renumbered_from_1_18_up() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_16_2,
        ] {
            let ids = MappingData::get().composed(version);
            let out = run(
                block_entity_data,
                &BLOCK_ENTITY_DATA,
                &block_entity_payload(9),
                version,
            )
            .unwrap();
            let expected = if version >= FIRST_TYPE_ID {
                u8::try_from(ids.blockentities.map(9).unwrap()).unwrap()
            } else {
                9
            };
            assert_eq!(out[8], expected, "{version}");
            assert_eq!(out.len(), 10, "{version}");
        }
    }

    /// Block entity 10 is one of the types no version below 26.1 has, so the
    /// packet goes rather than name a type the client cannot resolve.
    #[test]
    fn a_block_entity_the_client_lacks_drops_the_packet() {
        for version in [JavaMinecraftVersion::V_1_21, JavaMinecraftVersion::V_1_18] {
            assert!(
                run(
                    block_entity_data,
                    &BLOCK_ENTITY_DATA,
                    &block_entity_payload(10),
                    version,
                )
                .is_none(),
                "{version}"
            );
        }
        assert!(
            run(
                block_entity_data,
                &BLOCK_ENTITY_DATA,
                &block_entity_payload(10),
                JavaMinecraftVersion::V_26_2,
            )
            .is_some()
        );
    }

    #[test]
    fn the_block_event_block_is_renumbered() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_16_2,
        ] {
            let ids = MappingData::get().composed(version);
            let mut payload = vec![0u8; 10];
            payload.push(1);
            let out = run(block_event, &BLOCK_EVENT, &payload, version).unwrap();
            assert_eq!(
                out[10],
                u8::try_from(ids.blocks.map(1).unwrap()).unwrap(),
                "{version}"
            );
        }
    }

    #[test]
    fn a_block_the_client_lacks_drops_the_block_event() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let absent = i32::try_from(
            (0..MappingData::get().composed(version).blocks.len())
                .find(|id| {
                    MappingData::get()
                        .composed(version)
                        .blocks
                        .map(u32::try_from(*id).unwrap())
                        .is_none()
                })
                .unwrap(),
        )
        .unwrap();

        let mut payload = vec![0u8; 10];
        payload.write_var_int(&VarInt(absent)).unwrap();
        assert!(run(block_event, &BLOCK_EVENT, &payload, version).is_none());
    }

    /// Two block entities and a light tail, the shape core writes from 1.18
    /// (`net/java/chunk_data/v1_18.rs`).
    #[test]
    fn chunk_block_entities_are_renumbered_and_absent_ones_left_out() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let ids = MappingData::get().composed(version);
        assert!(ids.blockentities.map(10).is_none());

        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(2)).unwrap();
        for id in [9i32, 10] {
            payload.write_u8(0x21).unwrap();
            payload.write_i16_be(70).unwrap();
            payload.write_var_int(&VarInt(id)).unwrap();
            payload.write_u8(0).unwrap();
        }
        payload.extend_from_slice(b"light");

        let out = rewrite_chunk_block_entities(&payload, version).unwrap();
        let mut expected = Vec::new();
        expected.write_var_int(&VarInt(1)).unwrap();
        expected.write_u8(0x21).unwrap();
        expected.write_i16_be(70).unwrap();
        expected
            .write_var_int(&VarInt(
                i32::try_from(ids.blockentities.map(9).unwrap()).unwrap(),
            ))
            .unwrap();
        expected.write_u8(0).unwrap();
        expected.extend_from_slice(b"light");
        assert_eq!(out, expected);
    }
}
