use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{F32T, F64T, I32T, I64T, STRING, U8T, VAR_INT};
use crate::api::{ComposedMappings, PacketWrapper, TranslateError, UserConnection};

/// 1.19 replaced the parser identifier with an id; core writes the older form
/// itself (`ArgumentType::legacy_identifier_name_for_version`).
const FIRST_PARSER_ID: JavaMinecraftVersion = JavaMinecraftVersion::V_1_19;
/// 1.19.4 gave `minecraft:time` a minimum.
const FIRST_TIME_MINIMUM: JavaMinecraftVersion = JavaMinecraftVersion::V_1_19_4;

const STRING_PARSER: i32 = 5;
/// Parser ids resolved from ViaVersion's checked-in 26.3 identifier data.
/// They are context_float_provider (55), context_int_provider (56),
/// slot_source (57), feature (59) and swing_animation (60).
const QUOTABLE_26_3_ARGUMENTS: &[i32] = &[55, 56, 57, 59, 60];
const NODE_TYPE: u8 = 3;
const HAS_REDIRECT: u8 = 8;
const HAS_SUGGESTION_TYPE: u8 = 16;

/// Copies a parser's properties, or reads past them when the client is getting
/// a plain string instead.
fn properties(
    wrapper: &mut PacketWrapper,
    parser: i32,
    layout: JavaMinecraftVersion,
    keep: bool,
) -> Result<(), TranslateError> {
    match parser {
        1 => number(wrapper, &F32T, keep),
        2 => number(wrapper, &F64T, keep),
        3 => number(wrapper, &I32T, keep),
        4 => number(wrapper, &I64T, keep),
        STRING_PARSER => copy(wrapper, &VAR_INT, keep),
        6 | 31 => copy(wrapper, &U8T, keep),
        43 if layout >= FIRST_TIME_MINIMUM => copy(wrapper, &I32T, keep),
        44..=47 => copy(wrapper, &STRING, keep),
        _ => Ok(()),
    }
}

fn copy<T: crate::api::types::WireType>(
    wrapper: &mut PacketWrapper,
    t: &T,
    keep: bool,
) -> Result<(), TranslateError> {
    let value = wrapper.read(t)?;
    if keep {
        wrapper.write(t, &value)?;
    }
    Ok(())
}

fn number<T: crate::api::types::WireType>(
    wrapper: &mut PacketWrapper,
    t: &T,
    keep: bool,
) -> Result<(), TranslateError> {
    let flags = wrapper.read(&U8T)?;
    if keep {
        wrapper.write(&U8T, &flags)?;
    }
    if flags & 1 != 0 {
        copy(wrapper, t, keep)?;
    }
    if flags & 2 != 0 {
        copy(wrapper, t, keep)?;
    }
    Ok(())
}

