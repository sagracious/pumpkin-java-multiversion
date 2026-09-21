use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::bit_set::BitSet;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::codec::var_long::VarLong;
use pumpkin_protocol::ser::{
    NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt, ReadingError, WritingError,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion;
use uuid::Uuid;

pub trait WireType {
    type Value;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError>;
    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError>;
}

macro_rules! scalar {
    ($name:ident, $value:ty, $get:ident, $put:ident, $konst:ident) => {
        #[derive(Clone, Copy, Debug)]
        pub struct $name;

        impl WireType for $name {
            type Value = $value;

            fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
                r.$get()
            }

            fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
                w.$put(*v)
            }
        }

        pub const $konst: $name = $name;
    };
}

scalar!(BoolT, bool, get_bool, write_bool, BOOL);
scalar!(U8T, u8, get_u8, write_u8, U8);
scalar!(I8T, i8, get_i8, write_i8, I8);
scalar!(I16T, i16, get_i16_be, write_i16_be, I16);
scalar!(I32T, i32, get_i32_be, write_i32_be, I32);
scalar!(I64T, i64, get_i64_be, write_i64_be, I64);
scalar!(F32T, f32, get_f32_be, write_f32_be, F32);
scalar!(F64T, f64, get_f64_be, write_f64_be, F64);

#[derive(Clone, Copy, Debug)]
pub struct VarIntT;

impl WireType for VarIntT {
    type Value = VarInt;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        r.get_var_int()
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_var_int(v)
    }
}

pub const VAR_INT: VarIntT = VarIntT;

#[derive(Clone, Copy, Debug)]
pub struct VarLongT;

impl WireType for VarLongT {
    type Value = VarLong;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        r.get_var_long()
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_var_long(v)
    }
}

pub const VAR_LONG: VarLongT = VarLongT;

#[derive(Clone, Copy, Debug)]
pub struct StringT;

impl WireType for StringT {
    type Value = Box<str>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        r.get_str()
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_string(v)
    }
}

pub const STRING: StringT = StringT;

#[derive(Clone, Copy, Debug)]
pub struct UuidT;

impl WireType for UuidT {
    type Value = Uuid;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        r.get_uuid()
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_uuid(v)
    }
}

pub const UUID: UuidT = UuidT;

/// `BlockPos` has packed the same way since 1.14, well above the supported floor.
#[derive(Clone, Copy, Debug)]
pub struct BlockPosT;

impl WireType for BlockPosT {
    type Value = BlockPos;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        r.get_block_pos(&JavaMinecraftVersion::V_26_3)
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_block_pos(v, &JavaMinecraftVersion::V_26_3)
    }
}

pub const BLOCK_POS: BlockPosT = BlockPosT;

#[derive(Clone, Copy, Debug)]
pub struct ByteArrayT;

impl WireType for ByteArrayT {
    type Value = Vec<u8>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        let len = usize::try_from(r.get_var_int()?.0)
            .map_err(|_| ReadingError::Message("negative byte array length".into()))?;
        Ok(r.read_slice_borrowed(len)?.to_vec())
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_var_int(&VarInt(i32::try_from(v.len()).map_err(|_| {
            WritingError::Message("byte array too long".to_string())
        })?))?;
        w.write_slice(v)
    }
}

pub const BYTE_ARRAY: ByteArrayT = ByteArrayT;

/// A run of bytes whose length the layout fixes, such as a chat signature.
#[derive(Clone, Copy, Debug)]
pub struct FixedBytesT(pub usize);

impl WireType for FixedBytesT {
    type Value = Vec<u8>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        Ok(r.read_slice_borrowed(self.0)?.to_vec())
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        if v.len() != self.0 {
            return Err(WritingError::Message(format!(
                "expected {} bytes, got {}",
                self.0,
                v.len()
            )));
        }
        w.write_slice(v)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RemainingBytesT;

impl WireType for RemainingBytesT {
    type Value = Vec<u8>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        Ok(std::mem::take(r).to_vec())
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_slice(v)
    }
}

