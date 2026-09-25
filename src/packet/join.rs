//! Rewrites the play `LOGIN` (join) and `RESPAWN` packets for clients that
//! have no configuration state.
//!
//! 1.16.2 to 1.20.1 get the whole registry set inline as an NBT "dimension codec",
//! replaced wholesale with the version's own data since 26.3's data would not decode.
//! Entry ids stay positional: an entry this version lacks keeps its slot with a stand-in.

use std::io::Cursor;

use pumpkin_nbt::Nbt;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

/// Oldest login/respawn layout for which Pumpkin's packet writers expose the
/// fields PJM tracks. Older schemas are validated and preserved as already
/// versioned by Pumpkin; the generated registry rewrite starts at 1.16.2.
pub const OLDEST_LAYOUT: JavaMinecraftVersion = JavaMinecraftVersion::V_1_14_4;

/// First version whose registries travel in the configuration state instead,
/// where `crate::registry::bundle_registry_packets` takes over.
pub const FIRST_WITH_CONFIG_STATE: JavaMinecraftVersion = JavaMinecraftVersion::V_1_20_2;

/// First version that names its dimension type instead of repeating the whole
/// element inline in join and respawn.
pub const FIRST_WITH_DIMENSION_NAME: JavaMinecraftVersion = JavaMinecraftVersion::V_1_19;

/// Reads one named NBT document off the front of `cursor`, returning the
/// document and leaving `cursor` just past it.
fn take_named_nbt(cursor: &mut &[u8]) -> Option<Nbt> {
    let mut reader = Cursor::new(*cursor);
    let nbt = Nbt::read(&mut NbtReadHelperJava::new(&mut reader)).ok()?;
    let used = usize::try_from(reader.position()).ok()?;
    *cursor = cursor.get(used..)?;
    Some(nbt)
}

/// Writes a compound as a named NBT document, the form every version below
/// 1.20.2 uses on the wire (core's `write_nbt_with_version` draws the same
/// line).
fn put_named_nbt(out: &mut Vec<u8>, name: &str, compound: NbtCompound) {
    out.extend_from_slice(&Nbt::new(name.to_string(), compound).write());
}

/// Writes a pre-serialized unnamed compound (a generated registry entry) as a
/// named document with an empty root name: the same bytes with a zero length
/// string spliced in after the tag id.
fn put_named_element(out: &mut Vec<u8>, element: &[u8]) -> Option<()> {
    // 0x0a is TAG_Compound; generated dimension type entries are always one.
    let (&tag_id, body) = element.split_first()?;
    if tag_id != 0x0a {
        return None;
    }
    out.push(tag_id);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(body);
    Some(())
}

/// Replaces the registry codec compound with `version`'s own, keeping the root
/// name the server used (core writes an empty one).
fn rewrite_codec(version: JavaMinecraftVersion, codec: Nbt, out: &mut Vec<u8>) -> Option<()> {
    let rewritten = crate::registry::rewrite_registry_codec(version, &codec.root_tag)?;
    put_named_nbt(out, &codec.name, rewritten);
    Some(())
}

