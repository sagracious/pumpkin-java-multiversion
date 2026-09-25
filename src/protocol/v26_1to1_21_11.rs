use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion as V;
use std::collections::HashSet;

use crate::api::connection::{GameTimeStorage, UserConnection};
use crate::api::types::{BOOL, F32T, I64T, NbtT, STRING, U8, VAR_INT, WireType};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError};
use crate::packet::mappings::{clientbound, serverbound};

const FIRST_VERSION_WITH_REGISTRY_TAGS: V = V::V_1_17;
const MAX_TAG_GROUPS: usize = 256;
const MAX_TAGS_PER_GROUP: usize = 4096;
const MAX_TAG_IDS: usize = 65_536;
const INTERACT: i32 = 0;
const ATTACK: i32 = 1;
const INTERACT_AT: i32 = 2;
const SPECTATOR_GAME_MODE: u8 = 3;

#[derive(Clone, Copy)]
struct GameModeStorage(u8);

pub struct Protocol26_1To1_21_11;

impl Protocol for Protocol26_1To1_21_11 {
    fn step(&self) -> Step {
        Step {
            from: V::V_26_1,
            to: V::V_1_21_11,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.cancel_clientbound(&clientbound::play::LOW_DISK_SPACE_WARNING);
        reg.cancel_clientbound(&clientbound::play::GAME_RULE_VALUES);
        reg.clientbound(&clientbound::play::SET_TIME, set_time);
        reg.clientbound(&clientbound::play::UPDATE_TAGS, rewrite_update_tags);
        reg.clientbound(&clientbound::config::UPDATE_TAGS, rewrite_update_tags);
        reg.clientbound(&clientbound::play::LOGIN, capture_login_game_mode);
        reg.clientbound(&clientbound::play::RESPAWN, capture_respawn_game_mode);
        reg.clientbound(&clientbound::play::GAME_EVENT, capture_game_event_mode);
        reg.serverbound(&serverbound::play::INTERACT, rewrite_interact);
    }
}

fn set_time(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let game_time = wrapper.read(&I64T)?;
    // Pumpkin's CUpdateTime writer already emits the target client's legacy
    // time layout (the 26.1 clock list is only written for 26.1+). Preserve it
    // and retain the game tick used by the 1.21.11 -> 1.21.9 anger-time rewrite.
    connection.put(GameTimeStorage { game_time });
    wrapper.write(&I64T, &game_time)?;
    wrapper.passthrough_all();
    Ok(())
}

fn capture_login_game_mode(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if let Some(mode) = login_game_mode(wrapper.remaining(), ctx.layout) {
        connection.put(GameModeStorage(mode));
    }
    wrapper.passthrough_all();
    Ok(())
}

fn capture_respawn_game_mode(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if let Some(mode) = respawn_game_mode(wrapper.remaining(), ctx.layout) {
        connection.put(GameModeStorage(mode));
    }
    wrapper.passthrough_all();
    Ok(())
}

fn capture_game_event_mode(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let mut cursor = wrapper.remaining();
    if let (Ok(event), Ok(value)) = (cursor.get_u8(), cursor.get_f32_be())
        && event == 3
        && value.is_finite()
    {
        connection.put(GameModeStorage(
            (value + 0.5).floor().clamp(0.0, 255.0) as u8
        ));
    }
    wrapper.passthrough_all();
    Ok(())
}

fn login_game_mode(payload: &[u8], version: V) -> Option<u8> {
    let mut cursor = payload;
    cursor.get_i32_be().ok()?; // Entity id.
    cursor.get_bool().ok()?; // Hardcore.
    if version < V::V_1_20_2 {
        return cursor.get_u8().ok();
    }
    let dimensions = checked_count(cursor.get_var_int().ok()?.0, 256, "login dimensions").ok()?;
    for _ in 0..dimensions {
        cursor.get_str().ok()?;
    }
    cursor.get_var_int().ok()?; // Max players.
    cursor.get_var_int().ok()?; // View distance.
    cursor.get_var_int().ok()?; // Simulation distance.
    cursor.get_bool().ok()?; // Reduced debug info.
    cursor.get_bool().ok()?; // Respawn screen.
    cursor.get_bool().ok()?; // Limited crafting.
    if version >= V::V_1_20_5 {
        cursor.get_var_int().ok()?; // Dimension type id.
    } else {
        cursor.get_str().ok()?; // Dimension type key.
    }
    cursor.get_str().ok()?; // Dimension name.
    cursor.get_i64_be().ok()?; // Hashed seed.
    cursor.get_u8().ok()
}

fn respawn_game_mode(payload: &[u8], version: V) -> Option<u8> {
    let mut cursor = payload;
    if version < V::V_1_20_5 {
        if version < V::V_1_19 && version >= V::V_1_16_2 {
            NbtT::for_version(version).read(&mut cursor).ok()?; // Dimension type NBT.
        } else {
            cursor.get_str().ok()?; // Dimension type.
        }
        cursor.get_str().ok()?; // Dimension name.
    } else {
        cursor.get_var_int().ok()?; // Dimension type.
        cursor.get_str().ok()?; // Dimension name.
    }
    cursor.get_i64_be().ok()?; // Hashed seed.
    cursor.get_u8().ok()
}

fn rewrite_interact(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    // 1.21.9-1.21.11 share this interaction body; earlier clients use their
    // own handlers in the older protocol steps.
    if connection.version < V::V_1_21_6 {
        wrapper.passthrough_all();
        return Ok(());
    }

    let entity_id = wrapper.read(&VAR_INT)?;
    let action = wrapper.read(&VAR_INT)?.0;
    match action {
        INTERACT => {
            // Vanilla first emits INTERACT_AT and then a redundant INTERACT.
            wrapper.cancel();
        }
        ATTACK => {
            wrapper.read(&BOOL)?; // Secondary action is not in the old packets.
            let spectator = connection
                .get::<GameModeStorage>()
                .is_some_and(|mode| mode.0 == SPECTATOR_GAME_MODE);
            wrapper.set_packet(if spectator {
                &serverbound::play::SPECTATE_ENTITY
            } else {
                &serverbound::play::ATTACK
            });
            wrapper.write(&VAR_INT, &entity_id)?;
        }
        INTERACT_AT => {
            let x = wrapper.read(&crate::api::types::F32T)?;
            let y = wrapper.read(&crate::api::types::F32T)?;
            let z = wrapper.read(&crate::api::types::F32T)?;
            let hand = wrapper.read(&VAR_INT)?;
            wrapper.write(&VAR_INT, &entity_id)?;
            wrapper.write_bytes(&low_precision_vector(x, y, z)?);
            wrapper.write(&VAR_INT, &hand)?;
        }
        _ => return Err(TranslateError::Unsupported("entity interaction action")),
    }
    wrapper.passthrough_all();
    Ok(())
}

fn low_precision_vector(x: f32, y: f32, z: f32) -> Result<Vec<u8>, TranslateError> {
    const MAX_PART: f64 = (1_u64 << 15) as f64 - 2.0;
    const ABS_MAX: f64 = (1_u64 << 34) as f64 - 1.0;
    const SCALE_MASK: u64 = 3;
    const CONTINUATION: u64 = 4;

    let sanitize = |value: f32| {
        if value.is_nan() {
            0.0
        } else {
            f64::from(value).clamp(-ABS_MAX, ABS_MAX)
        }
    };
    let values = [sanitize(x), sanitize(y), sanitize(z)];
    let max_value = values
        .iter()
        .fold(0.0_f64, |max, value| max.max(value.abs()));
    if max_value < 1.0 / MAX_PART {
        return Ok(vec![0]);
    }

    let scale = max_value.ceil() as u64;
    let low_scale = scale & SCALE_MASK;
    let (encoded_scale, continuation) = if low_scale != scale {
        (low_scale | CONTINUATION, Some(scale >> 2))
    } else {
        (scale, None)
    };
    let quantize =
        |value: f64| -> u64 { ((((value / scale as f64) * 0.5) + 0.5) * MAX_PART).round() as u64 };
    let packed = encoded_scale
        | (quantize(values[0]) << 3)
        | (quantize(values[1]) << 18)
        | (quantize(values[2]) << 33);

    let mut out = Vec::with_capacity(if continuation.is_some() { 11 } else { 7 });
    out.push(packed as u8);
    out.push((packed >> 8) as u8);
    out.extend_from_slice(&((packed >> 16) as u32).to_be_bytes());
    if let Some(continuation) = continuation {
        out.write_var_int(&VarInt(
            i32::try_from(continuation)
                .map_err(|_| TranslateError::Unsupported("low-precision vector scale"))?,
        ))
        .map_err(|_| TranslateError::Unsupported("low-precision vector scale"))?;
    }
    Ok(out)
}

fn rewrite_update_tags(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let payload = wrapper.remaining();
    let mut cursor = payload;
    let mut out = Vec::with_capacity(payload.len());
    if connection.version < FIRST_VERSION_WITH_REGISTRY_TAGS {
        for registry in ["block", "item", "fluid", "entity_type"] {
            rewrite_tag_list(&mut cursor, &mut out, registry)?;
        }
    } else {
        let group_count =
            checked_count(cursor.get_var_int()?.0, MAX_TAG_GROUPS, "tag group count")?;
        out.write_var_int(&VarInt(group_count as i32))?;
        for _ in 0..group_count {
            let registry = cursor.get_str()?;
            out.write_string(&registry)?;
            let bare = registry.strip_prefix("minecraft:").unwrap_or(&registry);
            rewrite_tag_list(&mut cursor, &mut out, bare)?;
        }
    }
    if !cursor.is_empty() {
        return Err(TranslateError::TrailingBytes(cursor.len()));
    }
    wrapper.replace_remaining(out);
    Ok(())
}

fn rewrite_tag_list(
    cursor: &mut &[u8],
    out: &mut Vec<u8>,
    registry: &str,
) -> Result<(), TranslateError> {
    let count = checked_count(cursor.get_var_int()?.0, MAX_TAGS_PER_GROUP, "tag count")?;
    let mut rewritten = Vec::with_capacity(count);
    let mut seen_names = HashSet::new();
    let mut renamed_names = HashSet::new();
    let mut total_ids = 0usize;
    for _ in 0..count {
        let source_name = cursor.get_str()?;
        let ids = checked_count(cursor.get_var_int()?.0, MAX_TAG_IDS, "tag id count")?;
        total_ids = total_ids
            .checked_add(ids)
            .filter(|total| *total <= MAX_TAG_IDS)
            .ok_or(TranslateError::Unsupported("tag group member count"))?;
        let target_name = if registry == "block" {
            old_block_tag(&source_name).map(str::to_owned)
        } else {
            None
        };
        let name = target_name.as_deref().unwrap_or(&source_name);
        // The mapping pass restores old-datapack tags before this protocol step.
        // If a renamed 26.1 tag maps to one of those names, keep the server's
        // renamed members and avoid writing a duplicate registry tag.
        let name = name.to_string();
        let seen = seen_names.contains(&name);
        let was_renamed = renamed_names.contains(&name);
        if seen && (target_name.is_some() || was_renamed) {
            for _ in 0..ids {
                cursor.get_var_int()?;
            }
            continue;
        }
        seen_names.insert(name.clone());
        if target_name.is_some() {
            renamed_names.insert(name.clone());
        }
        let mut members = Vec::with_capacity(ids);
        for _ in 0..ids {
            members.push(cursor.get_var_int()?.0);
        }
        rewritten.push((name, members));
    }
    out.write_var_int(&VarInt(
        i32::try_from(rewritten.len()).map_err(|_| TranslateError::Unsupported("tag count"))?,
    ))?;
    for (name, members) in rewritten {
        out.write_string(&name)?;
        out.write_var_int(&VarInt(
            i32::try_from(members.len())
                .map_err(|_| TranslateError::Unsupported("tag id count"))?,
        ))?;
        for id in members {
            out.write_var_int(&VarInt(id))?;
        }
    }
    Ok(())
}

fn old_block_tag(name: &str) -> Option<&'static str> {
    Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
        "supports_dry_vegetation" => "minecraft:dry_vegetation_may_place_on",
        "supports_bamboo" => "minecraft:bamboo_plantable_on",
        "supports_small_dripleaf" => "minecraft:small_dripleaf_placeable",
        "supports_big_dripleaf" => "minecraft:big_dripleaf_placeable",
        "overrides_mushroom_light_requirement" => "minecraft:mushroom_grow_block",
        "support_override_snow_layer" => "minecraft:snow_layer_can_survive_on",
        "cannot_support_snow_layer" => "minecraft:snow_layer_cannot_survive_on",
        _ => return None,
    })
}