pub const REMAINING_BYTES: RemainingBytesT = RemainingBytesT;

#[derive(Clone, Copy, Debug)]
pub struct BitSetT;

impl WireType for BitSetT {
    type Value = BitSet;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        BitSet::decode(r)
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_bitset(v)
    }
}

pub const BIT_SET: BitSetT = BitSetT;

#[derive(Clone, Copy, Debug)]
pub struct OptionalT<T>(pub T);

impl<T: WireType> WireType for OptionalT<T> {
    type Value = Option<T::Value>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        if r.get_bool()? {
            Ok(Some(self.0.read(r)?))
        } else {
            Ok(None)
        }
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        match v {
            Some(value) => {
                w.write_bool(true)?;
                self.0.write(w, value)
            }
            None => w.write_bool(false),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ArrayT<T>(pub T);

impl<T: WireType> WireType for ArrayT<T> {
    type Value = Vec<T::Value>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        let len = usize::try_from(r.get_var_int()?.0)
            .map_err(|_| ReadingError::Message("negative array length".into()))?;
        if len > r.len() {
            return Err(ReadingError::Incomplete(format!("array of {len}")));
        }
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.0.read(r)?);
        }
        Ok(out)
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_var_int(&VarInt(
            i32::try_from(v.len())
                .map_err(|_| WritingError::Message("array too long".to_string()))?,
        ))?;
        for value in v {
            self.0.write(w, value)?;
        }
        Ok(())
    }
}

/// Named root below 1.20.2, unnamed from it.
#[derive(Clone, Copy, Debug)]
pub struct NbtT {
    version: JavaMinecraftVersion,
}

impl NbtT {
    #[must_use]
    pub const fn for_version(version: JavaMinecraftVersion) -> Self {
        Self { version }
    }
}

impl WireType for NbtT {
    type Value = Option<NbtTag>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        r.get_nbt_borrowed(&self.version)
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_nbt_with_version(v.as_ref(), &self.version)
    }
}

/// A JSON string below 1.20.3, network NBT from it.
#[derive(Clone, Copy, Debug)]
pub struct TextComponentT {
    version: JavaMinecraftVersion,
}

impl TextComponentT {
    #[must_use]
    pub const fn for_version(version: JavaMinecraftVersion) -> Self {
        Self { version }
    }
}

impl WireType for TextComponentT {
    type Value = TextComponent;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        r.get_component_borrowed(&self.version)
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        w.write_component(v, &self.version)
    }
}

/// One component of a stack, payload bytes as they arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemComponent {
    pub id: i32,
    pub data: Vec<u8>,
}

/// A stack in whichever form its layout carries, components still encoded.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Empty,
    Structured {
        count: i32,
        id: i32,
        added: Vec<ItemComponent>,
        removed: Vec<i32>,
    },
    Nbt {
        id: i32,
        count: i8,
        nbt: Option<NbtTag>,
    },
}

impl Item {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    #[must_use]
    pub const fn item_id(&self) -> Option<i32> {
        match self {
            Self::Empty => None,
            Self::Structured { id, .. } | Self::Nbt { id, .. } => Some(*id),
        }
    }
}

const MAX_COMPONENTS: i32 = 256;

/// How many bytes component `id` takes in the 26.3 encoding.
pub fn component_payload_len(id: i32, bytes: &[u8]) -> Result<usize, ReadingError> {
    crate::api::rewriter::item_shape::payload_len(id, bytes)
}

/// The structured form from 1.20.5, the NBT form below it.
///
/// `length_prefixed` is the form a client from 1.21.5 sends for an untrusted
/// stack, where every component payload carries its own length.
#[derive(Clone, Copy, Debug)]
pub struct ItemT {
    version: JavaMinecraftVersion,
    length_prefixed: bool,
}