/// Rewrites the play `LOGIN` payload for a client without a configuration
/// state. Returns `None` when the payload does not parse in full or there is
/// no registry data for `version`, in which case the packet is dropped.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn rewrite_login(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    if version < OLDEST_LAYOUT || version >= FIRST_WITH_CONFIG_STATE {
        return None;
    }
    if version < JavaMinecraftVersion::V_1_16 {
        validate_pre_1_16_login(payload, version)?;
        return Some(payload.to_vec());
    }
    // Pumpkin's CLogin writer already emits the 1.16/1.16.1 schema for these
    // clients, including the older combined hardcore/game-mode byte and named
    // dimension string. There is no generated registry-codec rewrite for this
    // tier, so validate and preserve its native bytes.
    if version < JavaMinecraftVersion::V_1_16_2 {
        validate_legacy_login(payload)?;
        return Some(payload.to_vec());
    }
    let inline_dimension = version < FIRST_WITH_DIMENSION_NAME;

    let mut read: &[u8] = payload;
    let entity_id = read.get_i32_be().ok()?;
    let is_hardcore = read.get_bool().ok()?;
    let game_mode = read.get_u8().ok()?;
    let previous_game_mode = read.get_i8().ok()?;

    let world_count = usize::try_from(read.get_var_int().ok()?.0).ok()?;
    let mut world_names = Vec::with_capacity(world_count.min(1024));
    for _ in 0..world_count {
        world_names.push(read.get_str().ok()?);
    }

    let codec = take_named_nbt(&mut read)?;
    // 1.16.2 to 1.18.2 write the dimension type element here instead of its name.
    if inline_dimension {
        take_named_nbt(&mut read)?;
    }
    let dimension_name = if inline_dimension {
        None
    } else {
        Some(read.get_str().ok()?)
    };
    let world_name = read.get_str().ok()?;
    let hashed_seed = read.get_i64_be().ok()?;
    let max_players = read.get_var_int().ok()?;
    let view_distance = read.get_var_int().ok()?;
    let simulation_distance = if version >= JavaMinecraftVersion::V_1_18 {
        Some(read.get_var_int().ok()?)
    } else {
        None
    };
    let reduced_debug_info = read.get_bool().ok()?;
    let enable_respawn_screen = read.get_bool().ok()?;
    let is_debug = read.get_bool().ok()?;
    let is_flat = read.get_bool().ok()?;
    let death = if version >= JavaMinecraftVersion::V_1_19 {
        read_death_location(&mut read)?
    } else {
        None
    };
    let portal_cooldown = if version >= JavaMinecraftVersion::V_1_20 {
        Some(read.get_var_int().ok()?)
    } else {
        None
    };
    if !read.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(payload.len());
    out.write_i32_be(entity_id).ok()?;
    out.write_bool(is_hardcore).ok()?;
    out.write_u8(game_mode).ok()?;
    out.write_i8(previous_game_mode).ok()?;
    out.write_var_int(&VarInt(i32::try_from(world_names.len()).ok()?))
        .ok()?;
    for name in &world_names {
        out.write_string(name).ok()?;
    }
    rewrite_codec(version, codec, &mut out)?;
    if inline_dimension {
        let element = crate::registry::dimension_type_element(version, &world_name)?;
        put_named_element(&mut out, element)?;
    } else {
        out.write_string(dimension_name.as_deref()?).ok()?;
    }
    out.write_string(&world_name).ok()?;
    out.write_i64_be(hashed_seed).ok()?;
    out.write_var_int(&max_players).ok()?;
    out.write_var_int(&view_distance).ok()?;
    if let Some(distance) = simulation_distance {
        out.write_var_int(&distance).ok()?;
    }
    out.write_bool(reduced_debug_info).ok()?;
    out.write_bool(enable_respawn_screen).ok()?;
    out.write_bool(is_debug).ok()?;
    out.write_bool(is_flat).ok()?;
    if version >= JavaMinecraftVersion::V_1_19 {
        write_death_location(&mut out, death.as_ref())?;
    }
    if let Some(cooldown) = portal_cooldown {
        out.write_var_int(&cooldown).ok()?;
    }
    Some(out)
}

/// Rewrites the play `RESPAWN` payload for 1.16.2 to 1.18.2, where the packet
/// opens with the dimension type element rather than its name. From 1.19 the
/// packet carries no NBT at all and needs no rewrite.
#[must_use]
pub fn rewrite_respawn(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    if version < OLDEST_LAYOUT || version >= FIRST_WITH_DIMENSION_NAME {
        return None;
    }
    if version < JavaMinecraftVersion::V_1_16 {
        validate_pre_1_16_respawn(payload, version)?;
        return Some(payload.to_vec());
    }
    // Pumpkin writes the 1.16/1.16.1 string-dimension form directly.
    if version < JavaMinecraftVersion::V_1_16_2 {
        validate_legacy_respawn(payload)?;
        return Some(payload.to_vec());
    }

    let mut read: &[u8] = payload;
    take_named_nbt(&mut read)?;
    let world_name = read.get_str().ok()?;
    let tail = read;
    // hashed seed, game mode, previous game mode, is debug, is flat, data kept
    if tail.len() != RESPAWN_TAIL_LEN {
        return None;
    }

    let mut out = Vec::with_capacity(payload.len());
    let element = crate::registry::dimension_type_element(version, &world_name)?;
    put_named_element(&mut out, element)?;
    out.write_string(&world_name).ok()?;
    out.extend_from_slice(tail);
    Some(out)
}

/// Bytes that follow the world name in a 1.16.2 to 1.18.2 respawn packet:
/// hashed seed (8), game mode, previous game mode, is debug, is flat and the
/// kept-data flag (1 each).
const RESPAWN_TAIL_LEN: usize = 8 + 5;

