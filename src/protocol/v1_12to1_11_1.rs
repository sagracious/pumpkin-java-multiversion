//! ViaBackwards' 1.12 -> 1.11.1 protocol step.
//!
//! Older metadata/entity tables and the 1.12 -> 1.11.1 registry mapping are
//! integration dependencies. This module contains the packet-local rewrites
//! that can be applied without changing those shared tables.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_util::version::JavaMinecraftVersion as V;
use serde_json::Value;

use crate::api::types::{F64T, I8T, I32T, STRING, U8T, VAR_INT, WireType};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;

const MAP_COLOR_REPLACEMENTS: &[(u8, u8)] = &[
    (144, 59),
    (145, 56),
    (146, 56),
    (147, 45),
    (148, 63),
    (149, 60),
    (150, 60),
    (151, 136),
    (152, 83),
    (153, 83),
    (154, 80),
    (155, 115),
    (156, 39),
    (157, 39),
    (158, 36),
    (159, 47),
    (160, 60),
    (161, 61),
    (162, 62),
    (163, 137),
    (164, 108),
    (165, 108),
    (166, 109),
    (167, 111),
    (168, 112),
    (169, 113),
    (170, 114),
    (171, 115),
    (172, 118),
    (173, 107),
    (174, 107),
    (175, 118),
    (176, 91),
    (177, 45),
    (178, 46),
    (179, 47),
    (180, 85),
    (181, 44),
    (182, 27),
    (183, 84),
    (184, 83),
    (185, 83),
    (186, 83),
    (187, 84),
    (188, 84),
    (189, 71),
    (190, 71),
    (191, 87),
    (192, 107),
    (193, 139),
    (194, 43),
    (195, 107),
    (196, 111),
    (197, 111),
    (198, 111),
    (199, 107),
    (200, 112),
    (201, 113),
    (202, 113),
    (203, 115),
    (204, 116),
    (205, 117),
    (206, 107),
    (207, 119),
];

pub struct Protocol1_12To1_11_1;

impl Protocol for Protocol1_12To1_11_1 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_12,
            to: V::V_1_11_1,
        }
    }

    fn register(&self, registry: &mut Registry) {
        registry.clientbound(&clientbound::play::CHAT, chat);
        registry.clientbound(&clientbound::play::TITLE, title);
        registry.clientbound(&clientbound::play::MAP_ITEM_DATA, map_item_data);
        registry.clientbound(&clientbound::play::UPDATE_ATTRIBUTES, update_attributes);
        registry.cancel_clientbound(&clientbound::play::UPDATE_ADVANCEMENTS);
        registry.cancel_clientbound(&clientbound::play::UNLOCK_RECIPES);
        registry.cancel_clientbound(&clientbound::play::SELECT_ADVANCEMENTS_TAB);
    }
}

fn rewrite_keybind_component(raw: &str) -> Result<Box<str>, TranslateError> {
    let mut component: Value = serde_json::from_str(raw)
        .map_err(|_| TranslateError::Unsupported("chat component json"))?;
    replace_keybinds(&mut component);
    Ok(serde_json::to_string(&component)
        .map_err(|_| TranslateError::Unsupported("chat component json"))?
        .into())
}

fn replace_keybinds(component: &mut Value) {
    match component {
        Value::Array(values) => values.iter_mut().for_each(replace_keybinds),
        Value::Object(object) => {
            if let Some(Value::String(key)) = object.remove("keybind") {
                object.insert("text".to_owned(), Value::String(key));
            }
            object.values_mut().for_each(replace_keybinds);
        }
        _ => {}
    }
}

fn chat(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let text = wrapper.read(&STRING)?;
    let text = rewrite_keybind_component(&text)?;
    wrapper.write(&STRING, &text)?;
    wrapper.passthrough_all();
    Ok(())
}

fn title(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let action = wrapper.passthrough(&VAR_INT)?.0;
    if (0..=2).contains(&action) {
        let text = wrapper.read(&STRING)?;
        wrapper.write(&STRING, &rewrite_keybind_component(&text)?)?;
    }
    wrapper.passthrough_all();
    Ok(())
}