impl ItemT {
    pub const FIRST_STRUCTURED: JavaMinecraftVersion = JavaMinecraftVersion::V_1_20_5;

    #[must_use]
    pub const fn for_version(version: JavaMinecraftVersion) -> Self {
        Self {
            version,
            length_prefixed: false,
        }
    }

    #[must_use]
    pub const fn length_prefixed(version: JavaMinecraftVersion) -> Self {
        Self {
            version,
            length_prefixed: true,
        }
    }

    #[must_use]
    pub const fn is_structured(&self) -> bool {
        self.version.protocol_version() >= Self::FIRST_STRUCTURED.protocol_version()
    }

    fn read_structured(&self, r: &mut &[u8]) -> Result<Item, ReadingError> {
        let count = r.get_var_int()?.0;
        if count == 0 {
            return Ok(Item::Empty);
        }
        let id = r.get_var_int()?.0;
        let to_add = r.get_var_int()?.0;
        let to_remove = r.get_var_int()?.0;
        if !(0..=MAX_COMPONENTS).contains(&to_add) || !(0..=MAX_COMPONENTS).contains(&to_remove) {
            return Err(ReadingError::Message(
                "component count out of bounds".into(),
            ));
        }

        let mut added = Vec::with_capacity(to_add as usize);
        for _ in 0..to_add {
            let id = r.get_var_int()?.0;
            let len = if self.length_prefixed {
                usize::try_from(r.get_var_int()?.0)
                    .map_err(|_| ReadingError::Message("negative component length".into()))?
            } else {
                component_payload_len(id, r)?
            };
            let data = r.read_slice_borrowed(len)?.to_vec();
            added.push(ItemComponent { id, data });
        }

        let mut removed = Vec::with_capacity(to_remove as usize);
        for _ in 0..to_remove {
            removed.push(r.get_var_int()?.0);
        }

        Ok(Item::Structured {
            count,
            id,
            added,
            removed,
        })
    }

    fn write_structured(&self, w: &mut Vec<u8>, v: &Item) -> Result<(), WritingError> {
        let Item::Structured {
            count,
            id,
            added,
            removed,
        } = v
        else {
            return w.write_var_int(&VarInt(0));
        };
        w.write_var_int(&VarInt(*count))?;
        w.write_var_int(&VarInt(*id))?;
        w.write_var_int(&VarInt(
            i32::try_from(added.len()).unwrap_or(MAX_COMPONENTS),
        ))?;
        w.write_var_int(&VarInt(
            i32::try_from(removed.len()).unwrap_or(MAX_COMPONENTS),
        ))?;
        for component in added {
            w.write_var_int(&VarInt(component.id))?;
            if self.length_prefixed {
                w.write_var_int(&VarInt(i32::try_from(component.data.len()).map_err(
                    |_| WritingError::Message("component too large".to_string()),
                )?))?;
            }
            w.write_slice(&component.data)?;
        }
        for id in removed {
            w.write_var_int(&VarInt(*id))?;
        }
        Ok(())
    }
}

impl WireType for ItemT {
    type Value = Item;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        if self.is_structured() {
            return self.read_structured(r);
        }
        if !r.get_bool()? {
            return Ok(Item::Empty);
        }
        let id = r.get_var_int()?.0;
        let count = r.get_i8()?;
        let nbt = r.get_nbt_borrowed(&self.version)?;
        Ok(Item::Nbt { id, count, nbt })
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        if self.is_structured() {
            return self.write_structured(w, v);
        }
        let Item::Nbt { id, count, nbt } = v else {
            return w.write_bool(false);
        };
        w.write_bool(true)?;
        w.write_var_int(&VarInt(*id))?;
        w.write_i8(*count)?;
        w.write_nbt_with_version(nbt.as_ref(), &self.version)
    }
}

/// The cost of a merchant offer from 1.20.5: never absent, no removals, and
/// the count sits after the item id.
#[derive(Clone, Copy, Debug)]
pub struct ItemCostT;

