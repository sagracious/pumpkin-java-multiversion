pub mod api;
pub mod data;
pub mod packet;
pub mod pipeline;
pub mod protocol;
pub mod registry;
pub mod remap;
pub mod tag;

use pumpkin_plugin_api::{
    Context, Plugin, PluginMetadata, Server,
    events::{
        EventHandler, EventPriority, packet::ProtocolPacketEvent,
        player::player_leave::PlayerLeaveEvent,
    },
    events_wit::{
        PacketDirection, PacketTranslationOutput, PlayerLeaveEventData, ProtocolPacketEventData,
    },
    register_plugin,
};

use crate::api::{bind_player, is_bound, remove_connection, remove_player};
use crate::packet::{HIGHEST_SUPPORTED, LOWEST_SUPPORTED, is_version_supported};
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_util::version::JavaMinecraftVersion;

/// The multi-version plugin allowing older Minecraft Java clients (from
/// [`LOWEST_SUPPORTED`]) to connect to a Pumpkin 26.3 server.
pub struct MultiVersionPlugin;

impl Plugin for MultiVersionPlugin {
    fn new() -> Self {
        Self
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "pumpkin-java-multiversion".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Pumpkin Developer".into()],
            description: "Multi-version Java Edition protocol translation plugin for Pumpkin."
                .into(),
            dependencies: vec![],
            permissions: vec![],
        }
    }

    fn on_load(&self, context: Context) -> Result<(), String> {
        tracing::info!("Loading Pumpkin Java Multi-Version Plugin...");

        context.register_event_handler(ProtocolPacketHandler, EventPriority::Lowest, true)?;

        context.register_event_handler(PlayerLeaveHandler, EventPriority::Lowest, true)?;

        tracing::info!(
            "Pumpkin Java Multi-Version Plugin enabled! Supporting {LOWEST_SUPPORTED} - {HIGHEST_SUPPORTED}"
        );
        Ok(())
    }

    fn on_unload(&self, _context: Context) -> Result<(), String> {
        tracing::info!("Unloading Pumpkin Java Multi-Version Plugin");
        Ok(())
    }
}

/// Translates raw Java packets across all protocol states using the connection's stable ID.
struct ProtocolPacketHandler;

impl EventHandler<ProtocolPacketEvent> for ProtocolPacketHandler {
    fn handle(
        &self,
        _server: Server,
        mut event: ProtocolPacketEventData,
    ) -> ProtocolPacketEventData {
        translate_protocol_packet(event)
    }
}

fn translate_protocol_packet(mut event: ProtocolPacketEventData) -> ProtocolPacketEventData {
    let Some(version) = event_version(event.protocol_version) else {
        return event;
    };
    if version == JavaMinecraftVersion::V_26_3 {
        return event;
    }
    let state = event.connection_state;
    let key = event.connection_id;
    if let Some(player) = event.player.as_ref()
        && !is_bound(key)
    {
        bind_player(key, version, player);
    }

    match event.direction {
        PacketDirection::Serverbound => {
            // Let login start reach Pumpkin so older unsupported clients can receive a refusal.
            if !is_version_supported(version) {
                return event;
            }
            event.translated = true;
            if state < 2 {
                return event;
            }
            match pipeline::translate_serverbound(
                key,
                version,
                state,
                event.packet_id,
                &event.raw_payload,
            ) {
                Some(translated) => {
                    event.serverbound_packets.extend(
                        translated
                            .serverbound
                            .into_iter()
                            .filter_map(|(packet, raw_payload)| {
                                (packet.v26_3 >= 0).then_some(PacketTranslationOutput {
                                    packet_id: packet.v26_3,
                                    raw_payload,
                                })
                            }),
                    );
                    event
                        .clientbound_packets
                        .extend(translated.replies.into_iter().filter_map(
                            |(packet, raw_payload)| {
                                let packet_id = packet.to_id(version);
                                (packet_id >= 0).then_some(PacketTranslationOutput {
                                    packet_id,
                                    raw_payload,
                                })
                            },
                        ));
                    if translated.cancelled {
                        event.cancelled = true;
                    } else {
                        event.packet_id = translated.packet.v26_3;
                        event.raw_payload = translated.payload;
                    }
                }
                None => event.cancelled = true,
            }
        }
        PacketDirection::Clientbound => {
            if !is_version_supported(version) {
                return refuse_unsupported(event, version, state);
            }
            // Handshake has no clientbound packets and therefore no table.
            if state == 0 {
                event.translated = true;
                return event;
            }
            #[cfg(feature = "rawdump")]
            tracing::info!(
                "RAWDUMP {} {} {} {}",
                version,
                state,
                event.packet_id,
                hex(&event.raw_payload)
            );
            let translated = pipeline::translate_clientbound(
                key,
                version,
                state,
                event.packet_id,
                &event.raw_payload,
            );
            // Status probes never become Players, so release the transient
            // translator state after each status response/pong.
            if state == 1 {
                remove_connection(key);
            }
            match translated {
                Some(translated) => {
                    if !translated.cancelled && translated.packet.to_id(version) < 0 {
                        event.cancelled = true;
                        return event;
                    }
                    event.translated = true;
                    event.serverbound_packets.extend(
                        translated
                            .serverbound
                            .into_iter()
                            .filter_map(|(packet, raw_payload)| {
                                (packet.v26_3 >= 0).then_some(PacketTranslationOutput {
                                    packet_id: packet.v26_3,
                                    raw_payload,
                                })
                            }),
                    );
                    event
                        .clientbound_packets
                        .extend(translated.extra.into_iter().filter_map(
                            |(packet, raw_payload)| {
                                let packet_id = packet.to_id(version);
                                (packet_id >= 0).then_some(PacketTranslationOutput {
                                    packet_id,
                                    raw_payload,
                                })
                            },
                        ));
                    if translated.cancelled {
                        event.cancelled = true;
                    } else {
                        event.packet_id = translated.packet.to_id(version);
                        event.raw_payload = translated.payload;
                    }
                }
                None => event.cancelled = true,
            }
        }
    }
    event
}
fn event_version(protocol_version: i32) -> Option<JavaMinecraftVersion> {
    let protocol_version = u32::try_from(protocol_version).ok()?;
    let version = JavaMinecraftVersion::from_protocol(protocol_version);
    (version != JavaMinecraftVersion::Unknown).then_some(version)
}

