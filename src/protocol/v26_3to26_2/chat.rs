use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};

use crate::api::rewriter::chat as chat_rewriter;
use crate::api::types::{BIT_SET, BOOL, BYTE_ARRAY, I64, NbtT, VAR_INT, WireType};
use crate::api::{Ctx, PacketWrapper, Registry, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;

const UNSUPPORTED_HOVER_ITEM_COMPONENTS: &[&str] = &[
    "attack_animation",
    "interact_animation",
    "provides_trim_material",
    "provides_pottery_pattern",
    "block_transformer",
    "compostable",
    "trim",
    "cooking_fuel",
    "brewing_fuel",
    "villager_food",
    "mob_visibility",
    "sign_text_front",
    "sign_text_back",
    "waxed",
    "cushion_color",
    "pot_decorations",
];

pub(super) fn register(reg: &mut Registry) {
    // Strip 26.3-only item components inside chat hover payloads. The core
    // chooses the surrounding packet layout for the negotiated version.
    reg.clientbound(&clientbound::play::PLAYER_CHAT, player_chat);
}

fn byte_set_to_words(bytes: &[u8]) -> Vec<i64> {
    let mut words: Vec<i64> = bytes
        .chunks(8)
        .map(|chunk| {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            i64::from_le_bytes(word)
        })
        .collect();
    while words.last() == Some(&0) {
        words.pop();
    }
    words
}

fn write_long_array(wrapper: &mut PacketWrapper, values: &[i64]) -> Result<(), TranslateError> {
    wrapper.write(
        &VAR_INT,
        &VarInt(
            i32::try_from(values.len())
                .map_err(|_| TranslateError::Unsupported("chat bitset length"))?,
        ),
    )?;
    for value in values {
        wrapper.write(&I64, value)?;
    }
    Ok(())
}

fn strip_hover_item_components(tag: &mut NbtTag) {
    match tag {
        NbtTag::Compound(compound) => strip_compound(compound),
        NbtTag::List(entries) => {
            for entry in entries {
                strip_hover_item_components(entry);
            }
        }
        _ => {}
    }
}

fn strip_compound(compound: &mut NbtCompound) {
    if let Some(NbtTag::Compound(components)) = compound.child_tags.get_mut("components") {
        for name in UNSUPPORTED_HOVER_ITEM_COMPONENTS {
            components.child_tags.remove(*name);
            components
                .child_tags
                .remove(format!("minecraft:{name}").as_str());
        }
    }
    for child in compound.child_tags.values_mut() {
        strip_hover_item_components(child);
    }
}

pub(super) fn rewrite_component_tag(tag: Option<NbtTag>) -> Option<NbtTag> {
    let mut tag = tag?;
    strip_hover_item_components(&mut tag);
    Some(tag)
}

fn player_chat(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    // Common signed header: global index, sender, sender index, signature,
    // plain text, timestamp, salt, and the last-seen signature list.
    chat_rewriter::signed_head(wrapper, true)?;

    let has_unsigned_content = wrapper.passthrough(&BOOL)?;
    if has_unsigned_content {
        let component = wrapper.read(&NbtT::for_version(ctx.layout))?;
        wrapper.write(
            &NbtT::for_version(ctx.layout),
            &rewrite_component_tag(component),
        )?;
    }

    let filter_type = wrapper.passthrough(&VAR_INT)?.0;
    if filter_type == 2 {
        if ctx.layout >= pumpkin_util::version::JavaMinecraftVersion::V_26_3 {
            let bytes = wrapper.read(&BYTE_ARRAY)?;
            let words = byte_set_to_words(&bytes);
            write_long_array(wrapper, &words)?;
        } else {
            wrapper.passthrough(&BIT_SET)?;
        }
    }

    wrapper.passthrough(&VAR_INT)?; // chat type holder
    let name = wrapper.read(&NbtT::for_version(ctx.layout))?;
    wrapper.write(&NbtT::for_version(ctx.layout), &rewrite_component_tag(name))?;

    let has_target_name = wrapper.passthrough(&BOOL)?;
    if has_target_name {
        let target = wrapper.read(&NbtT::for_version(ctx.layout))?;
        wrapper.write(
            &NbtT::for_version(ctx.layout),
            &rewrite_component_tag(target),
        )?;
    }
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_filter_mask_bytes_use_little_endian_words_and_drop_zero_tail() {
        assert_eq!(
            byte_set_to_words(&[1, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            vec![i64::from_le_bytes([1, 0, 0, 0, 0, 0, 0, 0]), 128]
        );
        assert!(byte_set_to_words(&[0, 0, 0]).is_empty());
    }

    #[test]
    fn v26_3_only_hover_item_components_are_removed_recursively() {
        let mut components = NbtCompound::new();
        components.put_string("minecraft:interact_animation", "test".to_string());
        components.put_string("minecraft:custom_name", "keep".to_string());
        let mut hover = NbtCompound::new();
        hover.put_compound("components", components);
        let mut root = NbtCompound::new();
        root.put_compound("hoverEvent", hover);

        let rewritten = rewrite_component_tag(Some(NbtTag::Compound(root))).unwrap();
        let NbtTag::Compound(root) = rewritten else {
            panic!("compound")
        };
        let hover = root.get_compound("hoverEvent").unwrap();
        let components = hover.get_compound("components").unwrap();
        assert!(components.get("minecraft:interact_animation").is_none());
        assert_eq!(components.get_string("minecraft:custom_name"), Some("keep"));
    }
}
