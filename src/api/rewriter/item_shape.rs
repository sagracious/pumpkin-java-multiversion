use pumpkin_data::data_component::DataComponent;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkReadSliceExt, ReadingError};
use pumpkin_util::version::JavaMinecraftVersion as V;

/// The 26.3 wire shape of a component payload; walking by shape avoids the registry tables the typed reader needs.
pub enum Shape {
    Unit,
    Bool,
    I32,
    F32,
    F64,
    VarInt,
    Str,
    Uuid,
    BlockPos,
    Nbt,
    Seq(&'static [Shape]),
    /// A varint count and that many values.
    Array(&'static Shape),
    /// A bool and the value when it is set.
    Opt(&'static Shape),
    /// A varint: zero is a tag name, anything else is that many ids less one.
    IdSet,
    /// A holder: zero is an inline sound, anything else is a registry id.
    Sound,
    /// A stack with the item id in front and no present flag.
    Template,
    /// A component id and that component's own payload.
    Component,
    StatusEffects,
    ConsumeEffect,
    /// A bool and then one of the two arms.
    BoolSwitch(&'static Shape, &'static Shape),
    /// A varint and then the arm it names, or the fallback.
    Switch(&'static [(i32, &'static Shape)], &'static Shape),
    /// Core's codec has no arm for this one, so it never reaches the wire.
    Absent,
}

const UNIT: Shape = Shape::Unit;
const BOOL: Shape = Shape::Bool;
const I32: Shape = Shape::I32;
const F32: Shape = Shape::F32;
pub(crate) const VAR_INT: Shape = Shape::VarInt;
pub(crate) const STR: Shape = Shape::Str;
pub(crate) const NBT: Shape = Shape::Nbt;
pub(crate) const ID_SET: Shape = Shape::IdSet;
pub(crate) const SOUND: Shape = Shape::Sound;
const TEMPLATE: Shape = Shape::Template;

static ENCHANTMENTS: Shape = Shape::Array(&Shape::Seq(&[Shape::VarInt, Shape::VarInt]));
static PAIR_VAR_INT: Shape = Shape::Array(&Shape::Seq(&[Shape::VarInt, Shape::VarInt]));

static KINETIC_CONDITION: Shape = Shape::Seq(&[Shape::VarInt, Shape::F32, Shape::F32]);

static TOOL: Shape = Shape::Seq(&[
    Shape::Array(&Shape::Seq(&[
        Shape::IdSet,
        Shape::Opt(&F32),
        Shape::Opt(&BOOL),
    ])),
    Shape::F32,
    Shape::VarInt,
    Shape::Bool,
]);

static CONSUMABLE: Shape = Shape::Seq(&[
    Shape::F32,
    Shape::VarInt,
    Shape::Sound,
    Shape::Bool,
    Shape::Array(&Shape::ConsumeEffect),
]);

static EQUIPPABLE: Shape = Shape::Seq(&[
    Shape::VarInt,
    Shape::Sound,
    Shape::Opt(&STR),
    Shape::Opt(&STR),
    Shape::Opt(&ID_SET),
    Shape::Bool,
    Shape::Bool,
    Shape::Bool,
    Shape::Bool,
    Shape::Bool,
    Shape::Sound,
]);

static POTION_CONTENTS: Shape = Shape::Seq(&[
    Shape::Opt(&VAR_INT),
    Shape::Opt(&I32),
    Shape::StatusEffects,
    Shape::Opt(&STR),
]);

static FIREWORK_EXPLOSION: Shape = Shape::Seq(&[
    Shape::VarInt,
    Shape::Array(&I32),
    Shape::Array(&I32),
    Shape::Bool,
    Shape::Bool,
]);

static FIREWORKS: Shape = Shape::Seq(&[Shape::VarInt, Shape::Array(&FIREWORK_EXPLOSION)]);

/// One entry of `can_place_on` / `can_break`.
static BLOCK_PREDICATE: Shape = Shape::Seq(&[
    Shape::Opt(&ID_SET),
    Shape::Opt(&Shape::Array(&Shape::Seq(&[
        Shape::Str,
        Shape::BoolSwitch(&STR, &Shape::Seq(&[Shape::Opt(&STR), Shape::Opt(&STR)])),
    ]))),
    Shape::Opt(&NBT),
    Shape::Array(&Shape::Component),
    Shape::Array(&VAR_INT),
]);

static BLOCK_PREDICATES: Shape = Shape::Array(&BLOCK_PREDICATE);

static ATTRIBUTE_MODIFIERS: Shape = Shape::Array(&Shape::Seq(&[
    Shape::VarInt,
    Shape::Str,
    Shape::F64,
    Shape::VarInt,
    Shape::VarInt,
    Shape::Switch(&[(2, &NBT)], &UNIT),
]));

static CUSTOM_MODEL_DATA: Shape = Shape::Seq(&[
    Shape::Array(&F32),
    Shape::Array(&BOOL),
    Shape::Array(&STR),
    Shape::Array(&I32),
]);

static BLOCKS_ATTACKS: Shape = Shape::Seq(&[
    Shape::F32,
    Shape::F32,
    Shape::Array(&Shape::Seq(&[
        Shape::F32,
        Shape::Opt(&ID_SET),
        Shape::F32,
        Shape::F32,
    ])),
    Shape::Switch(&[(1, &Shape::Seq(&[Shape::F32, Shape::F32]))], &UNIT),
    Shape::Opt(&ID_SET),
    Shape::Opt(&VAR_INT),
    Shape::Opt(&VAR_INT),
]);

static KINETIC_WEAPON: Shape = Shape::Seq(&[
    Shape::VarInt,
    Shape::VarInt,
    Shape::Opt(&KINETIC_CONDITION),
    Shape::Opt(&KINETIC_CONDITION),
    Shape::Opt(&KINETIC_CONDITION),
    Shape::F32,
    Shape::F32,
    Shape::Opt(&SOUND),
    Shape::Opt(&SOUND),
]);

static PIERCING_WEAPON: Shape = Shape::Seq(&[
    Shape::Bool,
    Shape::Bool,
    Shape::Opt(&SOUND),
    Shape::Opt(&SOUND),
]);

static WRITTEN_BOOK: Shape = Shape::Seq(&[
    Shape::Str,
    Shape::Opt(&STR),
    Shape::Str,
    Shape::VarInt,
    Shape::Array(&Shape::Seq(&[Shape::Nbt, Shape::Opt(&NBT)])),
    Shape::Bool,
]);

static WRITABLE_BOOK: Shape = Shape::Array(&Shape::Seq(&[Shape::Str, Shape::Opt(&STR)]));

static PROFILE: Shape = Shape::Seq(&[
    Shape::Switch(
        &[(0, &Shape::Seq(&[Shape::Uuid, Shape::Str]))],
        &Shape::Seq(&[Shape::Opt(&STR), Shape::Opt(&Shape::Uuid)]),
    ),
    Shape::Array(&Shape::Seq(&[Shape::Str, Shape::Str, Shape::Opt(&STR)])),
    Shape::Opt(&STR),
    Shape::Opt(&STR),
    Shape::Opt(&STR),
    Shape::Opt(&VAR_INT),
]);

static LODESTONE_TRACKER: Shape = Shape::Seq(&[
    Shape::Opt(&Shape::Seq(&[Shape::Str, Shape::BlockPos])),
    Shape::Bool,
]);

static BEES: Shape = Shape::Array(&Shape::Seq(&[
    Shape::VarInt,
    Shape::Nbt,
    Shape::VarInt,
    Shape::VarInt,
]));

static BLOCK_STATE: Shape = Shape::Array(&Shape::Seq(&[Shape::Str, Shape::Str]));

static TOOLTIP_DISPLAY: Shape = Shape::Seq(&[Shape::Bool, Shape::Array(&VAR_INT)]);

static USE_EFFECTS: Shape = Shape::Seq(&[Shape::Bool, Shape::Bool, Shape::F32]);

static USE_COOLDOWN: Shape = Shape::Seq(&[Shape::F32, Shape::Opt(&STR)]);

static ATTACK_RANGE: Shape = Shape::Seq(&[
    Shape::F32,
    Shape::F32,
    Shape::F32,
    Shape::F32,
    Shape::F32,
    Shape::F32,
]);

static FOOD: Shape = Shape::Seq(&[Shape::VarInt, Shape::F32, Shape::Bool]);

static WEAPON: Shape = Shape::Seq(&[Shape::VarInt, Shape::F32]);

static ENTITY_DATA: Shape = Shape::Seq(&[Shape::VarInt, Shape::Nbt]);

static TRIM: Shape = Shape::Seq(&[Shape::VarInt, Shape::VarInt]);

static SWING_ANIMATION: Shape = Shape::Seq(&[Shape::VarInt, Shape::VarInt]);

static CONTAINER: Shape = Shape::Array(&Shape::Opt(&TEMPLATE));

static LORE: Shape = Shape::Array(&NBT);

/// The payload shape of every 26.3 component.
#[must_use]
pub const fn shape(component: DataComponent) -> &'static Shape {
    use DataComponent as C;
    match component {
        C::MaxStackSize
        | C::MaxDamage
        | C::Damage
        | C::RepairCost
        | C::Rarity
        | C::DamageType
        | C::Enchantable
        | C::OminousBottleAmplifier
        | C::Instrument
        | C::ProvidesTrimMaterial
        | C::JukeboxPlayable
        | C::AdditionalTradeCost
        | C::Dye
        | C::MapPostProcessing
        | C::MapId
        | C::BaseColor
        | C::BreakSound => &VAR_INT,
        C::MinimumAttackCharge | C::PotionDurationScale => &F32,
        C::EnchantmentGlintOverride => &BOOL,
        C::DyedColor => &I32,
        C::ItemModel
        | C::ItemName
        | C::DamageResistant
        | C::TooltipStyle
        | C::NoteBlockSound
        | C::VillagerVariant
        | C::WolfVariant
        | C::WolfSoundVariant
        | C::WolfCollar
        | C::FoxVariant
        | C::SalmonSize
        | C::ParrotVariant
        | C::TropicalFishPattern
        | C::TropicalFishBaseColor
        | C::TropicalFishPatternColor
        | C::MooshroomVariant
        | C::RabbitVariant
        | C::PigVariant
        | C::PigSoundVariant
        | C::CowVariant
        | C::CowSoundVariant
        | C::ChickenVariant
        | C::ChickenSoundVariant
        | C::ZombieNautilusVariant
        | C::FrogVariant
        | C::HorseVariant
        | C::PaintingVariant
        | C::LlamaVariant
        | C::AxolotlVariant
        | C::CatVariant
        | C::CatSoundVariant
        | C::CatCollar
        | C::SheepColor
        | C::ShulkerColor => &STR,
        C::CustomName | C::CustomData | C::BucketEntityData | C::MapDecorations => &NBT,
        C::Unbreakable
        | C::CreativeSlotLock
        | C::IntangibleProjectile
        | C::Glider
        | C::DebugStickState
        | C::Recipes
        | C::Lock
        | C::ContainerLoot => &UNIT,
        C::Lore => &LORE,
        C::Enchantments | C::StoredEnchantments => &ENCHANTMENTS,
        C::SuspiciousStewEffects | C::BannerPatterns => &PAIR_VAR_INT,
        C::PotDecorations => &Shape::Array(&VAR_INT),
        C::CanPlaceOn | C::CanBreak => &BLOCK_PREDICATES,
        C::AttributeModifiers => &ATTRIBUTE_MODIFIERS,
        C::CustomModelData => &CUSTOM_MODEL_DATA,
        C::TooltipDisplay => &TOOLTIP_DISPLAY,
        C::UseEffects => &USE_EFFECTS,
        C::UseCooldown => &USE_COOLDOWN,
        C::Food => &FOOD,
        C::Consumable => &CONSUMABLE,
        C::UseRemainder | C::SulfurCubeContent => &TEMPLATE,
        C::Tool => &TOOL,
        C::Weapon => &WEAPON,
        C::AttackRange => &ATTACK_RANGE,
        C::Equippable => &EQUIPPABLE,
        C::Repairable | C::ProvidesBannerPatterns => &ID_SET,
        C::DeathProtection => &Shape::Array(&Shape::ConsumeEffect),
        C::BlocksAttacks => &BLOCKS_ATTACKS,
        C::PiercingWeapon => &PIERCING_WEAPON,
        C::KineticWeapon => &KINETIC_WEAPON,
        C::AttackAnimation | C::InteractAnimation => &SWING_ANIMATION,
        C::ChargedProjectiles | C::BundleContents => &Shape::Array(&TEMPLATE),
        C::PotionContents => &POTION_CONTENTS,
        C::WritableBookContent => &WRITABLE_BOOK,
        C::WrittenBookContent => &WRITTEN_BOOK,
        C::Trim => &TRIM,
        C::EntityData | C::BlockEntityData => &ENTITY_DATA,
        C::LodestoneTracker => &LODESTONE_TRACKER,
        C::FireworkExplosion => &FIREWORK_EXPLOSION,
        C::Fireworks => &FIREWORKS,
        C::Profile => &PROFILE,
        C::Container => &CONTAINER,
        C::BlockState => &BLOCK_STATE,
        C::Bees => &BEES,
        // `pumpkin_protocol`'s codec has no arm for these, so core can neither
        // write nor read one and no client below 26.3 has them.
        C::BlockTransformer
        | C::VillagerFood
        | C::MobVisibility
        | C::Compostable
        | C::CookingFuel
        | C::BrewingFuel
        | C::ProvidesPotteryPattern
        | C::SignTextFront
        | C::SignTextBack
        | C::Waxed
        | C::CushionColor => &Shape::Absent,
    }
}

const MAX_ENTRIES: i32 = 4096;

fn count(r: &mut &[u8]) -> Result<i32, ReadingError> {
    let count = r.get_var_int()?.0;
    if !(0..=MAX_ENTRIES).contains(&count) {
        return Err(ReadingError::Message("component list out of bounds".into()));
    }
    Ok(count)
}

/// Consumes one value of `shape`.
pub fn skip(shape: &Shape, r: &mut &[u8]) -> Result<(), ReadingError> {
    match shape {
        Shape::Unit => {}
        Shape::Bool => {
            r.get_bool()?;
        }
        Shape::I32 | Shape::F32 => {
            r.get_i32_be()?;
        }
        Shape::F64 => {
            r.get_f64_be()?;
        }
        Shape::VarInt => {
            r.get_var_int()?;
        }
        Shape::Str => {
            r.get_str()?;
        }
        Shape::Uuid => {
            r.get_uuid()?;
        }
        Shape::BlockPos => {
            r.get_i64_be()?;
        }
        Shape::Nbt => {
            r.get_nbt(&V::V_26_2)?;
        }
        Shape::Seq(parts) => {
            for part in *parts {
                skip(part, r)?;
            }
        }
        Shape::Array(inner) => {
            for _ in 0..count(r)? {
                skip(inner, r)?;
            }
        }
        Shape::Opt(inner) => {
            if r.get_bool()? {
                skip(inner, r)?;
            }
        }
        Shape::IdSet => {
            let tag = r.get_var_int()?.0;
            if tag == 0 {
                r.get_str()?;
            } else if tag > 0 {
                for _ in 0..tag - 1 {
                    r.get_var_int()?;
                }
            } else {
                return Err(ReadingError::Message("negative id set".into()));
            }
        }
        Shape::Sound => {
            if r.get_var_int()?.0 == 0 {
                r.get_str()?;
                if r.get_bool()? {
                    r.get_i32_be()?;
                }
            }
        }
        Shape::Template => {
            r.get_var_int()?;
            r.get_var_int()?;
            let to_add = count(r)?;
            let to_remove = count(r)?;
            for _ in 0..to_add {
                skip(&Shape::Component, r)?;
            }
            for _ in 0..to_remove {
                r.get_var_int()?;
            }
        }
        Shape::Component => {
            let id = r.get_var_int()?.0;
            skip(payload_shape(id)?, r)?;
        }
        Shape::StatusEffects => {
            for _ in 0..count(r)? {
                r.get_var_int()?;
                skip_effect_parameters(r)?;
            }
        }
        Shape::ConsumeEffect => {
            skip_consume_effect(r, true)?;
        }
        Shape::BoolSwitch(yes, no) => {
            let arm = if r.get_bool()? { yes } else { no };
            skip(arm, r)?;
        }
        Shape::Switch(arms, fallback) => {
            let tag = r.get_var_int()?.0;
            let arm = arms
                .iter()
                .find(|(value, _)| *value == tag)
                .map_or(*fallback, |(_, arm)| *arm);
            skip(arm, r)?;
        }
        Shape::Absent => {
            return Err(ReadingError::Message("component has no 26.3 shape".into()));
        }
    }
    Ok(())
}

const MAX_EFFECT_DEPTH: usize = 32;

pub(crate) fn skip_effect_parameters(r: &mut &[u8]) -> Result<(), ReadingError> {
    for _ in 0..MAX_EFFECT_DEPTH {
        r.get_var_int()?;
        r.get_var_int()?;
        r.get_bool()?;
        r.get_bool()?;
        r.get_bool()?;
        if !r.get_bool()? {
            return Ok(());
        }
    }
    Err(ReadingError::TooLarge("effect hidden depth".into()))
}

fn payload_shape(id: i32) -> Result<&'static Shape, ReadingError> {
    u8::try_from(id)
        .ok()
        .and_then(DataComponent::try_from_id)
        .map(shape)
        .ok_or_else(|| ReadingError::Message(format!("unknown component {id}")))
}