/// Forgets the per connection state a leaving player owned.
struct PlayerLeaveHandler;

impl EventHandler<PlayerLeaveEvent> for PlayerLeaveHandler {
    fn handle(&self, _server: Server, event: PlayerLeaveEventData) -> PlayerLeaveEventData {
        remove_player(&event.player);
        event
    }
}

/// Turns the first login packet for a client below the supported floor into a
/// disconnect with a readable reason, and drops everything else meant for it.
/// The status response is left alone since the client already shows itself as incompatible there.
fn refuse_unsupported(
    mut event: ProtocolPacketEventData,
    version: JavaMinecraftVersion,
    state: u8,
) -> ProtocolPacketEventData {
    // 0 handshake, 1 status, 2 login, 3 transfer, 4 config, 5 play.
    if state != 2 && state != 3 {
        event.cancelled = state != 1;
        if event.cancelled {
            event.clientbound_packets.clear();
        }
        return event;
    }
    let disconnect_id = packet::mappings::clientbound::login::LOGIN_DISCONNECT.to_id(version);
    if disconnect_id == -1 {
        event.cancelled = true;
        event.clientbound_packets.clear();
        return event;
    }
    let reason = serde_json::json!({
        "text": format!(
            "This server supports Minecraft {LOWEST_SUPPORTED} to {HIGHEST_SUPPORTED}. You are on {version}."
        ),
        "color": "red"
    });
    let mut payload = Vec::new();
    if payload.write_string(&reason.to_string()).is_err() {
        event.cancelled = true;
        return event;
    }
    event.packet_id = disconnect_id;
    event.raw_payload = payload;
    event.translated = true;
    event.clientbound_packets.clear();
    event
}

#[cfg(feature = "rawdump")]
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

register_plugin!(MultiVersionPlugin);

#[cfg(test)]
mod tests {
    use super::event_version;
    use crate::packet::mappings::clientbound::login::LOGIN_DISCONNECT;
    use crate::packet::{LOWEST_SUPPORTED, is_version_supported};
    use pumpkin_util::version::JavaMinecraftVersion;

    #[test]
    fn versions_below_the_floor_still_have_a_login_disconnect() {
        for version in [
            JavaMinecraftVersion::V_1_12_2,
            JavaMinecraftVersion::V_1_13_2,
            JavaMinecraftVersion::V_1_16,
            JavaMinecraftVersion::V_1_16_1,
            JavaMinecraftVersion::Unknown,
        ] {
            assert!(
                !is_version_supported(version),
                "{version} should be below the floor"
            );
            assert_eq!(
                LOGIN_DISCONNECT.to_id(version),
                0,
                "{version} has no login disconnect id to refuse it with"
            );
        }
    }

    #[test]
    fn the_floor_is_1_16_2() {
        assert_eq!(LOWEST_SUPPORTED, JavaMinecraftVersion::V_1_16_2);
        assert!(is_version_supported(JavaMinecraftVersion::V_1_16_2));
        assert!(is_version_supported(JavaMinecraftVersion::V_1_20));
        assert!(!is_version_supported(JavaMinecraftVersion::V_1_16_1));
        assert!(!is_version_supported(JavaMinecraftVersion::Unknown));
    }