/// The bounds of the world a `LOGIN` payload names, as the server has it. The
/// sections of a chunk packet are laid out against those, whatever the client
/// itself thinks its world is.
#[must_use]
pub fn login_world_bounds(payload: &[u8], version: JavaMinecraftVersion) -> Option<(i32, i32)> {
    if version < OLDEST_LAYOUT || version >= FIRST_WITH_CONFIG_STATE {
        return None;
    }
    if version < JavaMinecraftVersion::V_1_16 {
        let dimension_id = legacy_login_dimension_id(payload, version)?;
        validate_pre_1_16_login(payload, version)?;
        return world_bounds(legacy_dimension_name(dimension_id));
    }
    let mut read: &[u8] = payload;
    read.get_i32_be().ok()?;
    if version >= JavaMinecraftVersion::V_1_16_2 {
        read.get_bool().ok()?;
        read.get_u8().ok()?;
    } else {
        read.get_u8().ok()?; // Combined hardcore/game-mode byte.
    }
    read.get_i8().ok()?;
    let world_count = usize::try_from(read.get_var_int().ok()?.0).ok()?;
    for _ in 0..world_count {
        read.get_str().ok()?;
    }
    take_named_nbt(&mut read)?;
    if version >= JavaMinecraftVersion::V_1_16_2 && version < FIRST_WITH_DIMENSION_NAME {
        take_named_nbt(&mut read)?;
    } else {
        read.get_str().ok()?; // Dimension identifier.
    }
    world_bounds(&read.get_str().ok()?)
}

/// The same for a `RESPAWN`, whose dimension element is followed by the world
/// name on every version below 1.19.
#[must_use]
pub fn respawn_world_bounds(payload: &[u8], version: JavaMinecraftVersion) -> Option<(i32, i32)> {
    if version < OLDEST_LAYOUT || version >= FIRST_WITH_DIMENSION_NAME {
        return None;
    }
    if version < JavaMinecraftVersion::V_1_16 {
        let mut read: &[u8] = payload;
        let dimension_id = read.get_i32_be().ok()?;
        validate_pre_1_16_respawn(payload, version)?;
        return world_bounds(legacy_dimension_name(dimension_id));
    }
    let mut read: &[u8] = payload;
    if version >= JavaMinecraftVersion::V_1_16_2 {
        take_named_nbt(&mut read)?;
    } else {
        read.get_str().ok()?; // Dimension identifier.
    }
    world_bounds(&read.get_str().ok()?)
}

fn validate_legacy_login(payload: &[u8]) -> Option<()> {
    let mut read: &[u8] = payload;
    read.get_i32_be().ok()?; // Entity id.
    read.get_u8().ok()?; // Hardcore flag plus game mode.
    read.get_i8().ok()?; // Previous game mode.
    let world_count = usize::try_from(read.get_var_int().ok()?.0).ok()?;
    if world_count > 1024 {
        return None;
    }
    for _ in 0..world_count {
        read.get_str().ok()?;
    }
    take_named_nbt(&mut read)?; // 1.16 dimension registry list.
    read.get_str().ok()?; // Dimension identifier.
    read.get_str().ok()?; // World name.
    read.get_i64_be().ok()?; // Hashed seed.
    read.get_u8().ok()?; // Max players.
    read.get_var_int().ok()?; // View distance.
    for _ in 0..4 {
        read.get_bool().ok()?;
    }
    read.is_empty().then_some(())
}

fn legacy_login_dimension_id(payload: &[u8], version: JavaMinecraftVersion) -> Option<i32> {
    let mut read: &[u8] = payload;
    read.get_i32_be().ok()?; // Entity id.
    read.get_u8().ok()?; // Hardcore flag plus game mode.
    if version >= JavaMinecraftVersion::V_1_9_1 {
        read.get_i32_be().ok()
    } else {
        read.get_i8().ok().map(i32::from)
    }
}

fn legacy_dimension_name(dimension_id: i32) -> &'static str {
    match dimension_id {
        -1 => "minecraft:the_nether",
        1 => "minecraft:the_end",
        _ => "minecraft:overworld",
    }
}