/// The 1.12 map patch encodes colors as bytes; map colors 144..207 are not in
/// 1.11.1 and use ViaBackwards' nearest-color table.
fn map_item_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?; // Map id.
    wrapper.passthrough(&I8T)?; // Scale.
    wrapper.passthrough(&crate::api::types::BOOL)?; // Tracking position.
    let icons = wrapper.read(&VAR_INT)?.0;
    if !(0..=4096).contains(&icons) {
        return Err(TranslateError::Unsupported("map icon count"));
    }
    wrapper.write(&VAR_INT, &VarInt(icons))?;
    for _ in 0..icons {
        wrapper.passthrough(&U8T)?; // Icon type.
        wrapper.passthrough(&I8T)?; // X.
        wrapper.passthrough(&I8T)?; // Z.
    }

    let columns = wrapper.passthrough(&U8T)?;
    if columns == 0 {
        wrapper.passthrough_all();
        return Ok(());
    }
    wrapper.passthrough(&U8T)?; // Rows.
    wrapper.passthrough(&U8T)?; // X.
    wrapper.passthrough(&U8T)?; // Z.
    let mut pixels = wrapper.read(&crate::api::types::BYTE_ARRAY)?;
    for color in &mut pixels {
        if *color > 143 {
            *color = old_map_color(*color);
        }
    }
    wrapper.write(&crate::api::types::BYTE_ARRAY, &pixels)?;
    Ok(())
}

fn old_map_color(color: u8) -> u8 {
    MAP_COLOR_REPLACEMENTS
        .iter()
        .find_map(|&(new, old)| (new == color).then_some(old))
        .unwrap_or(color)
}

fn update_attributes(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?; // Entity id.
    let count = wrapper.read(&I32T)?;
    if !(0..=4096).contains(&count) {
        return Err(TranslateError::Unsupported("attribute count"));
    }
    let mut kept = 0i32;
    let mut out = Vec::new();
    for _ in 0..count {
        let name = wrapper.read(&STRING)?;
        let value = wrapper.read(&F64T)?;
        let modifiers = wrapper.read(&VAR_INT)?.0;
        if !(0..=1024).contains(&modifiers) {
            return Err(TranslateError::Unsupported("attribute modifier count"));
        }
        let mut encoded_modifiers = Vec::new();
        for _ in 0..modifiers {
            let uuid = wrapper.read(&crate::api::types::UUID)?;
            let amount = wrapper.read(&F64T)?;
            let operation = wrapper.read(&U8T)?;
            crate::api::types::UUID.write(&mut encoded_modifiers, &uuid)?;
            F64T.write(&mut encoded_modifiers, &amount)?;
            U8T.write(&mut encoded_modifiers, &operation)?;
        }
        if name.as_ref() == "generic.flyingSpeed" {
            continue;
        }
        STRING.write(&mut out, &name)?;
        F64T.write(&mut out, &value)?;
        VAR_INT.write(&mut out, &VarInt(modifiers))?;
        out.extend_from_slice(&encoded_modifiers);
        kept += 1;
    }
    let mut encoded = Vec::with_capacity(out.len() + 4);
    I32T.write(&mut encoded, &kept)?;
    encoded.extend_from_slice(&out);
    wrapper.replace_remaining(encoded);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybind_components_become_plain_text_recursively() {
        let input = r#"{"text":"Press ","extra":[{"keybind":"key.jump"}]}"#;
        let output = rewrite_keybind_component(input).unwrap();
        let json: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["extra"][0]["text"], "key.jump");
        assert!(json["extra"][0].get("keybind").is_none());
    }

    #[test]
    fn only_new_112_map_colors_are_replaced() {
        assert_eq!(old_map_color(143), 143);
        assert_eq!(old_map_color(144), 59);
        assert_eq!(old_map_color(207), 119);
        assert_eq!(old_map_color(208), 208);
    }

    #[test]
    fn the_step_is_exactly_112_to_1111() {
        let step = Protocol1_12To1_11_1.step();
        assert_eq!(step.from, V::V_1_12);
        assert_eq!(step.to, V::V_1_11_1);
    }
}
