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
        ClientboundPacket, EventHandler, EventPriority, JavaClientboundPacket,
        JavaServerboundPacket, ServerboundPacket,
        packet::{PacketReceivedEvent, PacketSentEvent},
        player::player_leave::PlayerLeaveEvent,
    },
    events_wit::{PacketReceivedEventData, PacketSentEventData, PlayerLeaveEventData},
    register_plugin,
};

use crate::api::connection::player_key;
use crate::api::{bind_player, is_bound, remove_player};
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

        context.register_event_handler(PacketReceivedHandler, EventPriority::Highest, true)?;

        context.register_event_handler(PacketSentHandler, EventPriority::Lowest, true)?;

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

/// Handles incoming packets from clients and translates them if the client is on an older version.
struct PacketReceivedHandler;

impl EventHandler<PacketReceivedEvent> for PacketReceivedHandler {
    fn handle(
        &self,
        _server: Server,
        mut event: PacketReceivedEventData,
    ) -> PacketReceivedEventData {
        let Some(version) = event_version(&event.player) else {
            // The current Pumpkin event API also emits packet events for
            // Bedrock clients, which this Java translator does not handle.
            return event;
        };
        if version == JavaMinecraftVersion::V_26_3 {
            return event;
        }
        // An unsupported client is refused on its first clientbound login
        // packet (see `refuse_unsupported`); its login start has to reach the
        // server untouched for that packet to be sent at all.
        if !is_version_supported(version) {
            return event;
        }
        let Some(state) = serverbound_state(&event.packet) else {
            event.cancelled = true;
            return event;
        };
        // Handshake and status ids never changed.
        if state < 2 {
            return event;
        }
        let key = player_key(&event.player);
        match pipeline::translate_serverbound(
            key,
            version,
            state,
            event.packet_id,
            &event.raw_payload,
        ) {
            Some(translated) => {
                if !translated.replies.is_empty() {
                    tracing::warn!(
                        version = %version,
                        packet_id = event.packet_id,
                        count = translated.replies.len(),
                        "Canceling translated packet because the current Pumpkin WIT bridge cannot send companion replies"
                    );
                    event.cancelled = true;
                    return event;
                }
                event.packet_id = translated.packet.v26_3;
                event.raw_payload = translated.payload;
            }
            // No 26.3 equivalent. Forwarding it unchanged makes the server
            // read the id as whatever packet now occupies that slot and
            // desync the stream, so drop it instead.
            None => event.cancelled = true,
        }
        event
    }
}

/// Handles outgoing packets to clients and translates them to match the client's expected version.
struct PacketSentHandler;