fn validate_pre_1_16_login(
    payload: &[u8],
    version: JavaMinecraftVersion,
) -> Option<()> {
    let mut read: &[u8] = payload;
    read.get_i32_be().ok()?; // Entity id.
    read.get_u8().ok()?; // Hardcore flag plus game mode.
    if version >= JavaMinecraftVersion::V_1_9_1 {
        read.get_i32_be().ok()?; // Dimension id.
    } else {
        read.get_i8().ok()?;
    }
    if version < JavaMinecraftVersion::V_1_14 {
        read.get_u8().ok()?; // Difficulty.
    }
    if version >= JavaMinecraftVersion::V_1_15 {
        read.get_i64_be().ok()?; // Hashed seed.
    }
    read.get_u8().ok()?; // Max players.
    read.get_str().ok()?; // Level type.
    if version >= JavaMinecraftVersion::V_1_14 {
        read.get_var_int().ok()?; // View distance.
    }
    if version >= JavaMinecraftVersion::V_1_8 {
        read.get_bool().ok()?; // Reduced debug info.
    }
    if version >= JavaMinecraftVersion::V_1_15 {
        read.get_bool().ok()?; // Enable respawn screen.
    }
    read.is_empty().then_some(())
}

fn validate_pre_1_16_respawn(
    payload: &[u8],
    version: JavaMinecraftVersion,
) -> Option<()> {
    let mut read: &[u8] = payload;
    read.get_i32_be().ok()?; // Dimension id.
    if version >= JavaMinecraftVersion::V_1_15 {
        read.get_i64_be().ok()?; // Hashed seed.
    } else if version < JavaMinecraftVersion::V_1_14 {
        read.get_u8().ok()?; // Difficulty.
    }
    read.get_u8().ok()?; // Game mode.
    read.get_str().ok()?; // Level type.
    read.is_empty().then_some(())
}

fn validate_legacy_respawn(payload: &[u8]) -> Option<()> {
    let mut read: &[u8] = payload;
    read.get_str().ok()?; // Dimension identifier.
    read.get_str().ok()?; // World name.
    read.get_i64_be().ok()?; // Hashed seed.
    read.get_u8().ok()?; // Game mode.
    read.get_i8().ok()?; // Previous game mode.
    for _ in 0..3 {
        read.get_bool().ok()?;
    }
    read.is_empty().then_some(())
}

fn world_bounds(name: &str) -> Option<(i32, i32)> {
    pumpkin_data::dimension::Dimension::from_name(name).map(|d| (d.min_y, d.height))
}

/// Reads the 1.19+ optional last death position: a flag, then a dimension name
/// and a packed `BlockPos` long.
#[allow(clippy::option_option)]
fn read_death_location(read: &mut &[u8]) -> Option<Option<(Box<str>, i64)>> {
    if !read.get_bool().ok()? {
        return Some(None);
    }
    let dimension = read.get_str().ok()?;
    let position = read.get_i64_be().ok()?;
    Some(Some((dimension, position)))
}

