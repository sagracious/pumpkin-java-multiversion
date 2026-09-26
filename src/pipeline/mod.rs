pub mod core_layout;
pub mod id_pass;
pub mod item_pass;
mod tables;

use std::collections::HashSet;
use std::sync::OnceLock;

use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::protocol::packet_key;
#[cfg(test)]
use crate::api::remove_connection;
use crate::api::{
    Ctx, MappingData, PacketWrapper, Registry, TranslateError, Translated, with_connection,
};
use crate::packet::mappings::PacketId;
use crate::protocol::STEPS;
use core_layout::core_layout_floor;

struct Chain {
    registries: Vec<Registry>,
    clientbound: HashSet<usize>,
    serverbound: HashSet<usize>,
}

fn chain() -> &'static Chain {
    static CHAIN: OnceLock<Chain> = OnceLock::new();
    CHAIN.get_or_init(|| {
        let mut registries = Vec::with_capacity(STEPS.len());
        let mut clientbound = HashSet::new();
        let mut serverbound = HashSet::new();
        for protocol in STEPS {
            let mut registry = Registry::default();
            protocol.register(&mut registry);
            clientbound.extend(registry.clientbound_keys());
            serverbound.extend(registry.serverbound_keys());
            registries.push(registry);
        }
        Chain {
            registries,
            clientbound,
            serverbound,
        }
    })
}

/// Steps whose lower endpoint is at or above the client. `STEPS` is ordered by
/// descending `to`, so this is a prefix.
fn steps_for(version: JavaMinecraftVersion) -> usize {
    STEPS
        .iter()
        .take_while(|protocol| protocol.step().to >= version)
        .count()
}

fn dropped(packet: &PacketId, version: JavaMinecraftVersion, stage: &str, error: &TranslateError) {
    tracing::debug!(
        "{version}: 26.3 packet {} dropped at {stage}: {error}",
        packet.v26_3
    );
}

pub fn translate_clientbound(
    key: u64,
    version: JavaMinecraftVersion,
    state: u8,
    packet_id: i32,
    payload: &[u8],
) -> Option<Translated> {
    let packet = tables::resolve_clientbound(state, packet_id, version)?;
    let layout = core_layout_floor(packet).max(version);
    let steps = &STEPS[..steps_for(version)];
    let chain = chain();

    with_connection(key, version, |connection| {
        let mut wrapper = PacketWrapper::new(packet, payload);

        if let Some(pass) = id_pass::id_pass_for(packet) {
            pass(
                &mut wrapper,
                connection,
                layout,
                MappingData::get().composed(layout),
            )
            .map_err(|error| dropped(packet, version, "id pass", &error))
            .ok()?;
        } else {
            wrapper.passthrough_all();
        }

        if chain.clientbound.contains(&packet_key(packet)) {
            for (index, protocol) in steps.iter().enumerate() {
                if wrapper.is_cancelled() {
                    break;
                }
                let step = protocol.step();
                let Some(entry) = chain.registries[index].clientbound_handler(wrapper.packet())
                else {
                    continue;
                };
                if !entry.runs(step.from, layout) {
                    continue;
                }
                wrapper.reset();
                let ctx = Ctx {
                    step,
                    mappings: MappingData::get().step(step.from),
                    layout,
                };
                (entry.handler)(&mut wrapper, connection, &ctx)
                    .map_err(|error| dropped(packet, version, "step", &error))
                    .ok()?;
            }
        }

        let translated = wrapper
            .finish_with_outputs()
            .map_err(|error| dropped(packet, version, "finish", &error))
            .ok()?;
        if translated.cancelled
            && translated.extra.is_empty()
            && translated.serverbound.is_empty()
        {
            return None;
        }
        if !translated.cancelled && translated.packet.to_id(version) == -1 {
            return None;
        }
        Some(translated)
    })
}