pub fn commands(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout < FIRST_PARSER_ID {
        wrapper.passthrough_all();
        return Ok(());
    }

    let string_parser = u32::try_from(STRING_PARSER)
        .ok()
        .and_then(|id| ids.argumenttypes.map(id))
        .and_then(|id| i32::try_from(id).ok())
        .ok_or(TranslateError::Unsupported("string argument type"))?;

    let nodes = wrapper.passthrough(&VAR_INT)?.0;
    for _ in 0..nodes {
        let flags = wrapper.passthrough(&U8T)?;
        let children = wrapper.passthrough(&VAR_INT)?.0;
        for _ in 0..children {
            wrapper.passthrough(&VAR_INT)?;
        }
        if flags & HAS_REDIRECT != 0 {
            wrapper.passthrough(&VAR_INT)?;
        }
        let node_type = flags & NODE_TYPE;
        if node_type == 1 || node_type == 2 {
            wrapper.passthrough(&STRING)?;
        }
        if node_type == 2 {
            let parser = wrapper.read(&VAR_INT)?.0;
            let mapped = u32::try_from(parser)
                .ok()
                .and_then(|id| ids.argumenttypes.map(id))
                .and_then(|id| i32::try_from(id).ok());
            // The 26.3->26.2 Via handler forces these named parser types to
            // brigadier:string with QUOTABLE_PHRASE, even where the numeric
            // mapping already aliases them to string.
            let force_quotable_fallback = QUOTABLE_26_3_ARGUMENTS.contains(&parser);
            let keep = !force_quotable_fallback
                && (mapped.is_some_and(|mapped| mapped != string_parser)
                    || parser == STRING_PARSER);
            let fallback = if force_quotable_fallback {
                string_parser
            } else {
                mapped.unwrap_or(string_parser)
            };
            wrapper.write(&VAR_INT, &VarInt(fallback))?;
            properties(wrapper, parser, layout, keep)?;
            if !keep {
                // ViaBackwards' 26.3 rewriter maps the newly added feature,
                // slot_source, swing_animation and context provider parsers to
                // brigadier:string with QUOTABLE_PHRASE. PJM has numeric ids
                // rather than Via's parser-name table, so this checked-in id
                // range uses that same safe quoting mode.
                wrapper.write(
                    &VAR_INT,
                    &VarInt(if force_quotable_fallback { 1 } else { 0 }),
                )?;
            }
        }
        if flags & HAS_SUGGESTION_TYPE != 0 {
            wrapper.passthrough(&STRING)?;
        }
    }
    wrapper.passthrough(&VAR_INT)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::packet::mappings::clientbound::play::COMMANDS;
    use pumpkin_protocol::ser::NetworkWriteExt;

    /// One root with one argument child, the smallest tree core writes.
    fn payload(parser: i32, properties: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.write_var_int(&VarInt(2)).unwrap();
        out.write_u8(0).unwrap();
        out.write_var_int(&VarInt(1)).unwrap();
        out.write_var_int(&VarInt(1)).unwrap();
        out.write_u8(2 | 4).unwrap();
        out.write_var_int(&VarInt(0)).unwrap();
        out.write_string("arg").unwrap();
        out.write_var_int(&VarInt(parser)).unwrap();
        out.extend_from_slice(properties);
        out.write_var_int(&VarInt(0)).unwrap();
        out
    }

    fn run(payload: &[u8], version: JavaMinecraftVersion) -> Vec<u8> {
        let ids = MappingData::get().composed(version);
        let mut wrapper = PacketWrapper::new(&COMMANDS, payload);
        let mut connection = UserConnection::new(0, version);
        commands(&mut wrapper, &mut connection, version, ids).unwrap();
        wrapper.finish().unwrap().unwrap().payload
    }

    /// 1.20.3 added `minecraft:style`, so every parser above it is one lower
    /// on 1.20.2.
    #[test]
    fn a_known_parser_keeps_its_properties() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_20_2,
        ] {
            let ids = MappingData::get().composed(version);
            let mapped = i32::try_from(ids.argumenttypes.map(3).unwrap()).unwrap();
            // An integer with a minimum of 1.
            let out = run(&payload(3, &[1, 0, 0, 0, 1]), version);
            assert_eq!(out, payload(mapped, &[1, 0, 0, 0, 1]), "{version}");
        }
    }

    /// Below 1.19 core writes the parser identifier, so the tree is copied.
    #[test]
    fn the_string_form_is_left_alone() {
        let version = JavaMinecraftVersion::V_1_18_2;
        let mut out = Vec::new();
        out.write_var_int(&VarInt(1)).unwrap();
        out.write_u8(0).unwrap();
        out.write_var_int(&VarInt(0)).unwrap();
        out.write_var_int(&VarInt(0)).unwrap();
        assert_eq!(run(&out, version), out);
    }

    /// `minecraft:hex_color` is 26.x only, so on 1.20.2 the node becomes a
    /// single word string and its properties go with it.
    #[test]
    fn a_parser_the_client_lacks_becomes_a_string() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let ids = MappingData::get().composed(version);
        assert_eq!(ids.argumenttypes.map(17), Some(5));

        let out = run(&payload(17, &[]), version);
        let string = i32::try_from(ids.argumenttypes.map(5).unwrap()).unwrap();
        assert_eq!(out, payload(string, &[0]));
    }

    #[test]
    fn the_five_263_only_parsers_fall_back_to_quotable_strings() {
        let mapping = MappingData::get();
        for version in [JavaMinecraftVersion::V_26_2] {
            let ids = mapping.composed(version);
            let string = i32::try_from(ids.argumenttypes.map(5).unwrap()).unwrap();
            for parser in QUOTABLE_26_3_ARGUMENTS {
                assert_eq!(
                    run(&payload(*parser, &[]), version),
                    payload(string, &[1]),
                    "ViaBackwards maps argument parser {parser} to quotable string on {version}"
                );
            }
        }
    }

    /// Dialog is 58 and uuid 61 on 26.3; both are taken as core writes them.
    #[test]
    fn dialog_and_uuid_map_from_their_26_3_ids() {
        let version = JavaMinecraftVersion::V_1_21_6;
        let ids = MappingData::get().composed(version);
        for parser in [58, 61] {
            let mapped = i32::try_from(ids.argumenttypes.map(parser).unwrap()).unwrap();
            assert_eq!(
                run(&payload(parser as i32, &[]), version),
                payload(mapped, &[])
            );
        }
    }
}