fn write_death_location(out: &mut Vec<u8>, death: Option<&(Box<str>, i64)>) -> Option<()> {
    match death {
        Some((dimension, position)) => {
            out.write_bool(true).ok()?;
            out.write_string(dimension).ok()?;
            out.write_i64_be(*position).ok()?;
        }
        None => out.write_bool(false).ok()?,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_nbt::tag::NbtTag;

    /// The 26.3 element core puts in every entry: NBT no old client decodes.
    fn server_element() -> NbtTag {
        let mut element = NbtCompound::new();
        element.put_string("marker", "from-server".to_string());
        NbtTag::Compound(element)
    }

    fn entry(name: &str, id: i32) -> NbtTag {
        let mut entry = NbtCompound::new();
        entry.put_string("name", name.to_string());
        entry.put_int("id", id);
        entry.put("element", server_element());
        NbtTag::Compound(entry)
    }

    fn registry(id: &str, entries: Vec<NbtTag>) -> NbtTag {
        let mut body = NbtCompound::new();
        body.put_string("type", id.to_string());
        body.put_list("value", entries);
        NbtTag::Compound(body)
    }

    fn server_codec() -> NbtCompound {
        let mut root = NbtCompound::new();
        root.put(
            "minecraft:worldgen/biome",
            registry(
                "minecraft:worldgen/biome",
                vec![
                    entry("minecraft:plains", 0),
                    // 26.3 only; no version in this tier has it.
                    entry("minecraft:pale_garden", 1),
                ],
            ),
        );
        root.put(
            "minecraft:dimension_type",
            registry(
                "minecraft:dimension_type",
                vec![entry("minecraft:overworld", 0)],
            ),
        );
        // 1.16.2 to 1.18.2 have no damage type registry at all.
        root.put(
            "minecraft:damage_type",
            registry("minecraft:damage_type", vec![entry("minecraft:arrow", 0)]),
        );
        root
    }

    /// Builds a join packet the way core writes it for `version`.
    fn server_login(version: JavaMinecraftVersion) -> Vec<u8> {
        let mut out = Vec::new();
        out.write_i32_be(7).unwrap();
        out.write_bool(false).unwrap();
        out.write_u8(0).unwrap();
        out.write_i8(-1).unwrap();
        out.write_var_int(&VarInt(1)).unwrap();
        out.write_string("minecraft:overworld").unwrap();
        put_named_nbt(&mut out, "", server_codec());
        if version < FIRST_WITH_DIMENSION_NAME {
            let mut dim = NbtCompound::new();
            dim.put_string("marker", "from-server".to_string());
            put_named_nbt(&mut out, "", dim);
        } else {
            out.write_string("minecraft:overworld").unwrap();
        }
        out.write_string("minecraft:overworld").unwrap();
        out.write_i64_be(1234).unwrap();
        out.write_var_int(&VarInt(20)).unwrap();
        out.write_var_int(&VarInt(10)).unwrap();
        if version >= JavaMinecraftVersion::V_1_18 {
            out.write_var_int(&VarInt(10)).unwrap();
        }
        out.write_bool(false).unwrap();
        out.write_bool(true).unwrap();
        out.write_bool(false).unwrap();
        out.write_bool(false).unwrap();
        if version >= JavaMinecraftVersion::V_1_19 {
            out.write_bool(false).unwrap();
        }
        if version >= JavaMinecraftVersion::V_1_20 {
            out.write_var_int(&VarInt(0)).unwrap();
        }
        out
    }

    fn legacy_login() -> Vec<u8> {
        let mut out = Vec::new();
        out.write_i32_be(7).unwrap();
        out.write_u8(0x08).unwrap(); // Hardcore plus survival.
        out.write_i8(-1).unwrap();
        out.write_var_int(&VarInt(1)).unwrap();
        out.write_string("minecraft:overworld").unwrap();
        put_named_nbt(&mut out, "", server_codec());
        out.write_string("minecraft:overworld").unwrap(); // Dimension identifier.
        out.write_string("minecraft:overworld").unwrap(); // World name.
        out.write_i64_be(1234).unwrap();
        out.write_u8(20).unwrap();
        out.write_var_int(&VarInt(10)).unwrap();
        for value in [false, true, false, false] {
            out.write_bool(value).unwrap();
        }
        out
    }

    fn legacy_respawn() -> Vec<u8> {
        let mut out = Vec::new();
        out.write_string("minecraft:overworld").unwrap(); // Dimension identifier.
        out.write_string("minecraft:overworld").unwrap(); // World name.
        out.write_i64_be(1234).unwrap();
        out.write_u8(0).unwrap(); // Game mode.
        out.write_i8(-1).unwrap(); // Previous game mode.
        for value in [false, false, true] {
            out.write_bool(value).unwrap();
        }
        out
    }

    fn pre_1_16_login(version: JavaMinecraftVersion) -> Vec<u8> {
        let mut out = Vec::new();
        out.write_i32_be(7).unwrap();
        out.write_u8(0x08).unwrap(); // Hardcore plus survival.
        if version >= JavaMinecraftVersion::V_1_9_1 {
            out.write_i32_be(-1).unwrap();
        } else {
            out.write_i8(-1).unwrap();
        }
        if version < JavaMinecraftVersion::V_1_14 {
            out.write_u8(2).unwrap(); // Difficulty.
        }
        if version >= JavaMinecraftVersion::V_1_15 {
            out.write_i64_be(1234).unwrap(); // Hashed seed.
        }
        out.write_u8(20).unwrap();
        out.write_string("default").unwrap();
        if version >= JavaMinecraftVersion::V_1_14 {
            out.write_var_int(&VarInt(10)).unwrap(); // View distance.
        }
        if version >= JavaMinecraftVersion::V_1_8 {
            out.write_bool(false).unwrap(); // Reduced debug info.
        }
        if version >= JavaMinecraftVersion::V_1_15 {
            out.write_bool(true).unwrap(); // Enable respawn screen.
        }
        out
    }

    fn pre_1_16_respawn(version: JavaMinecraftVersion) -> Vec<u8> {
        let mut out = Vec::new();
        out.write_i32_be(-1).unwrap(); // The Nether.
        if version >= JavaMinecraftVersion::V_1_15 {
            out.write_i64_be(1234).unwrap(); // Hashed seed.
        } else if version < JavaMinecraftVersion::V_1_14 {
            out.write_u8(2).unwrap(); // Difficulty.
        }
        out.write_u8(0).unwrap(); // Game mode.
        out.write_string("default").unwrap();
        out
    }

    /// Reads back a rewritten join packet: the codec and, below 1.19, the
    /// inline dimension element.
    fn parse_login(payload: &[u8], version: JavaMinecraftVersion) -> (NbtCompound, Option<Nbt>) {
        let mut read: &[u8] = payload;
        read.get_i32_be().unwrap();
        read.get_bool().unwrap();
        read.get_u8().unwrap();
        read.get_i8().unwrap();
        let count = read.get_var_int().unwrap().0;
        for _ in 0..count {
            read.get_str().unwrap();
        }
        let codec = take_named_nbt(&mut read).expect("codec");
        let inline = if version < FIRST_WITH_DIMENSION_NAME {
            Some(take_named_nbt(&mut read).expect("inline dimension"))
        } else {
            read.get_str().unwrap();
            None
        };
        (codec.root_tag, inline)
    }

    fn generated_element(version: JavaMinecraftVersion, registry: &str, name: &str) -> NbtTag {
        let entry = crate::registry::generated::get_synced(version)
            .expect("registry data")
            .iter()
            .find(|r| r.registry_id == registry)
            .expect("registry")
            .entries
            .iter()
            .find(|e| e.name == name)
            .expect("entry");
        let mut cursor = Cursor::new(entry.data);
        let mut reader = NbtReadHelperJava::new(&mut cursor);
        NbtTag::Compound(
            Nbt::read_unnamed(&mut reader)
                .expect("generated entry parses")
                .root_tag,
        )
    }

    const TIER: &[JavaMinecraftVersion] = &[
        JavaMinecraftVersion::V_1_16_2,
        JavaMinecraftVersion::V_1_16_3,
        JavaMinecraftVersion::V_1_16_4,
        JavaMinecraftVersion::V_1_17,
        JavaMinecraftVersion::V_1_17_1,
        JavaMinecraftVersion::V_1_18,
        JavaMinecraftVersion::V_1_18_2,
        JavaMinecraftVersion::V_1_19,
        JavaMinecraftVersion::V_1_19_1,
        JavaMinecraftVersion::V_1_19_3,
        JavaMinecraftVersion::V_1_19_4,
        JavaMinecraftVersion::V_1_20,
    ];

    #[test]
    fn native_1_16_login_is_validated_and_preserved() {
        for version in [
            JavaMinecraftVersion::V_1_16,
            JavaMinecraftVersion::V_1_16_1,
        ] {
            let payload = legacy_login();
            assert_eq!(rewrite_login(&payload, version).as_deref(), Some(payload.as_slice()));
            assert_eq!(
                login_world_bounds(&payload, version),
                world_bounds("minecraft:overworld")
            );

            let mut malformed = payload.clone();
            malformed.push(0);
            assert!(rewrite_login(&malformed, version).is_none());
        }
    }

    #[test]
    fn native_1_16_respawn_is_validated_and_preserved() {
        for version in [
            JavaMinecraftVersion::V_1_16,
            JavaMinecraftVersion::V_1_16_1,
        ] {
            let payload = legacy_respawn();
            assert_eq!(
                rewrite_respawn(&payload, version).as_deref(),
                Some(payload.as_slice())
            );
            assert_eq!(
                respawn_world_bounds(&payload, version),
                world_bounds("minecraft:overworld")
            );

            let mut malformed = payload.clone();
            malformed.push(0);
            assert!(rewrite_respawn(&malformed, version).is_none());
        }
    }

    #[test]
    fn native_pre_1_16_login_is_validated_and_preserved() {
        for version in [
            JavaMinecraftVersion::V_1_14_4,
            JavaMinecraftVersion::V_1_15_2,
        ] {
            let payload = pre_1_16_login(version);
            assert_eq!(rewrite_login(&payload, version).as_deref(), Some(payload.as_slice()));
            assert_eq!(
                login_world_bounds(&payload, version),
                world_bounds("minecraft:the_nether")
            );

            let mut malformed = payload.clone();
            malformed.push(0);
            assert!(rewrite_login(&malformed, version).is_none());
        }
    }

    #[test]
    fn native_pre_1_16_respawn_is_validated_and_preserved() {
        for version in [
            JavaMinecraftVersion::V_1_14_4,
            JavaMinecraftVersion::V_1_15_2,
        ] {
            let payload = pre_1_16_respawn(version);
            assert_eq!(
                rewrite_respawn(&payload, version).as_deref(),
                Some(payload.as_slice())
            );
            assert_eq!(
                respawn_world_bounds(&payload, version),
                world_bounds("minecraft:the_nether")
            );

            let mut malformed = payload.clone();
            malformed.push(0);
            assert!(rewrite_respawn(&malformed, version).is_none());
        }
    }

    #[test]
    fn every_tier_version_keeps_the_layout_and_replaces_the_codec() {
        for &version in TIER {
            let payload = server_login(version);
            let out = rewrite_login(&payload, version)
                .unwrap_or_else(|| panic!("{version} join rewritten"));

            let (codec, inline) = parse_login(&out, version);
            // Registries the version does not have are left out.
            assert_eq!(
                codec.get("minecraft:damage_type").is_some(),
                version >= JavaMinecraftVersion::V_1_19_4,
                "{version}: damage_type only from 1.19.4"
            );
            assert!(
                codec.get("minecraft:chat_type").is_none(),
                "{version}: the server sent no chat_type here"
            );

            let biome = codec
                .get_compound("minecraft:worldgen/biome")
                .expect("biome registry kept");
            let values = biome.get_list("value").expect("value list");
            assert_eq!(values.len(), 2, "{version}: entry count and order kept");
            let plains = generated_element(version, "worldgen/biome", "plains");
            for (index, (name, id)) in [("minecraft:plains", 0), ("minecraft:pale_garden", 1)]
                .into_iter()
                .enumerate()
            {
                let value = values[index].extract_compound().expect("entry compound");
                assert_eq!(value.get_string("name"), Some(name), "{version}: name kept");
                assert_eq!(value.get_int("id"), Some(id), "{version}: id kept");
                // The unknown 26.3 biome takes the stand-in, which is plains.
                assert_eq!(
                    value.get("element"),
                    Some(&plains),
                    "{version}: this version's own biome NBT"
                );
            }

            match inline {
                Some(inline) => {
                    assert!(
                        version < FIRST_WITH_DIMENSION_NAME,
                        "{version}: inline dimension only below 1.19"
                    );
                    assert_eq!(
                        NbtTag::Compound(inline.root_tag),
                        generated_element(version, "dimension_type", "overworld"),
                        "{version}: this version's own dimension type"
                    );
                }
                None => assert!(version >= FIRST_WITH_DIMENSION_NAME),
            }
        }
    }

    /// The part of a join packet from the world name on, which the rewrite
    /// must not touch.
    fn login_tail(payload: &[u8], version: JavaMinecraftVersion) -> Vec<u8> {
        let mut read: &[u8] = payload;
        read.get_i32_be().unwrap();
        read.get_bool().unwrap();
        read.get_u8().unwrap();
        read.get_i8().unwrap();
        let count = read.get_var_int().unwrap().0;
        for _ in 0..count {
            read.get_str().unwrap();
        }
        take_named_nbt(&mut read).expect("codec");
        if version < FIRST_WITH_DIMENSION_NAME {
            take_named_nbt(&mut read).expect("inline dimension");
        } else {
            read.get_str().unwrap();
        }
        read.to_vec()
    }

    #[test]
    fn tail_fields_survive_the_rewrite() {
        for &version in TIER {
            let payload = server_login(version);
            let out = rewrite_login(&payload, version).expect("rewritten");
            assert_eq!(
                login_tail(&out, version),
                login_tail(&payload, version),
                "{version}: everything from the world name on is untouched"
            );
        }
    }

    #[test]
    fn a_short_or_long_payload_is_dropped() {
        let version = JavaMinecraftVersion::V_1_18_2;
        let payload = server_login(version);
        assert!(
            rewrite_login(&payload[..payload.len() - 1], version).is_none(),
            "a truncated join packet is dropped, not half rewritten"
        );
        let mut extra = payload.clone();
        extra.push(0);
        assert!(
            rewrite_login(&extra, version).is_none(),
            "bytes left over mean this is not the layout we think it is"
        );
    }

    #[test]
    fn versions_outside_the_tier_are_not_touched() {
        let payload = server_login(JavaMinecraftVersion::V_1_20);
        assert!(rewrite_login(&payload, JavaMinecraftVersion::V_1_20_2).is_none());
        assert!(rewrite_login(&payload, JavaMinecraftVersion::V_1_16_1).is_none());
    }

    /// The respawn packet core writes for `version`.
    fn server_respawn(version: JavaMinecraftVersion) -> Vec<u8> {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_protocol::ClientPacket;
        use pumpkin_protocol::java::client::play::{CRespawn, PlayerSpawnData};

        let spawn_data = PlayerSpawnData::new(
            Dimension::THE_NETHER,
            1234,
            0,
            -1,
            false,
            false,
            None,
            VarInt(0),
            VarInt(63),
        );
        let mut out = Vec::new();
        CRespawn::new(spawn_data, CRespawn::KEEP_ALL_DATA)
            .write_packet_data(&mut out, &version)
            .unwrap_or_else(|e| panic!("{version}: core write failed: {e}"));
        out
    }

    #[test]
    fn respawn_dimension_is_this_versions_own() {
        for &version in TIER {
            let payload = server_respawn(version);
            let out = rewrite_respawn(&payload, version);
            if version >= FIRST_WITH_DIMENSION_NAME {
                assert!(out.is_none(), "{version}: respawn carries no NBT from 1.19");
                continue;
            }
            let out = out.unwrap_or_else(|| panic!("{version} respawn rewritten"));
            let mut read: &[u8] = &out;
            let inline = take_named_nbt(&mut read).expect("inline dimension");
            assert_eq!(
                NbtTag::Compound(inline.root_tag),
                generated_element(version, "dimension_type", "the_nether"),
                "{version}: the dimension named by the world name that follows"
            );
            assert_eq!(&*read.get_str().unwrap(), "minecraft:the_nether");
            assert_eq!(
                read.len(),
                RESPAWN_TAIL_LEN,
                "{version}: the tail is copied verbatim"
            );
        }
    }

    /// End to end against core's own writer, checking the parser here agrees
    /// with `CLogin::write_packet_data`.
    #[test]
    fn cores_own_join_packet_is_rewritten_for_every_tier_version() {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_protocol::ClientPacket;
        use pumpkin_protocol::java::client::play::{CLogin, PlayerSpawnData};

        for &version in TIER {
            let spawn_data = PlayerSpawnData::new(
                Dimension::OVERWORLD,
                1234,
                0,
                -1,
                false,
                false,
                None,
                VarInt(0),
                VarInt(63),
            );
            let dimension_names = ["minecraft:overworld".into()];
            let packet = CLogin::new(
                7,
                false,
                &dimension_names,
                VarInt(20),
                VarInt(10),
                VarInt(10),
                false,
                true,
                false,
                spawn_data,
                false,
                false,
            );
            let mut payload = Vec::new();
            packet
                .write_packet_data(&mut payload, &version)
                .unwrap_or_else(|e| panic!("{version}: core write failed: {e}"));

            let out = rewrite_login(&payload, version)
                .unwrap_or_else(|| panic!("{version}: core's own join packet must rewrite"));
            let (codec, inline) = parse_login(&out, version);

            let have: Vec<&str> = crate::registry::generated::get_synced(version)
                .expect("registry data")
                .iter()
                .map(|r| r.registry_id)
                .collect();
            for key in codec.child_tags.keys() {
                let bare = key.strip_prefix("minecraft:").unwrap_or(key);
                assert!(
                    have.contains(&bare),
                    "{version}: {key} survived but this version has no such registry"
                );
            }
            assert!(
                codec.get_compound("minecraft:worldgen/biome").is_some()
                    && codec.get_compound("minecraft:dimension_type").is_some(),
                "{version}: the two registries every version in the tier needs"
            );
            assert_eq!(
                codec.get_compound("minecraft:damage_type").is_some(),
                version >= JavaMinecraftVersion::V_1_19_4,
                "{version}: damage_type starts at 1.19.4"
            );
            assert_eq!(
                codec.get_compound("minecraft:chat_type").is_some(),
                version >= JavaMinecraftVersion::V_1_19,
                "{version}: chat_type starts at 1.19"
            );
            if let Some(inline) = inline {
                assert_eq!(
                    NbtTag::Compound(inline.root_tag),
                    generated_element(version, "dimension_type", "overworld"),
                    "{version}: the inline dimension is this version's own"
                );
            }
        }
    }

    #[test]
    fn a_respawn_with_a_wrong_tail_is_dropped() {
        let mut payload = server_respawn(JavaMinecraftVersion::V_1_16_2);
        payload.push(0);
        assert!(rewrite_respawn(&payload, JavaMinecraftVersion::V_1_16_2).is_none());
    }
}