impl EventHandler<PacketSentEvent> for PacketSentHandler {
    fn handle(&self, _server: Server, mut event: PacketSentEventData) -> PacketSentEventData {
        let Some(version) = event_version(&event.player) else {
            return event;
        };
        if version == JavaMinecraftVersion::V_26_3 {
            return event;
        }
        let Some(state) = clientbound_state(&event.packet) else {
            event.cancelled = true;
            return event;
        };
        if !is_version_supported(version) {
            return refuse_unsupported(event, version, state);
        }
        // Handshake has no clientbound packets and therefore no table.
        if state == 0 {
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
        let key = player_key(&event.player);
        if !is_bound(key) {
            bind_player(key, version, &event.player);
        }
        match pipeline::translate_clientbound(
            key,
            version,
            state,
            event.packet_id,
            &event.raw_payload,
        ) {
            Some(translated) => {
                if !translated.extra.is_empty() {
                    tracing::warn!(
                        version = %version,
                        packet_id = event.packet_id,
                        count = translated.extra.len(),
                        "Canceling translated packet because the current Pumpkin WIT bridge cannot send companion packets"
                    );
                    event.cancelled = true;
                    return event;
                }
                event.packet_id = translated.packet.to_id(version);
                event.raw_payload = translated.payload;
            }
            // No id for this version: the packet does not exist on the client.
            // Sending it under a 26.3 id would desync the stream, so drop it.
            None => event.cancelled = true,
        }
        event
    }
}

fn event_version(player: &pumpkin_plugin_api::Player) -> Option<JavaMinecraftVersion> {
    let version = from_wasm_java_version(player.as_java()?.get_version());
    (version != JavaMinecraftVersion::Unknown).then_some(version)
}

fn serverbound_state(packet: &ServerboundPacket) -> Option<u8> {
    let ServerboundPacket::Java(packet) = packet else {
        return None;
    };
    use JavaServerboundPacket as P;
    match packet {
        P::ConfigSAcceptCodeOfConduct
        | P::ConfigSAcknowledgeFinishConfig
        | P::ConfigSClientInformationConfig(_)
        | P::ConfigSConfigCookieResponse(_)
        | P::ConfigSCustomClickAction(_)
        | P::ConfigSKeepAlive(_)
        | P::ConfigSKnownPacks(_)
        | P::ConfigSPluginMessage(_)
        | P::ConfigSConfigPong(_)
        | P::ConfigSConfigResourcePack(_) => Some(4),
        P::LoginSLoginCookieResponse(_)
        | P::LoginSLoginAcknowledged
        | P::LoginSLoginStart(_)
        | P::LoginSLoginPluginResponse(_) => Some(2),
        P::StatusSStatusPingRequest(_) | P::StatusSStatusRequest => Some(1),
        P::Unknown => None,
        _ => Some(5),
    }
}

fn clientbound_state(packet: &ClientboundPacket) -> Option<u8> {
    let ClientboundPacket::Java(packet) = packet else {
        return None;
    };
    use JavaClientboundPacket as P;
    match packet {
        P::ConfigCConfigAddResourcePack(_)
        | P::ConfigCConfigClearDialog
        | P::ConfigCCodeOfConduct(_)
        | P::ConfigCConfigDisconnect(_)
        | P::ConfigCCookieRequest(_)
        | P::ConfigCConfigCustomReportDetails(_)
        | P::ConfigCFeatureFlags(_)
        | P::ConfigCFinishConfig
        | P::ConfigCKnownPacks(_)
        | P::ConfigCConfigPing(_)
        | P::ConfigCPluginMessage(_)
        | P::ConfigCConfigPostEffects(_)
        | P::ConfigCRegistryData(_)
        | P::ConfigCConfigRemoveResourcePack(_)
        | P::ConfigCConfigResetChat
        | P::ConfigCConfigServerLinks(_)
        | P::ConfigCConfigShowDialog(_)
        | P::ConfigCStoreCookie(_)
        | P::ConfigCTransfer(_)
        | P::ConfigCUpdateTags(_) => Some(4),
        P::LoginCLoginCookieRequest(_)
        | P::LoginCLoginDisconnect(_)
        | P::LoginCLoginPluginRequest(_)
        | P::LoginCSetCompression(_) => Some(2),
        P::StatusCPingResponse(_) | P::StatusCStatusResponse(_) => Some(1),
        P::Unknown => None,
        _ => Some(5),
    }
}

const fn from_wasm_java_version(
    version: pumpkin_plugin_api::wit::pumpkin::plugin::player::JavaMinecraftVersion,
) -> JavaMinecraftVersion {
    use pumpkin_plugin_api::wit::pumpkin::plugin::player::JavaMinecraftVersion as W;
    match version {
        W::V172 => JavaMinecraftVersion::V_1_7_2,
        W::V176 => JavaMinecraftVersion::V_1_7_6,
        W::V18 => JavaMinecraftVersion::V_1_8,
        W::V19 => JavaMinecraftVersion::V_1_9,
        W::V191 => JavaMinecraftVersion::V_1_9_1,
        W::V192 => JavaMinecraftVersion::V_1_9_2,
        W::V193 => JavaMinecraftVersion::V_1_9_3,
        W::V110 => JavaMinecraftVersion::V_1_10,
        W::V111 => JavaMinecraftVersion::V_1_11,
        W::V1111 => JavaMinecraftVersion::V_1_11_1,
        W::V112 => JavaMinecraftVersion::V_1_12,
        W::V1121 => JavaMinecraftVersion::V_1_12_1,
        W::V1122 => JavaMinecraftVersion::V_1_12_2,
        W::V113 => JavaMinecraftVersion::V_1_13,
        W::V1131 => JavaMinecraftVersion::V_1_13_1,
        W::V1132 => JavaMinecraftVersion::V_1_13_2,
        W::V114 => JavaMinecraftVersion::V_1_14,
        W::V1141 => JavaMinecraftVersion::V_1_14_1,
        W::V1142 => JavaMinecraftVersion::V_1_14_2,
        W::V1143 => JavaMinecraftVersion::V_1_14_3,
        W::V1144 => JavaMinecraftVersion::V_1_14_4,
        W::V115 => JavaMinecraftVersion::V_1_15,
        W::V1151 => JavaMinecraftVersion::V_1_15_1,
        W::V1152 => JavaMinecraftVersion::V_1_15_2,
        W::V116 => JavaMinecraftVersion::V_1_16,
        W::V1161 => JavaMinecraftVersion::V_1_16_1,
        W::V1162 => JavaMinecraftVersion::V_1_16_2,
        W::V1163 => JavaMinecraftVersion::V_1_16_3,
        W::V1164 => JavaMinecraftVersion::V_1_16_4,
        W::V117 => JavaMinecraftVersion::V_1_17,
        W::V1171 => JavaMinecraftVersion::V_1_17_1,
        W::V118 => JavaMinecraftVersion::V_1_18,
        W::V1182 => JavaMinecraftVersion::V_1_18_2,
        W::V119 => JavaMinecraftVersion::V_1_19,
        W::V1191 => JavaMinecraftVersion::V_1_19_1,
        W::V1193 => JavaMinecraftVersion::V_1_19_3,
        W::V1194 => JavaMinecraftVersion::V_1_19_4,
        W::V120 => JavaMinecraftVersion::V_1_20,
        W::V1202 => JavaMinecraftVersion::V_1_20_2,
        W::V1203 => JavaMinecraftVersion::V_1_20_3,
        W::V1205 => JavaMinecraftVersion::V_1_20_5,
        W::V121 => JavaMinecraftVersion::V_1_21,
        W::V1212 => JavaMinecraftVersion::V_1_21_2,
        W::V1214 => JavaMinecraftVersion::V_1_21_4,
        W::V1215 => JavaMinecraftVersion::V_1_21_5,
        W::V1216 => JavaMinecraftVersion::V_1_21_6,
        W::V1217 => JavaMinecraftVersion::V_1_21_7,
        W::V1219 => JavaMinecraftVersion::V_1_21_9,
        W::V12111 => JavaMinecraftVersion::V_1_21_11,
        W::V261 => JavaMinecraftVersion::V_26_1,
        W::V262 => JavaMinecraftVersion::V_26_2,
        W::V263 => JavaMinecraftVersion::V_26_3,
        W::Unknown => JavaMinecraftVersion::Unknown,
    }
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
    mut event: PacketSentEventData,
    version: JavaMinecraftVersion,
    state: u8,
) -> PacketSentEventData {
    // 0 handshake, 1 status, 2 login, 3 transfer, 4 config, 5 play.
    if state != 2 && state != 3 {
        event.cancelled = state != 1;
        return event;
    }
    let disconnect_id = packet::mappings::clientbound::login::LOGIN_DISCONNECT.to_id(version);
    if disconnect_id == -1 {
        event.cancelled = true;
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
    use super::{clientbound_state, serverbound_state};
    use crate::packet::mappings::clientbound::login::LOGIN_DISCONNECT;
    use crate::packet::{LOWEST_SUPPORTED, is_version_supported};
    use pumpkin_plugin_api::events::{
        ClientboundPacket, JavaClientboundPacket, JavaServerboundPacket, ServerboundPacket,
    };
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
    fn packet_event_variants_identify_serverbound_states() {
        assert_eq!(
            serverbound_state(&ServerboundPacket::Java(
                JavaServerboundPacket::ConfigSAcceptCodeOfConduct
            )),
            Some(4)
        );
        assert_eq!(
            serverbound_state(&ServerboundPacket::Java(
                JavaServerboundPacket::LoginSLoginAcknowledged
            )),
            Some(2)
        );
        assert_eq!(
            serverbound_state(&ServerboundPacket::Java(
                JavaServerboundPacket::StatusSStatusRequest
            )),
            Some(1)
        );
        assert_eq!(
            serverbound_state(&ServerboundPacket::Java(
                JavaServerboundPacket::SPlayerLoaded
            )),
            Some(5)
        );
        assert_eq!(serverbound_state(&ServerboundPacket::Unknown), None);
    }

    #[test]
    fn packet_event_variants_identify_clientbound_states() {
        assert_eq!(
            clientbound_state(&ClientboundPacket::Java(
                JavaClientboundPacket::ConfigCConfigClearDialog
            )),
            Some(4)
        );
        assert_eq!(
            clientbound_state(&ClientboundPacket::Java(
                JavaClientboundPacket::CBundleDelimiter
            )),
            Some(5)
        );
        assert_eq!(clientbound_state(&ClientboundPacket::Unknown), None);
    }
}