/// How many bytes component `id` takes in the 26.3 encoding.
pub fn payload_len(id: i32, bytes: &[u8]) -> Result<usize, ReadingError> {
    let mut cursor = bytes;
    skip(payload_shape(id)?, &mut cursor)?;
    Ok(bytes.len() - cursor.len())
}

/// Component size for a client-side layout. Pumpkin's 26.3 packet writer has
/// the newer `directional_particles` bool; clients before 26.3 do not.
pub fn payload_len_for_version(id: i32, bytes: &[u8], version: V) -> Result<usize, ReadingError> {
    let component = u8::try_from(id)
        .ok()
        .and_then(DataComponent::try_from_id)
        .ok_or_else(|| ReadingError::Message(format!("unknown component {id}")))?;
    if version >= V::V_26_3
        || !matches!(
            component,
            DataComponent::Consumable | DataComponent::DeathProtection
        )
    {
        return payload_len(id, bytes);
    }

    let mut cursor = bytes;
    if component == DataComponent::Consumable {
        skip(&Shape::F32, &mut cursor)?;
        skip(&Shape::VarInt, &mut cursor)?;
        skip(&Shape::Sound, &mut cursor)?;
        skip(&Shape::Bool, &mut cursor)?;
    }
    for _ in 0..count(&mut cursor)? {
        skip_consume_effect(&mut cursor, false)?;
    }
    Ok(bytes.len() - cursor.len())
}

