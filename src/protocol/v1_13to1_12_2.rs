use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::text::TextComponent;
use pumpkin_util::translation::Locale;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{BOOL, I8, I32, I64, REMAINING_BYTES, STRING, TextComponentT, U8, VAR_INT};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::{clientbound, serverbound};

const V1_13: JavaMinecraftVersion = JavaMinecraftVersion::V_1_13;
const V1_12_2: JavaMinecraftVersion = JavaMinecraftVersion::V_1_12_2;
const V1_20_3: JavaMinecraftVersion = JavaMinecraftVersion::V_1_20_3;
const V26_2: JavaMinecraftVersion = JavaMinecraftVersion::V_26_2;
const VELOCITY_FORWARDING_CHANNEL: &str = "velocity:player_info";
const VINE_FORWARDING_CHANNEL: &str = "vine:player_info";

pub struct Protocol1_13To1_12_2;

impl Protocol for Protocol1_13To1_12_2 {
    fn step(&self) -> Step {
        Step {
            from: V1_13,
            to: V1_12_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        // The 1.12 recipe book, tags and advancement UI cannot represent these
        // 1.13 packets.
        reg.cancel_clientbound(&clientbound::play::TAG_QUERY);
        reg.cancel_clientbound(&clientbound::play::PLACE_GHOST_RECIPE);
        reg.cancel_clientbound(&clientbound::play::UNLOCK_RECIPES);
        reg.cancel_clientbound(&clientbound::play::UPDATE_RECIPES);
        reg.cancel_clientbound(&clientbound::play::UPDATE_TAGS);
        reg.cancel_clientbound(&clientbound::play::UPDATE_ADVANCEMENTS);
        reg.cancel_serverbound(&serverbound::play::PLACE_RECIPE);
        reg.cancel_serverbound(&serverbound::play::RECIPE_BOOK_DATA);

        reg.clientbound_layout(&clientbound::play::BLOCK_EVENT, block_event);
        reg.clientbound_layout(&clientbound::play::BLOCK_UPDATE, block_update);
        reg.clientbound_layout(&clientbound::play::LEVEL_EVENT, level_event);
        reg.clientbound_layout(&clientbound::play::SET_OBJECTIVE, objective);
        reg.clientbound_layout(&clientbound::play::SET_PLAYER_TEAM, team);
        reg.clientbound_layout(&clientbound::play::STOP_SOUND, stop_sound);
        // Pumpkin's map writer already emits the target layout; this handler
        // only filters icon ids 1.12.2 cannot display.
        reg.clientbound(&clientbound::play::MAP_ITEM_DATA, map_item_data);
        reg.clientbound_layout(
            &clientbound::play::CUSTOM_PAYLOAD,
            clientbound_plugin_message,
        );
        reg.serverbound(
            &serverbound::play::CUSTOM_PAYLOAD,
            serverbound_plugin_message,
        );
        reg.clientbound(&clientbound::login::CUSTOM_QUERY, login_custom_query);
    }
}

fn block_event(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I64)?;
    wrapper.passthrough(&U8)?;
    wrapper.passthrough(&U8)?;
    let block = wrapper.read(&VAR_INT)?.0;
    let Some(mapped) = u32::try_from(block)
        .ok()
        .and_then(|id| ctx.mappings.blocks.map(id))
        .and_then(|id| i32::try_from(id).ok())
    else {
        wrapper.consume_remaining();
        wrapper.cancel();
        return Ok(());
    };
    wrapper.write(&VAR_INT, &VarInt(mapped))?;
    Ok(())
}

fn block_update(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I64)?;
    let state = wrapper.read(&VAR_INT)?.0;
    let mapped = u32::try_from(state)
        .ok()
        .and_then(|id| ctx.mappings.blockstates.map(id))
        .and_then(|id| i32::try_from(id).ok())
        .unwrap_or(0);
    wrapper.write(&VAR_INT, &VarInt(mapped))?;
    Ok(())
}

fn level_event(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let event = wrapper.passthrough(&I32)?;
    wrapper.passthrough(&I64)?;
    let data = wrapper.read(&I32)?;
    let mapped = match event {
        1010 => u32::try_from(data)
            .ok()
            .and_then(|id| ctx.mappings.items.map(id))
            .and_then(|id| i32::try_from(id).ok())
            .map(|id| id >> 4)
            .unwrap_or(-1),
        2001 => u32::try_from(data)
            .ok()
            .and_then(|id| ctx.mappings.blockstates.map(id))
            .and_then(|id| i32::try_from(id).ok())
            .unwrap_or(0),
        _ => data,
    };
    wrapper.write(&I32, &mapped)?;
    Ok(())
}

