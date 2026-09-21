use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::block::RawNbtT;
use crate::api::types::{BOOL, I8T, OptionalT, STRING, U8T, VAR_INT};
use crate::api::{ComposedMappings, PacketWrapper, TranslateError, UserConnection};

/// 1.17 replaced the tracking flag with an optional icon list.
const FIRST_OPTIONAL_ICONS: JavaMinecraftVersion = JavaMinecraftVersion::V_1_17;
/// Text components travel as network NBT from 1.20.3 and as JSON below it.
const FIRST_NBT_COMPONENT: JavaMinecraftVersion = JavaMinecraftVersion::V_1_20_3;

pub fn open_screen(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let menu = wrapper.read(&VAR_INT)?.0;
    let mapped = u32::try_from(menu)
        .ok()
        .and_then(|id| ids.menus.map(id))
        .and_then(|id| i32::try_from(id).ok());
    let Some(menu) = mapped else {
        wrapper.cancel();
        return Ok(());
    };
    wrapper.write(&VAR_INT, &VarInt(menu))?;
    wrapper.passthrough_all();
    Ok(())
}

/// How many decoration types a version has, from minecraft-data's `mapIcons`:
/// 27 up to 1.20.1, 34 from 1.20.2 (the village and temple markers) and 35
/// from 1.20.5 (trial chambers), next to the stand in for the rest.
const DECORATIONS: &[(JavaMinecraftVersion, u32, i32)] = &[
    (JavaMinecraftVersion::V_1_20_5, 35, 34),
    (JavaMinecraftVersion::V_1_20_2, 34, 32),
    (JavaMinecraftVersion::V_1_13, 27, 2),
];

/// A decoration the client does not have becomes the stand in `ViaBackwards`
/// picks for it: trial chambers, then jungle temple, then the red marker.
fn decoration(id: i32, version: JavaMinecraftVersion) -> i32 {
    if version >= JavaMinecraftVersion::V_26_3 {
        return id;
    }
    let Some((_, size, fallback)) = DECORATIONS
        .iter()
        .find(|(first, _, _)| version >= *first)
        .copied()
    else {
        return id;
    };
    if u32::try_from(id).is_ok_and(|id| id < size) {
        id
    } else {
        fallback
    }
}

pub fn map_item_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&I8T)?;
    if layout < FIRST_OPTIONAL_ICONS {
        // The tracking flag, dropped in 1.17.
        wrapper.passthrough(&BOOL)?;
        wrapper.passthrough(&BOOL)?;
    } else {
        wrapper.passthrough(&BOOL)?;
        if !wrapper.passthrough(&BOOL)? {
            wrapper.passthrough_all();
            return Ok(());
        }
    }

    let count = wrapper.passthrough(&VAR_INT)?.0;
    for _ in 0..count {
        let icon = wrapper.read(&VAR_INT)?.0;
        wrapper.write(&VAR_INT, &VarInt(decoration(icon, layout)))?;
        wrapper.passthrough(&I8T)?;
        wrapper.passthrough(&I8T)?;
        wrapper.passthrough(&U8T)?;
        if layout >= FIRST_NBT_COMPONENT {
            wrapper.passthrough(&OptionalT(RawNbtT::for_version(layout)))?;
        } else {
            wrapper.passthrough(&OptionalT(STRING))?;
        }
    }
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::packet::mappings::clientbound::play::{MAP_ITEM_DATA, OPEN_SCREEN};
    use pumpkin_protocol::ser::NetworkWriteExt;

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

    /// 1.20.3 put the crafter in the middle of the menu registry, so every
    /// later menu is one lower on 1.20.2 and unchanged above it.
    #[test]
    fn the_menu_type_is_renumbered() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_20_2,
        ] {
            let ids = MappingData::get().composed(version);
            let payload = [1u8, 20, 0];
            let out = run(open_screen, &OPEN_SCREEN, &payload, version).unwrap();
            assert_eq!(
                out[1],
                u8::try_from(ids.menus.map(20).unwrap()).unwrap(),
                "{version}"
            );
            assert_eq!(out[2], 0, "{version}");
        }
    }

    /// A menu past the end of the 1.20.2 registry has no stand in.
    #[test]
    fn a_menu_the_client_lacks_drops_the_packet() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let ids = MappingData::get().composed(version);
        let absent = i32::try_from(ids.menus.len()).unwrap();
        let mut payload = vec![1u8];
        payload.write_var_int(&VarInt(absent)).unwrap();
        payload.push(0);
        assert!(run(open_screen, &OPEN_SCREEN, &payload, version).is_none());
    }

    fn map_payload(icon: i32, version: JavaMinecraftVersion) -> Vec<u8> {
        let mut payload = vec![7u8, 0];
        if version < FIRST_OPTIONAL_ICONS {
            payload.push(1);
        }
        payload.push(0);
        if version >= FIRST_OPTIONAL_ICONS {
            payload.push(1);
        }
        payload.write_var_int(&VarInt(1)).unwrap();
        payload.write_var_int(&VarInt(icon)).unwrap();
        payload.extend_from_slice(&[1, 2, 3, 0]);
        // An empty patch.
        payload.push(0);
        payload
    }

    /// The decoration list is the same up to 1.20.1, so only the newest
    /// markers move; the display name is NBT from 1.20.3 and a string below.
    #[test]
    fn map_decorations_fall_back_per_version() {
        for (version, icon, expected) in [
            (JavaMinecraftVersion::V_26_2, 34, 34),
            (JavaMinecraftVersion::V_1_21, 40, 34),
            (JavaMinecraftVersion::V_1_20_3, 34, 32),
            (JavaMinecraftVersion::V_1_16_2, 30, 2),
            (JavaMinecraftVersion::V_1_16_2, 26, 26),
        ] {
            let out = run(
                map_item_data,
                &MAP_ITEM_DATA,
                &map_payload(icon, version),
                version,
            )
            .unwrap();
            assert_eq!(out, map_payload(expected, version), "{version} {icon}");
        }
    }

    #[test]
    fn a_map_without_icons_is_copied() {
        let version = JavaMinecraftVersion::V_1_21;
        let payload = [7u8, 0, 0, 0, 0];
        assert_eq!(
            run(map_item_data, &MAP_ITEM_DATA, &payload, version).unwrap(),
            payload
        );
    }
}