fn skip_consume_effect(r: &mut &[u8], has_directional_particles: bool) -> Result<(), ReadingError> {
    // Core writes the effects before the probability. Its own reader has them
    // the other way round.
    match r.get_var_int()?.0 {
        0 => {
            skip(&Shape::StatusEffects, r)?;
            r.get_i32_be()?;
        }
        1 => skip(&ID_SET, r)?,
        2 => {}
        3 => {
            r.get_i32_be()?;
            if has_directional_particles {
                r.get_bool()?;
            }
        }
        4 => skip(&SOUND, r)?,
        other => {
            return Err(ReadingError::Message(format!(
                "unknown consume effect {other}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::ser::NetworkWriteExt;

    /// The smallest payload `shape` accepts: every count zero, every option
    /// absent, every switch on its first arm.
    fn sample(shape: &Shape, out: &mut Vec<u8>) {
        match shape {
            Shape::Unit | Shape::Absent => {}
            Shape::Bool => out.push(0),
            Shape::I32 | Shape::F32 => out.extend([0, 0, 0, 0]),
            Shape::F64 => out.extend([0; 8]),
            Shape::VarInt => out.push(0),
            Shape::Str => out.push(0),
            Shape::Uuid => out.extend([0; 16]),
            Shape::BlockPos => out.extend([0; 8]),
            Shape::Nbt => out.push(0),
            Shape::Seq(parts) => {
                for part in *parts {
                    sample(part, out);
                }
            }
            Shape::Array(_) | Shape::StatusEffects => out.push(0),
            Shape::Opt(_) => out.push(0),
            // A tag name, which is the arm with no registry ids in it.
            Shape::IdSet => out.extend([0, 0]),
            Shape::Sound => out.push(1),
            Shape::Template => out.extend([0, 0, 0, 0]),
            Shape::Component => {
                out.push(DataComponent::Damage.to_id());
                out.push(0);
            }
            Shape::ConsumeEffect => {
                out.push(2);
            }
            Shape::BoolSwitch(_, no) => {
                out.push(0);
                sample(no, out);
            }
            Shape::Switch(_, fallback) => {
                out.push(9);
                sample(fallback, out);
            }
        }
    }

    fn all() -> impl Iterator<Item = DataComponent> {
        (0..=u8::MAX).filter_map(DataComponent::try_from_id)
    }

    /// A component with no shape would make the whole stack unparseable, so
    /// the table has to be total over the enum.
    #[test]
    fn every_component_id_has_a_shape() {
        for component in all() {
            let bytes = {
                let mut out = Vec::new();
                sample(shape(component), &mut out);
                out
            };
            let id = i32::from(component.to_id());
            if matches!(shape(component), Shape::Absent) {
                assert!(payload_len(id, &bytes).is_err(), "{}", component.to_name());
                continue;
            }
            assert_eq!(
                payload_len(id, &bytes)
                    .unwrap_or_else(|error| panic!("{}: {error}", component.to_name())),
                bytes.len(),
                "{}",
                component.to_name()
            );
        }
    }

    /// The shapes have to agree with the reader they replace, so every sample
    /// is measured against `pumpkin_protocol`'s typed component codec too.
    #[test]
    fn the_shapes_agree_with_cores_own_reader() {
        let mut checked = 0;
        for component in all() {
            if matches!(shape(component), Shape::Absent) {
                continue;
            }
            let mut bytes = Vec::new();
            sample(shape(component), &mut bytes);
            let mut cursor = bytes.as_slice();
            let Ok(()) =
                pumpkin_protocol::codec::data_component::deserialize(component, &mut cursor)
                    .map(|_| ())
            else {
                // Core validates registry ids while reading; a zero id it
                // rejects says nothing about the shape.
                continue;
            };
            assert_eq!(
                bytes.len() - cursor.len(),
                bytes.len(),
                "{} left bytes behind",
                component.to_name()
            );
            checked += 1;
        }
        assert!(checked > 80, "only {checked} components could be compared");
    }

    /// `md('1.21.11').types.SlotComponent` gives `enchantments` as a varint
    /// counted list of id and level pairs.
    #[test]
    fn a_real_enchantment_list_is_measured() {
        let mut bytes = Vec::new();
        bytes.write_var_int(&VarInt(2)).unwrap();
        for (id, level) in [(12, 5), (33, 1)] {
            bytes.write_var_int(&VarInt(id)).unwrap();
            bytes.write_var_int(&VarInt(level)).unwrap();
        }
        bytes.extend([0xaa, 0xbb]);
        assert_eq!(
            payload_len(i32::from(DataComponent::Enchantments.to_id()), &bytes).unwrap(),
            5
        );
    }

    /// A trailing byte must not be swallowed: the length is what the shape
    /// consumed, not the whole slice.
    #[test]
    fn a_string_payload_stops_at_its_own_end() {
        let mut bytes = Vec::new();
        bytes.write_string("minecraft:stone").unwrap();
        let len = bytes.len();
        bytes.push(0x7f);
        assert_eq!(
            payload_len(i32::from(DataComponent::ItemModel.to_id()), &bytes).unwrap(),
            len
        );
    }

    #[test]
    fn consume_effect_length_depends_on_the_client_version() {
        let old_consumable = [
            0x3f, 0x80, 0x00, 0x00, // consume seconds
            0x00, // animation
            0x01, // sound holder id
            0x00, // consume particles
            0x01, // one effect
            0x03, // teleport randomly
            0x41, 0x80, 0x00, 0x00, // diameter
        ];
        let mut new_consumable = old_consumable.to_vec();
        new_consumable.push(0);
        let id = i32::from(DataComponent::Consumable.to_id());
        assert_eq!(
            payload_len_for_version(id, &old_consumable, V::V_26_2).unwrap(),
            old_consumable.len()
        );
        assert_eq!(
            payload_len(id, &new_consumable).unwrap(),
            new_consumable.len()
        );

        let old_death_protection = [
            0x01, // one effect
            0x03, // teleport randomly
            0x41, 0x80, 0x00, 0x00, // diameter
        ];
        let mut new_death_protection = old_death_protection.to_vec();
        new_death_protection.push(1);
        let id = i32::from(DataComponent::DeathProtection.to_id());
        assert_eq!(
            payload_len_for_version(id, &old_death_protection, V::V_26_2).unwrap(),
            old_death_protection.len()
        );
        assert_eq!(
            payload_len(id, &new_death_protection).unwrap(),
            new_death_protection.len()
        );
    }
}
