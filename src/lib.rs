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
        EventHandler, EventPriority,
        packet::{PacketReceivedEvent, PacketSentEvent},
        player::player_leave::PlayerLeaveEvent,
    },
    events_wit::{PacketReceivedEventData, PacketSentEventData, PlayerLeaveEventData},
    register_plugin,
};

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
        // Login and configuration packets arrive before a Player exists, so
        // the version comes off the event itself rather than off the player.
        let version = JavaMinecraftVersion::from_protocol(event.protocol_version as u32);
        if version == JavaMinecraftVersion::V_26_3 {
            return event;
        }
        // An unsupported client is refused on its first clientbound login
        // packet (see `refuse_unsupported`); its login start has to reach the
        // server untouched for that packet to be sent at all.
        if !is_version_supported(version) {
            return event;
        }
        // Handshake and status ids never changed and carry no version yet.
        if event.connection_state < 2 {
            return event;
        }
        match pipeline::translate_serverbound(
            event.connection_id,
            version,
            event.connection_state,
            event.packet_id,
            &event.raw_payload,
        ) {
            Some(translated) => {
                event.packet_id = translated.packet.v26_3;
                event.raw_payload = translated.payload;
                event.reply_packets = translated
                    .replies
                    .into_iter()
                    .filter_map(|(packet, payload)| {
                        let id = packet.to_id(version);
                        (id != -1).then_some((id, payload))
                    })
                    .collect();
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
        // Login and configuration packets are sent before a Player exists, so the
        // version comes off the event itself rather than off the player.
        let version = JavaMinecraftVersion::from_protocol(event.protocol_version as u32);
        if version == JavaMinecraftVersion::V_26_3 {
            return event;
        }
        if !is_version_supported(version) {
            return refuse_unsupported(event, version);
        }
        // Handshake has no clientbound packets and therefore no table.
        if event.connection_state == 0 {
            return event;
        }
        #[cfg(feature = "rawdump")]
        tracing::info!(
            "RAWDUMP {} {} {} {}",
            event.protocol_version,
            event.connection_state,
            event.packet_id,
            hex(&event.raw_payload)
        );
        let key = event.connection_id;
        if let Some(player) = &event.player
            && !is_bound(key)
        {
            bind_player(key, version, player);
        }
        match pipeline::translate_clientbound(
            key,
            version,
            event.connection_state,
            event.packet_id,
            &event.raw_payload,
        ) {
            Some(translated) => {
                event.packet_id = translated.packet.to_id(version);
                event.raw_payload = translated.payload;
                event.extra_packets = translated
                    .extra
                    .into_iter()
                    .filter_map(|(packet, payload)| {
                        let id = packet.to_id(version);
                        (id != -1).then_some((id, payload))
                    })
                    .collect();
            }
            // No id for this version: the packet does not exist on the client.
            // Sending it under a 26.3 id would desync the stream, so drop it.
            None => event.cancelled = true,
        }
        event
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
) -> PacketSentEventData {
    // 0 handshake, 1 status, 2 login, 3 transfer, 4 config, 5 play.
    if event.connection_state != 2 && event.connection_state != 3 {
        event.cancelled = event.connection_state != 1;
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
}
