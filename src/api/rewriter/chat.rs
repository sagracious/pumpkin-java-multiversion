use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::text::TextComponent;

use crate::api::types::{
    BIT_SET, BOOL, BYTE_ARRAY, FixedBytesT, I64T, OptionalT, STRING, TextComponentT, UUID, VAR_INT,
    WireType,
};
use crate::api::{PacketWrapper, TranslateError, UserConnection};

const SIGNATURE: FixedBytesT = FixedBytesT(256);

/// Core writes every text component with the client's own version, so a
/// component is in the client's form whatever layout the frame around it is in.
#[must_use]
pub fn text(connection: &UserConnection) -> TextComponentT {
    TextComponentT::for_version(connection.version)
}

/// The decoration of every entry of the 26.3 `chat_type` registry, in the
/// order the server numbers them: chat, emote, the two message commands, say,
/// the two team message commands and raw.
const DECORATIONS: &[(&str, &[Parameter])] = &[
    ("chat.type.text", &[Parameter::Sender, Parameter::Content]),
    ("chat.type.emote", &[Parameter::Sender, Parameter::Content]),
    (
        "commands.message.display.incoming",
        &[Parameter::Sender, Parameter::Content],
    ),
    (
        "commands.message.display.outgoing",
        &[Parameter::Target, Parameter::Content],
    ),
    (
        "chat.type.announcement",
        &[Parameter::Sender, Parameter::Content],
    ),
    (
        "chat.type.team.text",
        &[Parameter::Target, Parameter::Sender, Parameter::Content],
    ),
    (
        "chat.type.team.sent",
        &[Parameter::Target, Parameter::Sender, Parameter::Content],
    ),
    ("%s", &[Parameter::Content]),
];

#[derive(Clone, Copy)]
enum Parameter {
    Sender,
    Target,
    Content,
}

/// Applies the chat type's decoration, which a client below 1.19.3 cannot do
/// for a message it receives as a system one.
// The key comes off a table, so `translate_java!` cannot check it.
#[expect(deprecated)]
pub fn decorate(
    chat_type: i32,
    sender: TextComponent,
    target: Option<TextComponent>,
    content: TextComponent,
) -> Result<TextComponent, TranslateError> {
    let (key, parameters) = usize::try_from(chat_type)
        .ok()
        .and_then(|index| DECORATIONS.get(index))
        .ok_or(TranslateError::Unsupported("chat type"))?;
    let with = parameters
        .iter()
        .map(|parameter| match parameter {
            Parameter::Sender => sender.clone(),
            Parameter::Target => target.clone().unwrap_or_else(TextComponent::empty),
            Parameter::Content => content.clone(),
        })
        .collect::<Vec<_>>();
    Ok(TextComponent::translate(key.to_string(), with))
}

/// Reads the trailing chat type, sender and target of a 1.19.3 chat packet and
/// returns the decorated message a system chat carries in their place.
fn decorated_tail(
    wrapper: &mut PacketWrapper,
    content: TextComponent,
    text: TextComponentT,
) -> Result<TextComponent, TranslateError> {
    let chat_type = wrapper.read(&VAR_INT)?.0;
    let sender = wrapper.read(&text)?;
    let target = wrapper.read(&OptionalT(text))?;
    decorate(chat_type, sender, target, content)
}

/// Turns a `DISGUISED_CHAT` into the 1.19.1 `SYSTEM_CHAT` body.
pub fn disguised_chat_to_system(
    wrapper: &mut PacketWrapper,
    text: TextComponentT,
) -> Result<(), TranslateError> {
    let content = wrapper.read(&text)?;
    let message = decorated_tail(wrapper, content, text)?;
    wrapper.write(&text, &message)?;
    wrapper.write(&BOOL, &false)
}