fn checked_count(count: i32, max: usize, name: &'static str) -> Result<usize, TranslateError> {
    let count = usize::try_from(count).map_err(|_| TranslateError::Unsupported(name))?;
    if count > max {
        return Err(TranslateError::Unsupported(name));
    }
    Ok(count)
}

/// No translation is needed when a packet stays in its source form, but these
/// names let tests exercise the stateless output behavior through the registry.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::connection::remove_connection;
    use crate::api::types::WireType;
    use crate::packet::mappings::{clientbound, serverbound};

    fn direct(
        key: u64,
        version: V,
        packet: &'static crate::packet::mappings::PacketId,
        payload: &[u8],
        handler: crate::api::Handler,
    ) -> Option<crate::api::Translated> {
        crate::api::with_connection(key, version, |connection| {
            let mut wrapper = PacketWrapper::new(packet, payload);
            let context = Ctx {
                step: Protocol26_1To1_21_11.step(),
                mappings: MappingData::get().step(V::V_26_1),
                layout: V::V_26_3,
            };
            handler(&mut wrapper, connection, &context).unwrap();
            wrapper.finish().unwrap()
        })
    }

    #[test]
    fn unsupported_clientbound_packets_are_cancelled() {
        for packet in [
            &clientbound::play::LOW_DISK_SPACE_WARNING,
            &clientbound::play::GAME_RULE_VALUES,
        ] {
            let mut registry = Registry::default();
            Protocol26_1To1_21_11.register(&mut registry);
            let handler = registry.clientbound_handler(packet).unwrap().handler;
            assert!(direct(0x2111, V::V_1_21_11, packet, &[], handler).is_none());
            remove_connection(0x2111);
        }
    }

    #[test]
    fn set_time_preserves_pumpkins_legacy_layout_and_tracks_game_time() {
        for (index, version) in [V::V_1_21_11, V::V_1_20].into_iter().enumerate() {
            let key = 0x2111_01 + index as u64;
            let mut payload = Vec::new();
            I64T.write(&mut payload, &1000).unwrap();
            I64T.write(&mut payload, &24_000).unwrap();
            if version >= V::V_1_21_2 {
                BOOL.write(&mut payload, &true).unwrap();
            }
            let original = payload.clone();
            let (out, game_time) = crate::api::with_connection(key, version, |connection| {
                let mut wrapper = PacketWrapper::new(&clientbound::play::SET_TIME, &payload);
                let context = Ctx {
                    step: Protocol26_1To1_21_11.step(),
                    mappings: MappingData::get().step(V::V_26_1),
                    layout: V::V_26_3,
                };
                set_time(&mut wrapper, connection, &context).unwrap();
                (
                    wrapper.finish().unwrap().unwrap().payload,
                    connection.get::<GameTimeStorage>().unwrap().game_time,
                )
            });
            assert_eq!(out, original, "{version}");
            assert_eq!(game_time, 1000, "{version}");
            remove_connection(key);
        }
    }

    #[test]
    fn login_and_respawn_capture_the_game_mode_from_26_3_layouts() {
        let mut login = Vec::new();
        login.write_i32_be(42).unwrap();
        BOOL.write(&mut login, &false).unwrap();
        VAR_INT.write(&mut login, &VarInt(0)).unwrap(); // Dimension names.
        for value in [20, 10, 10] {
            VAR_INT.write(&mut login, &VarInt(value)).unwrap();
        }
        BOOL.write(&mut login, &false).unwrap();
        BOOL.write(&mut login, &true).unwrap();
        BOOL.write(&mut login, &false).unwrap();
        VAR_INT.write(&mut login, &VarInt(0)).unwrap(); // Dimension type.
        STRING
            .write(&mut login, &"minecraft:overworld".into())
            .unwrap();
        I64T.write(&mut login, &0).unwrap();
        U8.write(&mut login, &SPECTATOR_GAME_MODE).unwrap();
        assert_eq!(
            login_game_mode(&login, V::V_1_21_11),
            Some(SPECTATOR_GAME_MODE)
        );

        let mut respawn = Vec::new();
        VAR_INT.write(&mut respawn, &VarInt(0)).unwrap();
        STRING
            .write(&mut respawn, &"minecraft:overworld".into())
            .unwrap();
        I64T.write(&mut respawn, &0).unwrap();
        U8.write(&mut respawn, &2).unwrap();
        assert_eq!(respawn_game_mode(&respawn, V::V_1_21_11), Some(2));

        let mut old_login = Vec::new();
        old_login.write_i32_be(42).unwrap();
        BOOL.write(&mut old_login, &false).unwrap();
        U8.write(&mut old_login, &SPECTATOR_GAME_MODE).unwrap();
        assert_eq!(
            login_game_mode(&old_login, V::V_1_20),
            Some(SPECTATOR_GAME_MODE)
        );
    }

    #[test]
    fn game_mode_change_event_updates_spectator_interaction_state() {
        let key = 0x2111_07;
        let mut payload = Vec::new();
        payload.write_u8(3).unwrap();
        payload
            .write_f32_be(f32::from(SPECTATOR_GAME_MODE))
            .unwrap();
        crate::api::with_connection(key, V::V_1_21_11, |connection| {
            let mut wrapper = PacketWrapper::new(&clientbound::play::GAME_EVENT, &payload);
            let context = Ctx {
                step: Protocol26_1To1_21_11.step(),
                mappings: MappingData::get().step(V::V_26_1),
                layout: V::V_26_3,
            };
            capture_game_event_mode(&mut wrapper, connection, &context).unwrap();
            assert_eq!(
                connection.get::<GameModeStorage>().unwrap().0,
                SPECTATOR_GAME_MODE
            );
        });
        remove_connection(key);
    }

    fn named_tags() -> Vec<u8> {
        let mut out = Vec::new();
        VAR_INT.write(&mut out, &VarInt(2)).unwrap();
        for (registry, name) in [
            ("minecraft:block", "minecraft:supports_bamboo"),
            ("minecraft:item", "minecraft:supports_bamboo"),
        ] {
            STRING.write(&mut out, &registry.into()).unwrap();
            VAR_INT
                .write(
                    &mut out,
                    &VarInt(if registry == "minecraft:block" { 2 } else { 1 }),
                )
                .unwrap();
            STRING.write(&mut out, &name.into()).unwrap();
            VAR_INT.write(&mut out, &VarInt(2)).unwrap();
            VAR_INT.write(&mut out, &VarInt(3)).unwrap();
            VAR_INT.write(&mut out, &VarInt(4)).unwrap();
            if registry == "minecraft:block" {
                STRING
                    .write(&mut out, &"minecraft:bamboo_plantable_on".into())
                    .unwrap();
                VAR_INT.write(&mut out, &VarInt(1)).unwrap();
                VAR_INT.write(&mut out, &VarInt(9)).unwrap();
            }
        }
        out
    }

    #[test]
    fn update_tags_renames_only_the_block_registry_name() {
        let out = direct(
            0x2111_03,
            V::V_1_21_11,
            &clientbound::play::UPDATE_TAGS,
            &named_tags(),
            rewrite_update_tags,
        )
        .unwrap();
        let mut read = out.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(2));
        let block = STRING.read(&mut read).unwrap();
        assert_eq!(&*block, "minecraft:block");
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(1));
        assert_eq!(
            &*STRING.read(&mut read).unwrap(),
            "minecraft:bamboo_plantable_on"
        );
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(3));
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(4));
        assert_eq!(&*STRING.read(&mut read).unwrap(), "minecraft:item");
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(1));
        assert_eq!(
            &*STRING.read(&mut read).unwrap(),
            "minecraft:supports_bamboo"
        );
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(3));
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(4));
        assert!(read.is_empty());
        remove_connection(0x2111_03);
    }

    #[test]
    fn vanilla_interaction_action_is_cancelled() {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(8)).unwrap();
        VAR_INT.write(&mut payload, &VarInt(INTERACT)).unwrap();
        assert!(
            direct(
                0x2111_04,
                V::V_1_21_11,
                &serverbound::play::INTERACT,
                &payload,
                rewrite_interact,
            )
            .is_none()
        );
        remove_connection(0x2111_04);
    }

    #[test]
    fn attack_becomes_spectate_for_spectators_and_drops_the_secondary_flag() {
        let key = 0x2111_05;
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(8)).unwrap();
        VAR_INT.write(&mut payload, &VarInt(ATTACK)).unwrap();
        BOOL.write(&mut payload, &true).unwrap();
        crate::api::with_connection(key, V::V_1_21_11, |connection| {
            connection.put(GameModeStorage(SPECTATOR_GAME_MODE));
        });
        let out = direct(
            key,
            V::V_1_21_11,
            &serverbound::play::INTERACT,
            &payload,
            rewrite_interact,
        )
        .unwrap();
        assert_eq!(out.packet.v26_3, serverbound::play::SPECTATE_ENTITY.v26_3);
        assert_eq!(out.payload, [8]);
        remove_connection(key);
    }

    #[test]
    fn interact_at_is_encoded_as_a_low_precision_vector() {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(8)).unwrap();
        VAR_INT.write(&mut payload, &VarInt(INTERACT_AT)).unwrap();
        F32T.write(&mut payload, &1.0).unwrap();
        F32T.write(&mut payload, &0.0).unwrap();
        F32T.write(&mut payload, &-1.0).unwrap();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap();
        let out = direct(
            0x2111_06,
            V::V_1_21_11,
            &serverbound::play::INTERACT,
            &payload,
            rewrite_interact,
        )
        .unwrap();
        assert_eq!(out.packet.v26_3, serverbound::play::INTERACT.v26_3);
        assert_eq!(out.payload[0], 8);
        assert_eq!(out.payload[1] & 3, 1); // Scale 1.
        assert_eq!(out.payload.last(), Some(&1)); // Hand.
        remove_connection(0x2111_06);
    }
}