fn objective(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&STRING)?;
    let action = wrapper.passthrough(&U8)?;
    if action == 0 || action == 2 {
        let title = wrapper.read(&TextComponentT::for_version(ctx.layout))?;
        let legacy = legacy_text(title, ctx.step.to, 32);
        let render_type = wrapper.read(&VAR_INT)?.0;
        if ctx.layout >= V1_20_3 {
            // Modern Pumpkin appends the 1.20.3+ number-format holder. The
            // 1.12 objective packet has no field for it.
            wrapper.consume_remaining();
        }
        wrapper.write(&STRING, &legacy.into())?;
        let legacy_render = if render_type == 1 {
            "hearts"
        } else {
            "integer"
        };
        wrapper.write(&STRING, &legacy_render.into())?;
    }
    Ok(())
}

fn team(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&STRING)?;
    let action = wrapper.passthrough(&U8)?;
    if action == 0 || action == 2 {
        let text = TextComponentT::for_version(ctx.layout);
        let display = wrapper.read(&text)?;
        let (prefix, suffix, options, visibility, collision, color) = if ctx.layout >= V26_2 {
            let prefix = wrapper.read(&text)?;
            let suffix = wrapper.read(&text)?;
            let visibility = legacy_visibility(wrapper.read(&VAR_INT)?.0);
            let collision = legacy_collision(wrapper.read(&VAR_INT)?.0);
            let color = if wrapper.read(&BOOL)? {
                wrapper.read(&VAR_INT)?.0
            } else {
                -1
            };
            let options = wrapper.read(&I8)?;
            (prefix, suffix, options, visibility, collision, color)
        } else {
            let options = wrapper.read(&I8)?;
            let visibility: String = wrapper.read(&STRING)?.into();
            let collision: String = wrapper.read(&STRING)?.into();
            let color = wrapper.read(&VAR_INT)?.0;
            let prefix = wrapper.read(&text)?;
            let suffix = wrapper.read(&text)?;
            (prefix, suffix, options, visibility, collision, color)
        };

        wrapper.write(&STRING, &legacy_text(display, ctx.step.to, 32).into())?;
        wrapper.write(&I8, &options)?;
        wrapper.write(&STRING, &visibility.into())?;
        wrapper.write(&STRING, &collision.into())?;
        wrapper.write(
            &I8,
            &(if (0..=15).contains(&color) {
                color as i8
            } else {
                -1
            }),
        )?;
        wrapper.write(&STRING, &legacy_text(prefix, ctx.step.to, 16).into())?;
        wrapper.write(&STRING, &legacy_text(suffix, ctx.step.to, 16).into())?;
    }
    wrapper.passthrough_all();
    Ok(())
}

fn legacy_visibility(id: i32) -> String {
    match id {
        1 => "never",
        2 => "hideForOtherTeams",
        3 => "hideForOwnTeam",
        _ => "always",
    }
    .to_string()
}

fn legacy_collision(id: i32) -> String {
    match id {
        1 => "never",
        2 => "pushOtherTeams",
        3 => "pushOwnTeam",
        _ => "always",
    }
    .to_string()
}

fn legacy_text(component: TextComponent, version: JavaMinecraftVersion, limit: usize) -> String {
    let text = component.to_legacy_string_for_version(&version, Locale::EnUs);
    let mut out = String::with_capacity(text.len().min(limit));
    let mut chars = text.chars();
    let mut used = 0;
    while let Some(character) = chars.next() {
        if used >= limit {
            break;
        }
        if character == '§' {
            let Some(code) = chars.next() else {
                out.push(character);
                break;
            };
            if used + 2 > limit {
                break;
            }
            out.push(character);
            out.push(code);
            used += 2;
        } else {
            out.push(character);
            used += 1;
        }
    }
    out
}

fn stop_sound(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    const SOURCES: [&str; 10] = [
        "master", "music", "record", "weather", "block", "hostile", "neutral", "player", "ambient",
        "voice",
    ];

    wrapper.set_packet(&clientbound::play::CUSTOM_PAYLOAD);
    wrapper.write(&STRING, &"MC|StopSound".into())?;
    let flags = wrapper.read(&U8)?;
    let source = if flags & 1 != 0 {
        let id = usize::try_from(wrapper.read(&VAR_INT)?.0)
            .map_err(|_| TranslateError::Unsupported("sound source"))?;
        SOURCES
            .get(id)
            .copied()
            .ok_or(TranslateError::Unsupported("sound source"))?
    } else {
        ""
    };
    let sound = if flags & 2 != 0 {
        wrapper.read(&STRING)?
    } else {
        "".into()
    };
    wrapper.write(&STRING, &source.into())?;
    wrapper.write(&STRING, &sound)?;
    Ok(())
}