/// Turns a `PLAYER_CHAT` into the 1.19.1 `SYSTEM_CHAT` body. The signed half
/// is dropped: nothing below 1.19.3 can verify a signature keyed by index.
pub fn player_chat_to_system(
    wrapper: &mut PacketWrapper,
    text: TextComponentT,
) -> Result<(), TranslateError> {
    let plain = signed_head(wrapper, false)?;
    let unsigned = wrapper.read(&OptionalT(text))?;
    filter_mask(wrapper, false)?;

    let content = unsigned.unwrap_or_else(|| TextComponent::text(plain.to_string()));
    let message = decorated_tail(wrapper, content, text)?;
    wrapper.write(&text, &message)?;
    wrapper.write(&BOOL, &false)
}

fn take<T: WireType>(
    wrapper: &mut PacketWrapper,
    t: &T,
    keep: bool,
) -> Result<T::Value, TranslateError> {
    if keep {
        wrapper.passthrough(t)
    } else {
        wrapper.read(t)
    }
}

/// Everything a `PLAYER_CHAT` carries before its unsigned content, returning
/// the signed text. `keep` writes it back; a message becoming a system one
/// throws it away.
pub fn signed_head(wrapper: &mut PacketWrapper, keep: bool) -> Result<Box<str>, TranslateError> {
    take(wrapper, &UUID, keep)?;
    take(wrapper, &VAR_INT, keep)?;
    if take(wrapper, &BOOL, keep)? {
        take(wrapper, &SIGNATURE, keep)?;
    }
    let plain = take(wrapper, &STRING, keep)?;
    take(wrapper, &I64T, keep)?;
    take(wrapper, &I64T, keep)?;
    let previous = take(wrapper, &VAR_INT, keep)?.0;
    if !(0..=20).contains(&previous) {
        return Err(TranslateError::Unsupported("last seen count"));
    }
    for _ in 0..previous {
        if take(wrapper, &VAR_INT, keep)?.0 == 0 {
            take(wrapper, &SIGNATURE, keep)?;
        }
    }
    Ok(plain)
}

pub fn filter_mask(wrapper: &mut PacketWrapper, keep: bool) -> Result<(), TranslateError> {
    if take(wrapper, &VAR_INT, keep)?.0 == 2 {
        take(wrapper, &BIT_SET, keep)?;
    }
    Ok(())
}

/// Copies the chat type across, unwrapping the holder 1.21 introduced. An
/// inline entry has no id an older client could resolve.
pub fn chat_type_to_id(wrapper: &mut PacketWrapper) -> Result<(), TranslateError> {
    let holder = wrapper.read(&VAR_INT)?.0;
    if holder <= 0 {
        return Err(TranslateError::Unsupported("inline chat type"));
    }
    wrapper.write(&VAR_INT, &VarInt(holder - 1))
}

/// The 1.19.2 `DELETE_CHAT` body: the whole signature as a length prefixed
/// array. A packed id refers to a cache the client keeps under a scheme
/// 1.19.2 does not have.
pub fn delete_chat_to_1_19_1(wrapper: &mut PacketWrapper) -> Result<(), TranslateError> {
    if wrapper.read(&VAR_INT)?.0 != 0 {
        return Err(TranslateError::Unsupported("packed signature id"));
    }
    let signature = wrapper.read(&SIGNATURE)?;
    wrapper.write(&BYTE_ARRAY, &signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::version::JavaMinecraftVersion;

    #[test]
    fn every_26_3_chat_type_has_a_decoration() {
        assert_eq!(DECORATIONS.len(), 8);
        let message = decorate(
            4,
            TextComponent::text("bob"),
            None,
            TextComponent::text("hi"),
        )
        .unwrap();
        let json = message.to_json_for_version(&JavaMinecraftVersion::V_1_16_2);
        assert!(json.contains("chat.type.announcement"), "{json}");
        assert!(json.contains("bob"), "{json}");
        assert!(decorate(8, TextComponent::empty(), None, TextComponent::empty()).is_err());
    }
}