impl WireType for ItemCostT {
    type Value = Item;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        let id = r.get_var_int()?.0;
        let count = r.get_var_int()?.0;
        let to_add = r.get_var_int()?.0;
        if !(0..=MAX_COMPONENTS).contains(&to_add) {
            return Err(ReadingError::Message(
                "component count out of bounds".into(),
            ));
        }
        let mut added = Vec::with_capacity(to_add as usize);
        for _ in 0..to_add {
            let id = r.get_var_int()?.0;
            let len = component_payload_len(id, r)?;
            let data = r.read_slice_borrowed(len)?.to_vec();
            added.push(ItemComponent { id, data });
        }
        Ok(Item::Structured {
            count,
            id,
            added,
            removed: Vec::new(),
        })
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        let (count, id, added) = match v {
            Item::Structured {
                count, id, added, ..
            } => (*count, *id, added.as_slice()),
            _ => (0, 0, [].as_slice()),
        };
        w.write_var_int(&VarInt(id))?;
        w.write_var_int(&VarInt(count))?;
        w.write_var_int(&VarInt(
            i32::try_from(added.len()).unwrap_or(MAX_COMPONENTS),
        ))?;
        for component in added {
            w.write_var_int(&VarInt(component.id))?;
            w.write_slice(&component.data)?;
        }
        Ok(())
    }
}

pub const ITEM_COST: ItemCostT = ItemCostT;

/// A stack nested in a component, and the advancement icon from 26.1: the
/// item id leads the count and an empty stack cannot be expressed.
#[derive(Clone, Copy, Debug)]
pub struct TemplateItemT;

impl WireType for TemplateItemT {
    type Value = Item;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        let id = r.get_var_int()?.0;
        let count = r.get_var_int()?.0;
        let to_add = r.get_var_int()?.0;
        let to_remove = r.get_var_int()?.0;
        if !(0..=MAX_COMPONENTS).contains(&to_add) || !(0..=MAX_COMPONENTS).contains(&to_remove) {
            return Err(ReadingError::Message(
                "component count out of bounds".into(),
            ));
        }
        let mut added = Vec::with_capacity(to_add as usize);
        for _ in 0..to_add {
            let id = r.get_var_int()?.0;
            let len = component_payload_len(id, r)?;
            added.push(ItemComponent {
                id,
                data: r.read_slice_borrowed(len)?.to_vec(),
            });
        }
        let mut removed = Vec::with_capacity(to_remove as usize);
        for _ in 0..to_remove {
            removed.push(r.get_var_int()?.0);
        }
        Ok(Item::Structured {
            count,
            id,
            added,
            removed,
        })
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        let (count, id, added, removed) = match v {
            Item::Structured {
                count,
                id,
                added,
                removed,
            } => (*count, *id, added.as_slice(), removed.as_slice()),
            _ => (1, 0, [].as_slice(), [].as_slice()),
        };
        w.write_var_int(&VarInt(id))?;
        w.write_var_int(&VarInt(count))?;
        w.write_var_int(&VarInt(
            i32::try_from(added.len()).unwrap_or(MAX_COMPONENTS),
        ))?;
        w.write_var_int(&VarInt(
            i32::try_from(removed.len()).unwrap_or(MAX_COMPONENTS),
        ))?;
        for component in added {
            w.write_var_int(&VarInt(component.id))?;
            w.write_slice(&component.data)?;
        }
        for id in removed {
            w.write_var_int(&VarInt(*id))?;
        }
        Ok(())
    }
}

pub const TEMPLATE_ITEM: TemplateItemT = TemplateItemT;

/// The hash a client from 1.21.5 sends in place of a component payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashedItem {
    pub id: i32,
    pub count: i32,
    pub added: Vec<(i32, i32)>,
    pub removed: Vec<i32>,
}

/// The serverbound stack from 1.21.5: an optional id, count and component hashes.
#[derive(Clone, Copy, Debug)]
pub struct HashedItemT;