fn map_item_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V1_12_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&I8)?;
    wrapper.passthrough(&BOOL)?;
    let count = wrapper.read(&VAR_INT)?.0;
    let count = usize::try_from(count)
        .map_err(|_| TranslateError::Unsupported("negative map icon count"))?;
    let mut icons = Vec::with_capacity(count.min(128));
    for _ in 0..count {
        let icon = wrapper.read(&U8)?;
        let x = wrapper.read(&I8)?;
        let z = wrapper.read(&I8)?;
        if icon >> 4 <= 9 {
            icons.push((icon, x, z));
        }
    }
    let kept = i32::try_from(icons.len())
        .map_err(|_| TranslateError::Unsupported("too many map icons"))?;
    wrapper.write(&VAR_INT, &VarInt(kept))?;
    for (icon, x, z) in icons {
        wrapper.write(&U8, &icon)?;
        wrapper.write(&I8, &x)?;
        wrapper.write(&I8, &z)?;
    }
    wrapper.passthrough_all();
    Ok(())
}

fn channel_to_new(channel: &str) -> Option<String> {
    let known = match channel {
        "MC|TrList" => "minecraft:trader_list",
        "MC|Brand" => "minecraft:brand",
        "MC|BOpen" => "minecraft:book_open",
        "MC|DebugPath" => "minecraft:debug/paths",
        "MC|DebugNeighborsUpdate" => "minecraft:debug/neighbors_update",
        "REGISTER" => "minecraft:register",
        "UNREGISTER" => "minecraft:unregister",
        "BungeeCord" => "bungeecord:main",
        _ => "",
    };
    if !known.is_empty() {
        return Some(known.to_string());
    }
    valid_channel(channel).then(|| {
        if channel.contains(':') {
            channel.to_string()
        } else {
            format!("minecraft:{channel}")
        }
    })
}

fn channel_to_old(channel: &str) -> Option<String> {
    let known = match channel {
        "minecraft:trader_list" => "MC|TrList",
        "minecraft:brand" => "MC|Brand",
        "minecraft:book_open" => "MC|BOpen",
        "minecraft:debug/paths" => "MC|DebugPath",
        "minecraft:debug/neighbors_update" => "MC|DebugNeighborsUpdate",
        "minecraft:register" => "REGISTER",
        "minecraft:unregister" => "UNREGISTER",
        "bungeecord:main" => "BungeeCord",
        _ => "",
    };
    if !known.is_empty() {
        return Some(known.to_string());
    }
    if !valid_channel(channel) {
        return None;
    }
    let length = channel.chars().count();
    Some(if length > 20 {
        channel.chars().take(20).collect()
    } else {
        channel.to_string()
    })
}

fn valid_channel(channel: &str) -> bool {
    if channel.is_empty() || channel.chars().count() > 32767 {
        return false;
    }
    let mut parts = channel.split(':');
    let namespace = parts.next().unwrap_or_default();
    let path = parts.next();
    if parts.next().is_some() {
        return false;
    }
    let (namespace, path) = match path {
        Some(path) => (namespace, path),
        None => ("minecraft", namespace),
    };
    !namespace.is_empty()
        && !path.is_empty()
        && namespace
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c))
        && path
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-/".contains(c))
}

fn rewrite_channel_list(
    wrapper: &mut PacketWrapper,
    map_channel: fn(&str) -> Option<String>,
) -> Result<(), TranslateError> {
    let bytes = wrapper.read(&REMAINING_BYTES)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| TranslateError::Unsupported("plugin channel list encoding"))?;
    let rewritten = text
        .split('\0')
        .filter_map(map_channel)
        .collect::<Vec<_>>()
        .join("\0");
    if rewritten.is_empty() {
        wrapper.cancel();
        return Ok(());
    }
    wrapper.write(&REMAINING_BYTES, &rewritten.into_bytes())?;
    Ok(())
}

