use std::collections::HashMap;
use std::sync::OnceLock;

use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::protocol::packet_key;
use crate::packet::mappings::{PacketId, clientbound, serverbound};

/// Lowest version core writes a correct payload for. Packets with no entry
/// are written in 26.3 layout.
#[must_use]
pub fn core_layout_floor(packet: &'static PacketId) -> JavaMinecraftVersion {
    table()
        .get(&packet_key(packet))
        .copied()
        .unwrap_or(JavaMinecraftVersion::V_26_3)
}

/// Lowest version core's reader takes a client's own payload for. Packets with
/// no entry are read as sent. Where the answer is above the client, the chain
/// converts the payload up to it, except in the fields core still branches on
/// itself: core reads with the client's version, not with this one.
#[must_use]
pub fn core_read_floor(
    packet: &'static PacketId,
    version: JavaMinecraftVersion,
) -> JavaMinecraftVersion {
    read_table()
        .get(&packet_key(packet))
        .map_or(version, |floor| floor(version))
}

type ReadFloor = fn(JavaMinecraftVersion) -> JavaMinecraftVersion;

fn read_table() -> &'static HashMap<usize, ReadFloor> {
    static TABLE: OnceLock<HashMap<usize, ReadFloor>> = OnceLock::new();
    TABLE.get_or_init(|| {
        use JavaMinecraftVersion as V;
        let mut table: HashMap<usize, ReadFloor> = HashMap::new();

        // java/server/play/click_container.rs: OptionalItemStackHash::read takes
        // no version, so core reads hashed stacks on every version.
        table.insert(packet_key(&serverbound::play::CONTAINER_CLICK), |_| {
            V::V_1_21_5
        });
        // java/server/play/chat_message.rs: the signed branch starts at 1.19 and
        // only matches the wire from 1.19.3.
        table.insert(packet_key(&serverbound::play::CHAT), |version| {
            if version >= V::V_1_19 {
                V::V_1_19_3
            } else {
                version
            }
        });
        // java/server/play/client_information.rs: the text filtering flag is read
        // with the 1.18 meaning from 1.17 on.
        table.insert(
            packet_key(&serverbound::play::CLIENT_INFORMATION),
            |version| {
                if version >= V::V_1_17 {
                    V::V_1_18
                } else {
                    version
                }
            },
        );

        table
    })
}