pub fn translate_serverbound(
    key: u64,
    version: JavaMinecraftVersion,
    state: u8,
    packet_id: i32,
    payload: &[u8],
) -> Option<Translated> {
    let packet = tables::resolve_serverbound(state, packet_id, version)?;
    let layout = core_layout::core_read_floor(packet, version).max(version);
    let steps = &STEPS[..steps_for(version)];
    let chain = chain();

    with_connection(key, version, |connection| {
        let mut wrapper = PacketWrapper::new(packet, payload);

        if chain.serverbound.contains(&packet_key(packet)) {
            for (index, protocol) in steps.iter().enumerate().rev() {
                if wrapper.is_cancelled() {
                    break;
                }
                let step = protocol.step();
                let Some(entry) = chain.registries[index].serverbound_handler(wrapper.packet())
                else {
                    continue;
                };
                if !entry.runs(step.from, layout) {
                    continue;
                }
                wrapper.reset();
                let ctx = Ctx {
                    step,
                    mappings: MappingData::get().step(step.from),
                    layout,
                };
                (entry.handler)(&mut wrapper, connection, &ctx)
                    .map_err(|error| dropped(packet, version, "step", &error))
                    .ok()?;
            }
        }

        if let Some(pass) = id_pass::id_pass_for(wrapper.packet()) {
            wrapper.reset();
            pass(
                &mut wrapper,
                connection,
                layout,
                MappingData::get().composed(layout),
            )
            .map_err(|error| dropped(packet, version, "id pass", &error))
            .ok()?;
        } else {
            wrapper.passthrough_all();
        }

        let translated = wrapper
            .finish_with_outputs()
            .map_err(|error| dropped(packet, version, "finish", &error))
            .ok()?;
        if translated.cancelled
            && translated.replies.is_empty()
            && translated.serverbound.is_empty()
        {
            return None;
        }
        if !translated.cancelled && translated.packet.v26_3 == -1 {
            return None;
        }
        Some(translated)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{I64, WireType};
    use crate::packet::mappings::{clientbound, serverbound};

    const PLAY: u8 = 5;

    #[test]
    fn steps_run_from_26_3_down_to_the_client() {
        assert_eq!(steps_for(JavaMinecraftVersion::V_26_3), 0);
        assert_eq!(steps_for(JavaMinecraftVersion::V_26_2), 1);
        assert_eq!(
            steps_for(JavaMinecraftVersion::V_1_16_2),
            STEPS
                .iter()
                .position(|protocol| {
                    protocol.step().to == JavaMinecraftVersion::V_1_16_2
                })
                .unwrap()
                + 1
        );
        let last = STEPS[steps_for(JavaMinecraftVersion::V_1_18_2) - 1].step();
        assert_eq!(last.from, JavaMinecraftVersion::V_1_19);
        assert_eq!(last.to, JavaMinecraftVersion::V_1_18_2);
    }

    #[test]
    fn registered_steps_and_version_endpoints_form_one_contiguous_chain() {
        assert_eq!(crate::protocol::VERSIONS.len(), STEPS.len() + 1);
        for (index, protocol) in STEPS.iter().enumerate() {
            let step = protocol.step();
            assert_eq!(crate::protocol::VERSIONS[index], step.from);
            assert_eq!(crate::protocol::VERSIONS[index + 1], step.to);
            if let Some(next) = STEPS.get(index + 1) {
                assert_eq!(step.to, next.step().from);
            }
        }
    }

    /// A layout handler at the 1.20.5 boundary is owed only where core did not
    /// write for the client itself: configuration `UPDATE_TAGS` has floor
    /// 1.20.2, while `SET_TIME` retains its native payload layout.
    #[test]
    fn a_layout_handler_skips_what_core_already_wrote() {
        fn nothing(
            _wrapper: &mut PacketWrapper,
            _connection: &mut crate::api::UserConnection,
            _ctx: &Ctx,
        ) -> Result<(), crate::api::TranslateError> {
            Ok(())
        }

        let from = JavaMinecraftVersion::V_1_20_5;
        let client = JavaMinecraftVersion::V_1_20_2;
        let branched = core_layout_floor(&clientbound::config::UPDATE_TAGS).max(client);
        let native = core_layout_floor(&clientbound::play::SET_TIME).max(client);
        assert_eq!(branched, JavaMinecraftVersion::V_1_20_2);
        assert_eq!(native, JavaMinecraftVersion::V_26_3);

        let mut registry = Registry::default();
        registry.clientbound_layout(&clientbound::config::UPDATE_TAGS, nothing);
        registry.clientbound_layout(&clientbound::play::SET_TIME, nothing);
        assert!(
            !registry
                .clientbound_handler(&clientbound::config::UPDATE_TAGS)
                .unwrap()
                .runs(from, branched)
        );
        assert!(
            registry
                .clientbound_handler(&clientbound::play::SET_TIME)
                .unwrap()
                .runs(from, native)
        );

        let mut registry = Registry::default();
        registry.clientbound(&clientbound::config::UPDATE_TAGS, nothing);
        assert!(
            registry
                .clientbound_handler(&clientbound::config::UPDATE_TAGS)
                .unwrap()
                .runs(from, branched)
        );
    }

    #[test]
    fn set_time_keeps_its_payload_and_gets_the_client_id() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let mut payload = Vec::new();
        I64.write(&mut payload, &7i64).unwrap();
        I64.write(&mut payload, &3i64).unwrap();
        let out = translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::SET_TIME.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(out.payload, payload);
        assert_eq!(
            out.packet.to_id(version),
            clientbound::play::SET_TIME.to_id(version)
        );
    }

    #[test]
    fn a_packet_the_client_does_not_have_is_dropped() {
        assert!(
            translate_clientbound(
                0,
                JavaMinecraftVersion::V_1_16_2,
                PLAY,
                clientbound::play::WAYPOINT.v26_3,
                &[],
            )
            .is_none()
        );
    }

    #[test]
    fn serverbound_only_renumbers() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let payload = [7u8, 7];
        let wire = serverbound::play::KEEP_ALIVE.to_id(version);
        let out = translate_serverbound(0, version, PLAY, wire, &payload).unwrap();
        assert_eq!(out.payload, payload);
        assert_eq!(out.packet.v26_3, serverbound::play::KEEP_ALIVE.v26_3);
    }

    #[test]
    fn a_step_retargets_reset_score_onto_set_score() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let out = translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::RESET_SCORE.v26_3,
            b"\x03bob\x00",
        )
        .unwrap();
        assert_eq!(out.payload, b"\x03bob\x01\x00");
        assert_eq!(
            out.packet.to_id(version),
            clientbound::play::SET_SCORE.to_id(version)
        );
    }

    #[test]
    fn reset_score_reaches_the_client_untouched_from_1_20_3_up() {
        let version = JavaMinecraftVersion::V_1_20_3;
        let out = translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::RESET_SCORE.v26_3,
            b"\x03bob\x00",
        )
        .unwrap();
        assert_eq!(out.payload, b"\x03bob\x00");
        assert_eq!(
            out.packet.to_id(version),
            clientbound::play::RESET_SCORE.to_id(version)
        );
    }

    #[test]
    fn recipe_display_packet_is_kept_only_where_the_client_has_that_format() {
        for version in [JavaMinecraftVersion::V_26_2, JavaMinecraftVersion::V_1_21_2] {
            assert!(
                translate_clientbound(
                    0,
                    version,
                    PLAY,
                    clientbound::play::RECIPE_BOOK_ADD.v26_3,
                    &[0],
                )
                .is_some(),
                "{version}"
            );
        }
        assert!(
            translate_clientbound(
                0,
                JavaMinecraftVersion::V_1_16_2,
                PLAY,
                clientbound::play::RECIPE_BOOK_ADD.v26_3,
                &[0],
            )
            .is_none()
        );
    }

    #[test]
    fn swing_arrives_as_punch() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_20_2,
            JavaMinecraftVersion::V_1_16_2,
        ] {
            let wire = serverbound::play::SWING.to_id(version);
            let out = translate_serverbound(0, version, PLAY, wire, &[0]).unwrap();
            assert_eq!(out.payload, [0]);
            assert_eq!(
                out.packet.v26_3,
                serverbound::play::PUNCH.v26_3,
                "{version}"
            );
        }
    }

    /// The tracker is what tells the 1.19 step the entity is a painting: the id
    /// pass has already renumbered the type field by then.
    #[test]
    fn a_painting_becomes_its_own_packet_on_1_18_2() {
        use pumpkin_data::entity::EntityType;
        use pumpkin_protocol::codec::var_int::VarInt;
        use pumpkin_protocol::{ClientPacket, java::client::play::CSpawnEntity};
        use pumpkin_util::math::vector3::Vector3;

        let version = JavaMinecraftVersion::V_1_18_2;
        let spawn = CSpawnEntity::new(
            VarInt(5),
            uuid::Uuid::from_u128(2),
            VarInt(i32::from(EntityType::PAINTING.id)),
            Vector3::new(4.0, 5.0, 6.0),
            0.0,
            0.0,
            0.0,
            // north, the 3D direction index core writes
            VarInt(2),
            Vector3::new(0.0, 0.0, 0.0),
        );
        let mut payload = Vec::new();
        spawn.write_packet_data(&mut payload, &version).unwrap();

        let out = translate_clientbound(
            1,
            version,
            PLAY,
            clientbound::play::ADD_ENTITY.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(
            out.packet.to_id(version),
            clientbound::play::SPAWN_PAINTING.to_id(version)
        );
        remove_connection(1);
    }
}