fn clientbound_plugin_message(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let channel: String = wrapper.read(&STRING)?.into();
    if channel == "minecraft:trader_list" {
        wrapper.consume_remaining();
        wrapper.cancel();
        return Ok(());
    }
    let Some(old_channel) = channel_to_old(&channel) else {
        wrapper.consume_remaining();
        wrapper.cancel();
        return Ok(());
    };
    wrapper.write(&STRING, &old_channel.clone().into())?;
    if old_channel == "REGISTER" || old_channel == "UNREGISTER" {
        rewrite_channel_list(wrapper, channel_to_old)
    } else if old_channel == "MC|BOpen" {
        let hand = wrapper.read(&VAR_INT)?.0;
        let hand = u8::try_from(hand).map_err(|_| TranslateError::Unsupported("book hand"))?;
        wrapper.write(&U8, &hand)?;
        Ok(())
    } else {
        wrapper.passthrough_all();
        Ok(())
    }
}

fn serverbound_plugin_message(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let channel: String = wrapper.read(&STRING)?.into();
    match channel.as_str() {
        "MC|BEdit" | "MC|BSign" | "MC|AdvCmd" | "MC|AutoCmd" | "MC|Struct" => {
            wrapper.consume_remaining();
            wrapper.cancel();
        }
        "MC|ItemName" => {
            wrapper.set_packet(&serverbound::play::RENAME_ITEM);
            wrapper.passthrough_all();
        }
        "MC|Beacon" => {
            wrapper.set_packet(&serverbound::play::SET_BEACON);
            let primary = wrapper.read(&I32)?;
            let secondary = wrapper.read(&I32)?;
            wrapper.write(&VAR_INT, &VarInt(primary))?;
            wrapper.write(&VAR_INT, &VarInt(secondary))?;
        }
        "MC|TrSel" => {
            wrapper.set_packet(&serverbound::play::SELECT_TRADE);
            let slot = wrapper.read(&I32)?;
            wrapper.write(&VAR_INT, &VarInt(slot))?;
        }
        "MC|PickItem" => {
            wrapper.set_packet(&serverbound::play::PICK_ITEM);
            wrapper.passthrough_all();
        }
        _ => {
            let Some(new_channel) = channel_to_new(&channel) else {
                wrapper.consume_remaining();
                wrapper.cancel();
                return Ok(());
            };
            wrapper.write(&STRING, &new_channel.clone().into())?;
            if new_channel == "minecraft:register" || new_channel == "minecraft:unregister" {
                rewrite_channel_list(wrapper, channel_to_new)?;
            } else {
                wrapper.passthrough_all();
            }
        }
    }
    Ok(())
}

fn login_custom_query(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let id = wrapper.read(&VAR_INT)?;
    let channel = wrapper.read(&STRING)?;
    if matches!(
        channel.as_ref(),
        VELOCITY_FORWARDING_CHANNEL | VINE_FORWARDING_CHANNEL
    ) {
        // These login queries authenticate the proxy connection, not a 1.13
        // client feature. Let Velocity/Vine answer them unchanged.
        wrapper.write(&VAR_INT, &id)?;
        wrapper.write(&STRING, &channel)?;
        wrapper.passthrough_all();
        return Ok(());
    }
    wrapper.consume_remaining();
    wrapper.send_serverbound(
        &serverbound::login::CUSTOM_QUERY_ANSWER,
        negative_login_query_answer(id)?,
    );
    wrapper.cancel();
    Ok(())
}