fn table() -> &'static HashMap<usize, JavaMinecraftVersion> {
    static TABLE: OnceLock<HashMap<usize, JavaMinecraftVersion>> = OnceLock::new();
    TABLE.get_or_init(|| {
        use JavaMinecraftVersion as V;
        let mut table = HashMap::new();
        let mut put = |packet: &'static PacketId, floor: V| {
            table.insert(packet_key(packet), floor);
        };

        // net/java/chunk_data/mod.rs: the v1_18 writer on every version, so nothing below it.
        put(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT, V::V_1_18);
        // java/client/play/light_update.rs: branches at 1.17 for the masks and 1.16 for trust edges.
        put(&clientbound::play::LIGHT_UPDATE, V::V_1_14);
        // java/client/play/block_update.rs: no branch beyond write_block_pos, which packs 1.14 style from 1.14.
        put(&clientbound::play::BLOCK_UPDATE, V::V_1_14);
        // java/client/play/multi_block_update.rs: branches at 1.16 and 1.7.6.
        put(&clientbound::play::SECTION_BLOCKS_UPDATE, V::V_1_7_2);
        // java/client/play/worldevent.rs: no branch beyond write_block_pos.
        put(&clientbound::play::LEVEL_EVENT, V::V_1_14);
        // java/client/play/spawn_entity.rs: branches at 1.9, 1.14, 1.19 and 1.21.9.
        put(&clientbound::play::ADD_ENTITY, V::V_1_7_2);
        // java/client/play/remove_entities.rs: the 1.17 single id and the 1.7.6 form.
        put(&clientbound::play::REMOVE_ENTITIES, V::V_1_7_2);
        // java/client/play/update_tags.rs: fixed lists below 1.17, nothing below 1.13.
        put(&clientbound::play::UPDATE_TAGS, V::V_1_13);
        // java/client/config/update_tags.rs: no branch; the configuration state starts at 1.20.2.
        put(&clientbound::config::UPDATE_TAGS, V::V_1_20_2);
        // java/client/config/registry_data.rs: no branch; this is the per registry form of 1.20.5.
        put(&clientbound::config::REGISTRY_DATA, V::V_1_20_5);
        // java/client/status/status_response.rs: one json string on every version.
        put(&clientbound::status::STATUS_RESPONSE, V::V_1_7_2);
        // java/client/play/login.rs: branches from 1.8 up.
        put(&clientbound::play::LOGIN, V::V_1_7_2);
        // java/client/play/respawn.rs: branches from 1.14 up, older arm below.
        put(&clientbound::play::RESPAWN, V::V_1_7_2);
        // java/client/play/reset_score.rs: no branch; the packet starts at 1.20.3.
        put(&clientbound::play::RESET_SCORE, V::V_1_20_3);
        // java/client/play/update_score.rs: branches at 1.20.3 and 1.7.6.
        put(&clientbound::play::SET_SCORE, V::V_1_7_2);
        // java/client/play/entity_metadata.rs: the 1.8 entity id, and Metadata::write branches at 1.9.
        put(&clientbound::play::SET_ENTITY_DATA, V::V_1_7_2);
        // java/client/play/update_attributes.rs: branches at 1.7.6, 1.16, 1.17, 1.20.5 and 1.21.
        put(&clientbound::play::UPDATE_ATTRIBUTES, V::V_1_7_2);
        // java/client/play/sound_effect.rs: branches at 1.19.3, 1.19, 1.10 and 1.9.
        put(&clientbound::play::SOUND, V::V_1_7_2);
        // java/client/play/entity_sound_effect.rs: branches at 1.19.3, 1.19 and 1.10.
        put(&clientbound::play::SOUND_ENTITY, V::V_1_7_2);
        // java/client/play/explode.rs: branches at 1.21.9, 1.21.2, 1.20.5, 1.20.3, 1.19.3 and 1.17.
        put(&clientbound::play::EXPLODE, V::V_1_7_2);
        // java/client/play/particle.rs: branches at 26.3, 1.21.4, 1.20.5, 1.19, 1.15, 1.8 and 1.7.6.
        put(&clientbound::play::LEVEL_PARTICLES, V::V_1_7_2);
        // java/client/play/block_entity_data.rs: branches at 1.20.2, 1.18 and 1.8.
        put(&clientbound::play::BLOCK_ENTITY_DATA, V::V_1_7_2);
        // java/client/play/block_event.rs: no branch beyond write_block_pos.
        put(&clientbound::play::BLOCK_EVENT, V::V_1_14);
        // java/client/play/award_stats.rs: no branch; the container is the same from 1.13.
        put(&clientbound::play::AWARD_STATS, V::V_1_13);
        // java/client/play/commands.rs: the identifier form below 1.19; the packet starts at 1.13.
        put(&clientbound::play::COMMANDS, V::V_1_13);
        // java/client/play/open_screen.rs: no branch; the menu is an id from 1.14.
        put(&clientbound::play::OPEN_SCREEN, V::V_1_14);
        // java/client/play/map_item_data.rs: branches at 1.17, 1.14, 1.13 and 1.9.
        put(&clientbound::play::MAP_ITEM_DATA, V::V_1_7_2);
        // java/client/play/recipe_book_add.rs: no branch on the container.
        put(&clientbound::play::RECIPE_BOOK_ADD, V::V_26_3);
        // java/client/play/set_container_content.rs: branches at 1.17.1 and 1.21.2.
        put(&clientbound::play::CONTAINER_SET_CONTENT, V::V_1_7_2);
        // java/client/play/set_container_slot.rs: the same two branches.
        put(&clientbound::play::CONTAINER_SET_SLOT, V::V_1_7_2);
        // java/client/play/set_cursor_slot.rs: the stack alone, no branch.
        put(&clientbound::play::SET_CURSOR_ITEM, V::V_1_7_2);
        // java/client/play/set_player_inventory.rs: a slot and the stack, no branch.
        put(&clientbound::play::SET_PLAYER_INVENTORY, V::V_1_7_2);
        // java/client/play/set_equipment.rs: branches at 1.7.6, 1.9 and 1.16.
        put(&clientbound::play::SET_EQUIPMENT, V::V_1_7_2);
        // java/client/play/merchant_offers.rs: branches at 1.19 and 1.20.5.
        put(&clientbound::play::MERCHANT_OFFERS, V::V_1_7_2);
        // java/client/play/item_cooldown.rs: no branch, the cooldown group as a
        // string, which is the form 1.21.2 introduced.
        put(&clientbound::play::COOLDOWN, V::V_1_21_2);
        // java/client/play/update_advancement.rs: branches at 1.20, 1.20.2, 1.21.5, 26.1 and 26.3.
        put(&clientbound::play::UPDATE_ADVANCEMENTS, V::V_1_7_2);
        // net/java/play/tag_query.rs: the root is always written unnamed, which is the 1.20.2 form.
        put(&clientbound::play::TAG_QUERY, V::V_1_20_2);
        // java/client/play/player_info_update.rs: masks the list priority below 1.21.2 and the hat below 1.21.4.
        put(&clientbound::play::PLAYER_INFO_UPDATE, V::V_1_19_3);
        // java/client/play/player_remove.rs: no branch; the packet starts at 1.19.3.
        put(&clientbound::play::PLAYER_INFO_REMOVE, V::V_1_19_3);
        // java/client/play/player_chat_message.rs: no branch beyond the component form, and the global index it always writes starts at 1.21.5.
        put(&clientbound::play::PLAYER_CHAT, V::V_1_21_5);
        // java/client/play/system_chat_message.rs: branches at 1.19.1, 1.19, 1.16 and 1.8.
        put(&clientbound::play::SYSTEM_CHAT, V::V_1_7_2);
        // java/client/play/disguised_chat_message.rs: no branch; the chat type goes out in the holder form 1.21 reads.
        put(&clientbound::play::DISGUISED_CHAT, V::V_1_21);
        // java/client/play/delete_chat.rs: no branch; the packed signature id starts at 1.19.3.
        put(&clientbound::play::DELETE_CHAT, V::V_1_19_3);
        // java/client/play/server_data.rs: branches at 1.19.4, 1.19.3, 1.19.1 and 1.20.5.
        put(&clientbound::play::SERVER_DATA, V::V_1_19);
        // java/client/play/combat_event.rs: no branch; the opponent id 1.17 to 1.19.4 carry is gone from 1.20.
        put(&clientbound::play::PLAYER_COMBAT_END, V::V_1_20);
        // java/client/play/combat_death.rs: no branch beyond the component form; the killer id likewise stops at 1.19.4.
        put(&clientbound::play::PLAYER_COMBAT_KILL, V::V_1_20);
        // java/client/play/combat_event.rs: an empty payload on every version that has the packet.
        put(&clientbound::play::PLAYER_COMBAT_ENTER, V::V_1_17);
        // java/client/play/initialize_world_border.rs: no branch; the varlong lerp time is the varint form for every value core sends.
        put(&clientbound::play::INITIALIZE_BORDER, V::V_1_17);
        // java/client/play/set_border_lerp_size.rs: the same varlong.
        put(&clientbound::play::SET_BORDER_LERP_SIZE, V::V_1_17);
        // java/client/play/set_border_size.rs: no branch; the packet starts at 1.17.
        put(&clientbound::play::SET_BORDER_SIZE, V::V_1_17);
        // java/client/play/set_border_center.rs: likewise.
        put(&clientbound::play::SET_BORDER_CENTER, V::V_1_17);
        // java/client/play/set_border_warning_delay.rs: likewise.
        put(&clientbound::play::SET_BORDER_WARNING_DELAY, V::V_1_17);
        // java/client/play/set_border_warning_distance.rs: likewise.
        put(&clientbound::play::SET_BORDER_WARNING_DISTANCE, V::V_1_17);
        // java/client/play/set_title.rs: no branch beyond the component form; the packet starts at 1.17.
        put(&clientbound::play::SET_TITLE_TEXT, V::V_1_17);
        // java/client/play/subtitle.rs: likewise.
        put(&clientbound::play::SET_SUBTITLE_TEXT, V::V_1_17);
        // java/client/play/actionbar.rs: likewise.
        put(&clientbound::play::SET_ACTION_BAR_TEXT, V::V_1_17);
        // java/client/play/set_title_animation.rs: three ints, no branch.
        put(&clientbound::play::SET_TITLES_ANIMATION, V::V_1_17);
        // java/client/play/clear_title.rs: one bool, no branch.
        put(&clientbound::play::CLEAR_TITLES, V::V_1_17);

        table
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::pipeline::translate_clientbound;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::java::client::play::CServerData;
    use pumpkin_util::text::TextComponent;

    /// minecraft-data `packet_server_data` is an optional motd, an optional
    /// icon and the chat preview flag on 1.19; 1.19.1 adds the secure chat
    /// flag, 1.19.3 drops the preview one and 1.20.5 the secure one. Core
    /// writes every one of them, so the packet passes through.
    #[test]
    fn server_data_reaches_every_version_as_core_wrote_it() {
        let motd = TextComponent::text("m");
        for (version, trailing) in [
            (JavaMinecraftVersion::V_1_19, 1usize),
            (JavaMinecraftVersion::V_1_19_1, 2),
            (JavaMinecraftVersion::V_1_19_3, 1),
            (JavaMinecraftVersion::V_1_19_4, 0),
            (JavaMinecraftVersion::V_1_20_5, 0),
        ] {
            let mut payload = Vec::new();
            CServerData::new(&motd, None)
                .write_packet_data(&mut payload, &version)
                .unwrap();
            let out = translate_clientbound(
                95,
                version,
                5,
                clientbound::play::SERVER_DATA.v26_3,
                &payload,
            )
            .unwrap();
            assert_eq!(out.payload, payload, "{version}");
            // The icon flag and whatever flags follow it are all false.
            assert_eq!(
                &payload[payload.len() - trailing - 1..],
                &vec![0u8; trailing + 1][..],
                "{version}"
            );
            remove_connection(95);
        }
    }

    use crate::pipeline::translate_serverbound;
    use pumpkin_protocol::ServerPacket;
    use pumpkin_protocol::java::server::login::SLoginStart;
    use pumpkin_protocol::java::server::play::SSetCreativeSlot;

    const LOGIN: u8 = 2;
    const PLAY: u8 = 5;

    #[test]
    fn a_packet_with_no_entry_is_native() {
        assert_eq!(
            core_layout_floor(&clientbound::play::SET_TIME),
            JavaMinecraftVersion::V_26_3
        );
        assert_eq!(
            core_layout_floor(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT),
            JavaMinecraftVersion::V_1_18
        );
    }

    #[test]
    fn a_serverbound_packet_with_no_entry_is_read_as_sent() {
        for version in [
            JavaMinecraftVersion::V_1_16_2,
            JavaMinecraftVersion::V_1_19,
            JavaMinecraftVersion::V_1_20_2,
        ] {
            assert_eq!(
                core_read_floor(&serverbound::login::HELLO, version),
                version
            );
        }
    }

    /// minecraft-data `packet_login_start`: the name alone up to 1.18.2, plus
    /// an optional profile key on 1.19, plus an optional uuid on 1.19.1 and
    /// 1.19.2, name and optional uuid from 1.19.3, name and uuid from 1.20.2.
    #[test]
    fn every_login_start_a_client_sends_reaches_core_unchanged() {
        for version in [
            JavaMinecraftVersion::V_1_16_2,
            JavaMinecraftVersion::V_1_17_1,
            JavaMinecraftVersion::V_1_18_2,
            JavaMinecraftVersion::V_1_19,
            JavaMinecraftVersion::V_1_19_1,
            JavaMinecraftVersion::V_1_19_3,
            JavaMinecraftVersion::V_1_20,
            JavaMinecraftVersion::V_1_20_2,
        ] {
            let mut payload = Vec::new();
            payload.push(5);
            payload.extend_from_slice(b"Notch");
            if version >= JavaMinecraftVersion::V_1_19 && version < JavaMinecraftVersion::V_1_19_3 {
                payload.push(0);
            }
            if version >= JavaMinecraftVersion::V_1_20_2 {
                payload.extend_from_slice(&[0u8; 16]);
            } else if version >= JavaMinecraftVersion::V_1_19_1 {
                payload.push(1);
                payload.extend_from_slice(&[0u8; 16]);
            }

            let wire = serverbound::login::HELLO.to_id(version);
            let out = translate_serverbound(0, version, LOGIN, wire, &payload).expect("{version}");
            assert_eq!(out.payload, payload, "{version}");
            let mut read: &[u8] = &out.payload;
            let packet = SLoginStart::read(&mut read, &version).expect("read");
            assert_eq!(&*packet.name, "Notch", "{version}");
            assert!(read.is_empty(), "{version}");
        }
    }

    /// The four packets core writes all the way down keep their floor: what it
    /// wrote for the oldest supported client is what the id pass reads back.
    #[test]
    fn the_effect_packets_core_writes_itself_are_read_in_the_clients_layout() {
        use crate::pipeline::translate_clientbound;
        use pumpkin_data::particle::Particle;
        use pumpkin_data::sound::SoundCategory;
        use pumpkin_protocol::ClientPacket;
        use pumpkin_protocol::IdOr;
        use pumpkin_protocol::codec::var_int::VarInt;
        use pumpkin_protocol::java::client::play::{
            CEntitySoundEffect, CExplosion, CParticle, CSoundEffect,
        };
        use pumpkin_util::math::vector3::Vector3;

        let version = JavaMinecraftVersion::V_1_16_2;
        let mut payloads: Vec<(&'static PacketId, Vec<u8>)> = Vec::new();

        let mut bytes = Vec::new();
        CExplosion::new(
            Vector3::new(0.0, 0.0, 0.0),
            4.0,
            0,
            None,
            VarInt(Particle::Explosion as i32),
            IdOr::Id(0),
        )
        .write_packet_data(&mut bytes, &version)
        .unwrap();
        payloads.push((&clientbound::play::EXPLODE, bytes));

        let mut bytes = Vec::new();
        CParticle::new(
            false,
            false,
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.1, 0.2, 0.3),
            0.5,
            10,
            VarInt(Particle::Smoke as i32),
            &[],
        )
        .write_packet_data(&mut bytes, &version)
        .unwrap();
        payloads.push((&clientbound::play::LEVEL_PARTICLES, bytes));

        let mut bytes = Vec::new();
        CSoundEffect::new(
            IdOr::Id(3),
            SoundCategory::Players,
            &Vector3::new(1.0, 2.0, 3.0),
            1.0,
            0.5,
            42,
        )
        .write_packet_data(&mut bytes, &version)
        .unwrap();
        payloads.push((&clientbound::play::SOUND, bytes));

        let mut bytes = Vec::new();
        CEntitySoundEffect::new(
            IdOr::Id(3),
            SoundCategory::Players,
            VarInt(123),
            1.0,
            0.5,
            42,
        )
        .write_packet_data(&mut bytes, &version)
        .unwrap();
        payloads.push((&clientbound::play::SOUND_ENTITY, bytes));

        for (packet, payload) in payloads {
            assert_eq!(core_layout_floor(packet), JavaMinecraftVersion::V_1_7_2);
            assert!(
                translate_clientbound(0, version, PLAY, packet.v26_3, &payload).is_some(),
                "{}",
                packet.v26_3
            );
        }
    }

    /// minecraft-data `packet_set_creative_slot`: a two byte slot on every
    /// version, so only the stack itself changes shape.
    #[test]
    fn a_creative_slot_reaches_cores_reader_on_every_version() {
        use crate::api::MappingData;
        use crate::api::rewriter::item::StructuredItemRewriter;
        use crate::api::types::{Item, ItemT, WireType};

        for version in [
            JavaMinecraftVersion::V_1_16_2,
            JavaMinecraftVersion::V_1_20_3,
            JavaMinecraftVersion::V_1_20_5,
            JavaMinecraftVersion::V_1_21_4,
            JavaMinecraftVersion::V_1_21_5,
        ] {
            let native = Item::Structured {
                count: 1,
                id: i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id),
                added: Vec::new(),
                removed: Vec::new(),
            };
            let ids = MappingData::get().composed(version);
            let item = StructuredItemRewriter::to_version(&native, version, ids);
            let mut payload = vec![0, 36];
            if version >= JavaMinecraftVersion::V_1_21_5 {
                ItemT::length_prefixed(version)
                    .write(&mut payload, &item)
                    .unwrap();
            } else {
                ItemT::for_version(version)
                    .write(&mut payload, &item)
                    .unwrap();
            }

            let wire = serverbound::play::SET_CREATIVE_MODE_SLOT.to_id(version);
            let out = translate_serverbound(0, version, PLAY, wire, &payload).expect("{version}");
            let mut read: &[u8] = &out.payload;
            let packet = SSetCreativeSlot::read(&mut read, &version).expect("read");
            assert_eq!(packet.slot, 36, "{version}");
            assert!(read.is_empty(), "{version}");
        }
    }
}
