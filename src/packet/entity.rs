//! Remaps the entity type id in `ADD_ENTITY`.
//!
//! Fallback for when the full `CSpawnEntity` decode fails; rewrites only the type
//! field so it can't fail the same way.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::remap::entity_id_remap::remap_entity_id_for_version;

/// Number of bytes in the entity UUID that follows the entity id.
const UUID_LEN: usize = 16;

/// Rewrites the entity type in an `ADD_ENTITY` payload for `version`.
/// Layout is `varint entity_id`, `uuid`, `varint type`, then fields copied through untouched;
/// that prefix has been stable since 1.14, below which the type is a single byte in a different id space.
#[must_use]
pub fn remap_spawn_entity(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let entity_id = cursor.get_var_int().ok()?;

    if cursor.len() < UUID_LEN {
        return None;
    }
    let (uuid_bytes, after_uuid) = cursor.split_at(UUID_LEN);
    cursor = after_uuid;

    let type_id = cursor.get_var_int().ok()?.0;
    let remapped = remap_entity_id_for_version(u16::try_from(type_id).ok()?, version);

    let mut out = Vec::with_capacity(payload.len());
    out.write_var_int(&entity_id).ok()?;
    out.extend_from_slice(uuid_bytes);
    out.write_var_int(&VarInt(i32::from(remapped))).ok()?;
    out.extend_from_slice(cursor);
    Some(out)
}