/// A 1.12 client cannot answer the 1.13 login plugin query. Return a negative
/// response on the serverbound login path so the pending connection can proceed.
fn negative_login_query_answer(id: VarInt) -> Result<Vec<u8>, TranslateError> {
    let mut reply = Vec::new();
    VAR_INT.write(&mut reply, &id)?;
    BOOL.write(&mut reply, &false)?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::api::types::WireType;

    fn context(layout: JavaMinecraftVersion) -> Ctx<'static> {
        Ctx {
            step: Protocol1_13To1_12_2.step(),
            mappings: MappingData::get().step(V1_13),
            layout,
        }
    }

    #[test]
    fn map_icons_use_the_legacy_packed_byte_and_drop_new_types() {
        // map id, scale, tracking, two icons: type 2/dir 3 and type 12.
        let input = [7, 1, 1, 2, 0x23, 4, 5, 0xc1, 6, 7, 0];
        let mut wrapper = PacketWrapper::new(&clientbound::play::MAP_ITEM_DATA, &input);
        map_item_data(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V1_12_2),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        assert_eq!(output, [7, 1, 1, 1, 0x23, 4, 5, 0]);
    }

    #[test]
    fn scoreboard_objective_components_become_legacy_text_and_render_names() {
        let mut input = Vec::new();
        STRING.write(&mut input, &"objective".into()).unwrap();
        U8.write(&mut input, &0).unwrap();
        TextComponentT::for_version(V1_13)
            .write(&mut input, &TextComponent::text("Score"))
            .unwrap();
        VAR_INT.write(&mut input, &VarInt(1)).unwrap();
        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_OBJECTIVE, &input);
        objective(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V1_13),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(&*STRING.read(&mut read).unwrap(), "objective");
        assert_eq!(U8.read(&mut read).unwrap(), 0);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "Score");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "hearts");
        assert!(read.is_empty());
    }

    #[test]
    fn modern_objective_number_format_is_consumed() {
        let mut input = Vec::new();
        STRING.write(&mut input, &"objective".into()).unwrap();
        U8.write(&mut input, &0).unwrap();
        TextComponentT::for_version(V26_2)
            .write(&mut input, &TextComponent::text("Score"))
            .unwrap();
        VAR_INT.write(&mut input, &VarInt(0)).unwrap();
        VAR_INT.write(&mut input, &VarInt(0)).unwrap(); // blank NumberFormat
        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_OBJECTIVE, &input);
        objective(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V26_2),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(&*STRING.read(&mut read).unwrap(), "objective");
        assert_eq!(U8.read(&mut read).unwrap(), 0);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "Score");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "integer");
        assert!(read.is_empty());
    }

    #[test]
    fn team_components_become_legacy_fields_and_invalid_color_becomes_reset() {
        let mut input = Vec::new();
        STRING.write(&mut input, &"team".into()).unwrap();
        U8.write(&mut input, &0).unwrap();
        for text in ["Display", "§a", "§b"] {
            TextComponentT::for_version(V1_13)
                .write(&mut input, &TextComponent::text(text))
                .unwrap();
            if text == "Display" {
                U8.write(&mut input, &0).unwrap();
                STRING.write(&mut input, &"always".into()).unwrap();
                STRING.write(&mut input, &"always".into()).unwrap();
                VAR_INT.write(&mut input, &VarInt(21)).unwrap();
            }
        }
        input.extend_from_slice(&[0]); // no members

        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_PLAYER_TEAM, &input);
        team(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V1_13),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(&*STRING.read(&mut read).unwrap(), "team");
        assert_eq!(U8.read(&mut read).unwrap(), 0);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "Display");
        assert_eq!(U8.read(&mut read).unwrap(), 0);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "always");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "always");
        assert_eq!(I8.read(&mut read).unwrap(), -1);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "§a");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "§b");
        assert_eq!(read, [0]);
    }

    #[test]
    fn modern_team_order_becomes_legacy_team_fields() {
        let mut input = Vec::new();
        STRING.write(&mut input, &"team".into()).unwrap();
        U8.write(&mut input, &2).unwrap();
        let text = TextComponentT::for_version(V26_2);
        text.write(&mut input, &TextComponent::text("Display"))
            .unwrap();
        text.write(&mut input, &TextComponent::text("§a")).unwrap();
        text.write(&mut input, &TextComponent::text("§b")).unwrap();
        VAR_INT.write(&mut input, &VarInt(2)).unwrap(); // hideForOtherTeams
        VAR_INT.write(&mut input, &VarInt(3)).unwrap(); // pushOwnTeam
        BOOL.write(&mut input, &true).unwrap();
        VAR_INT.write(&mut input, &VarInt(21)).unwrap();
        I8.write(&mut input, &1).unwrap();

        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_PLAYER_TEAM, &input);
        team(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V26_2),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(&*STRING.read(&mut read).unwrap(), "team");
        assert_eq!(U8.read(&mut read).unwrap(), 2);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "Display");
        assert_eq!(I8.read(&mut read).unwrap(), 1);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "hideForOtherTeams");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "pushOwnTeam");
        assert_eq!(I8.read(&mut read).unwrap(), -1);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "§a");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "§b");
        assert!(read.is_empty());
    }

    #[test]
    fn legacy_plugin_channels_map_to_namespaced_ids() {
        assert_eq!(
            channel_to_new("MC|Brand").as_deref(),
            Some("minecraft:brand")
        );
        assert_eq!(
            channel_to_new("BungeeCord").as_deref(),
            Some("bungeecord:main")
        );
        assert_eq!(
            channel_to_old("minecraft:brand").as_deref(),
            Some("MC|Brand")
        );
        assert_eq!(channel_to_new("not|valid"), None);
    }

    #[test]
    fn clientbound_brand_channel_keeps_its_payload() {
        let mut input = Vec::new();
        STRING.write(&mut input, &"minecraft:brand".into()).unwrap();
        STRING.write(&mut input, &"Pumpkin".into()).unwrap();
        let mut wrapper = PacketWrapper::new(&clientbound::play::CUSTOM_PAYLOAD, &input);
        clientbound_plugin_message(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V1_13),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(&*STRING.read(&mut read).unwrap(), "MC|Brand");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "Pumpkin");
        assert!(read.is_empty());
    }

    #[test]
    fn unsupported_recipe_packets_are_registered_for_cancellation() {
        let mut reg = Registry::default();
        Protocol1_13To1_12_2.register(&mut reg);
        let registered = reg
            .clientbound_handler(&clientbound::play::UPDATE_RECIPES)
            .expect("cancel handler");
        let mut wrapper = PacketWrapper::new(&clientbound::play::UPDATE_RECIPES, &[]);
        (registered.handler)(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V1_13),
        )
        .unwrap();
        assert!(wrapper.is_cancelled());
    }

    #[test]
    fn login_query_response_uses_the_serverbound_negative_answer_shape() {
        let payload = negative_login_query_answer(VarInt(37)).unwrap();
        let mut read = payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 37);
        assert!(!BOOL.read(&mut read).unwrap());
        assert!(read.is_empty());
        assert_ne!(
            serverbound::login::CUSTOM_QUERY_ANSWER.to_id(V1_12_2),
            -1,
            "the old client may decline a plugin query"
        );
    }

    #[test]
    fn canceled_login_query_queues_the_followup_toward_the_server() {
        let mut input = Vec::new();
        VAR_INT.write(&mut input, &VarInt(37)).unwrap();
        STRING.write(&mut input, &"minecraft:brand".into()).unwrap();
        input.extend_from_slice(b"ignored query data");
        let mut wrapper = PacketWrapper::new(&clientbound::login::CUSTOM_QUERY, &input);
        login_custom_query(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V1_13),
        )
        .unwrap();

        let output = wrapper.finish_with_outputs().unwrap();
        assert!(output.cancelled);
        assert_eq!(output.serverbound.len(), 1);
        assert_eq!(
            output.serverbound[0].0.to_id(V1_12_2),
            serverbound::login::CUSTOM_QUERY_ANSWER.to_id(V1_12_2)
        );
        assert_eq!(output.serverbound[0].1, [37, 0]);
    }

    #[test]
    fn forwarding_authentication_queries_pass_through_unchanged() {
        for channel in [VELOCITY_FORWARDING_CHANNEL, VINE_FORWARDING_CHANNEL] {
            let mut input = Vec::new();
            VAR_INT.write(&mut input, &VarInt(37)).unwrap();
            STRING.write(&mut input, &channel.into()).unwrap();
            input.extend_from_slice(b"forwarding payload");

            let mut wrapper = PacketWrapper::new(&clientbound::login::CUSTOM_QUERY, &input);
            login_custom_query(
                &mut wrapper,
                &mut UserConnection::new(0, V1_12_2),
                &context(V1_13),
            )
            .unwrap();

            let output = wrapper.finish_with_outputs().unwrap();
            assert!(!output.cancelled, "{channel}");
            assert!(output.serverbound.is_empty(), "{channel}");
            assert_eq!(output.payload, input, "{channel}");
        }
    }

    #[test]
    fn stop_sound_becomes_the_legacy_custom_payload() {
        // Both flags set, category 1 (music), sound name.
        let mut input = vec![3, 1];
        STRING
            .write(&mut input, &"minecraft:music_disc.cat".into())
            .unwrap();
        let mut wrapper = PacketWrapper::new(&clientbound::play::STOP_SOUND, &input);
        stop_sound(
            &mut wrapper,
            &mut UserConnection::new(0, V1_12_2),
            &context(V1_13),
        )
        .unwrap();
        let result = wrapper.finish().unwrap().unwrap();
        assert_eq!(
            result.packet.to_id(V1_12_2),
            clientbound::play::CUSTOM_PAYLOAD.to_id(V1_12_2)
        );
        let mut read = result.payload.as_slice();
        assert_eq!(&*STRING.read(&mut read).unwrap(), "MC|StopSound");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "music");
        assert_eq!(
            &*STRING.read(&mut read).unwrap(),
            "minecraft:music_disc.cat"
        );
        assert!(read.is_empty());
    }
}