impl WireType for HashedItemT {
    type Value = Option<HashedItem>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        if !r.get_bool()? {
            return Ok(None);
        }
        let id = r.get_var_int()?.0;
        let count = r.get_var_int()?.0;
        let added_len = r.get_var_int()?.0;
        if !(0..=MAX_COMPONENTS).contains(&added_len) {
            return Err(ReadingError::Message("added_length out of bounds".into()));
        }
        let mut added = Vec::with_capacity(added_len as usize);
        for _ in 0..added_len {
            added.push((r.get_var_int()?.0, r.get_i32_be()?));
        }
        let removed_len = r.get_var_int()?.0;
        if !(0..=MAX_COMPONENTS).contains(&removed_len) {
            return Err(ReadingError::Message("removed_length out of bounds".into()));
        }
        let mut removed = Vec::with_capacity(removed_len as usize);
        for _ in 0..removed_len {
            removed.push(r.get_var_int()?.0);
        }
        Ok(Some(HashedItem {
            id,
            count,
            added,
            removed,
        }))
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        let Some(item) = v else {
            return w.write_bool(false);
        };
        w.write_bool(true)?;
        w.write_var_int(&VarInt(item.id))?;
        w.write_var_int(&VarInt(item.count))?;
        w.write_var_int(&VarInt(
            i32::try_from(item.added.len()).unwrap_or(MAX_COMPONENTS),
        ))?;
        for (id, hash) in &item.added {
            w.write_var_int(&VarInt(*id))?;
            w.write_i32_be(*hash)?;
        }
        w.write_var_int(&VarInt(
            i32::try_from(item.removed.len()).unwrap_or(MAX_COMPONENTS),
        ))?;
        for id in &item.removed {
            w.write_var_int(&VarInt(*id))?;
        }
        Ok(())
    }
}

pub const HASHED_ITEM: HashedItemT = HashedItemT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_and_containers_round_trip() {
        let mut buf = Vec::new();
        VAR_INT.write(&mut buf, &VarInt(300)).unwrap();
        STRING.write(&mut buf, &"hi".into()).unwrap();
        ArrayT(I16T).write(&mut buf, &vec![1i16, -2]).unwrap();
        OptionalT(BOOL).write(&mut buf, &Some(true)).unwrap();
        OptionalT(BOOL).write(&mut buf, &None).unwrap();

        let mut read: &[u8] = &buf;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 300);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "hi");
        assert_eq!(ArrayT(I16T).read(&mut read).unwrap(), vec![1i16, -2]);
        assert_eq!(OptionalT(BOOL).read(&mut read).unwrap(), Some(true));
        assert_eq!(OptionalT(BOOL).read(&mut read).unwrap(), None);
        assert!(read.is_empty());
    }

    #[test]
    fn an_array_longer_than_the_input_is_rejected() {
        let mut buf = Vec::new();
        buf.write_var_int(&VarInt(1000)).unwrap();
        let mut read: &[u8] = &buf;
        assert!(ArrayT(I64T).read(&mut read).is_err());
    }

    #[test]
    fn nbt_root_naming_follows_the_version() {
        let mut compound = pumpkin_nbt::compound::NbtCompound::new();
        compound.put_int("a", 1);
        let tag = Some(NbtTag::Compound(compound));

        let mut named = Vec::new();
        NbtT::for_version(JavaMinecraftVersion::V_1_20)
            .write(&mut named, &tag)
            .unwrap();
        let mut unnamed = Vec::new();
        NbtT::for_version(JavaMinecraftVersion::V_1_20_2)
            .write(&mut unnamed, &tag)
            .unwrap();
        assert_eq!(named.len(), unnamed.len() + 2);

        let mut read: &[u8] = &named;
        assert!(
            NbtT::for_version(JavaMinecraftVersion::V_1_20)
                .read(&mut read)
                .unwrap()
                .is_some()
        );
        assert!(read.is_empty());
    }
}