    #[test]
    fn protocol_context_identifies_the_client_version() {
        assert_eq!(
            event_version(JavaMinecraftVersion::V_26_2.protocol_version()),
            Some(JavaMinecraftVersion::V_26_2)
        );
        assert_eq!(event_version(-1), None, "Bedrock sentinel is ignored");
    }
}

#[cfg(test)]
mod protocol_packet_event_tests {
    use super::translate_protocol_packet;
    use crate::api::remove_connection;
    use crate::packet::mappings::{clientbound, serverbound};
    use pumpkin_data::registry::RegistryEntryData;
    use pumpkin_plugin_api::events_wit::{PacketDirection, ProtocolPacketEventData};
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::java::client::config::CRegistryData;
    use pumpkin_util::version::JavaMinecraftVersion;

    #[test]
    fn translated_legacy_click_keeps_its_clientbound_confirmation_reply() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let connection_id = 0x504a_4d01;
        let event = ProtocolPacketEventData {
            connection_id,
            player: None,
            direction: PacketDirection::Serverbound,
            packet_id: serverbound::play::CONTAINER_CLICK.to_id(version),
            raw_payload: vec![1, 0, 36, 0, 0, 7, 0, 0],
            protocol_version: version.protocol_version(),
            connection_state: 5,
            translated: false,
            clientbound_packets: Vec::new(),
            serverbound_packets: Vec::new(),
            cancelled: false,
        };

        let translated = translate_protocol_packet(event);
        assert!(!translated.cancelled, "the recognized click remains usable");
        assert!(translated.translated);
        assert_eq!(
            translated.packet_id,
            serverbound::play::CONTAINER_CLICK.v26_3
        );
        assert_eq!(translated.clientbound_packets.len(), 1);
        assert_eq!(
            translated.clientbound_packets[0].packet_id,
            clientbound::play::WINDOW_CONFIRMATION.to_id(version)
        );
        assert_eq!(
            translated.clientbound_packets[0].raw_payload.as_slice(),
            &[1, 0, 7, 1]
        );

        remove_connection(connection_id);
    }

    fn registry_packet(registry_id: &str, names: &[&str]) -> Vec<u8> {
        let entries: Vec<_> = names
            .iter()
            .map(|name| RegistryEntryData {
                entry_id: format!("minecraft:{name}"),
                data: Some(Box::new([0x0a, 0x00])),
            })
            .collect();
        let registry_id = registry_id.to_string();
        let mut payload = Vec::new();
        CRegistryData::new(&registry_id, &entries)
            .write_packet_data(&mut payload, &JavaMinecraftVersion::V_1_20_5)
            .unwrap();
        payload
    }

    #[test]
    fn translated_registry_flush_keeps_its_following_tags_packet() {
        let version = JavaMinecraftVersion::V_1_20_3;
        let connection_id = 0x504a_4d02;
        for payload in [
            registry_packet("minecraft:dimension_type", &["overworld"]),
            registry_packet("minecraft:worldgen/biome", &["plains"]),
        ] {
            let event = ProtocolPacketEventData {
                connection_id,
                player: None,
                direction: PacketDirection::Clientbound,
                packet_id: clientbound::config::REGISTRY_DATA.v26_3,
                raw_payload: payload,
                protocol_version: version.protocol_version(),
                connection_state: 4,
                translated: false,
                clientbound_packets: Vec::new(),
                serverbound_packets: Vec::new(),
                cancelled: false,
            };
            let collected = translate_protocol_packet(event);
            assert!(
                collected.cancelled,
                "per-registry packets are held for bundling"
            );
        }

        let tags = vec![0];
        let event = ProtocolPacketEventData {
            connection_id,
            player: None,
            direction: PacketDirection::Clientbound,
            packet_id: clientbound::config::UPDATE_TAGS.v26_3,
            raw_payload: tags.clone(),
            protocol_version: version.protocol_version(),
            connection_state: 4,
            translated: false,
            clientbound_packets: Vec::new(),
            serverbound_packets: Vec::new(),
            cancelled: false,
        };
        let translated = translate_protocol_packet(event);
        assert!(!translated.cancelled);
        assert_eq!(
            translated.packet_id,
            clientbound::config::REGISTRY_DATA.to_id(version)
        );
        assert_eq!(translated.clientbound_packets.len(), 1);
        assert_eq!(
            translated.clientbound_packets[0].packet_id,
            clientbound::config::UPDATE_TAGS.to_id(version)
        );
        assert_eq!(translated.clientbound_packets[0].raw_payload, tags);

        remove_connection(connection_id);
    }
}
