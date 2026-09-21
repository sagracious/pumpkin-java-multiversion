use pumpkin_data::particle::Particle as ParticleKind;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{ReadingError, WritingError};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::sound;
use crate::api::types::{BOOL, F32T, F64T, I8T, I32T, I64T, STRING, VAR_INT, WireType};
use crate::api::{ComposedMappings, IdMapping, PacketWrapper, TranslateError, UserConnection};

/// A particle as 26.3 wrote it: the registry id and its option data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub id: i32,
    pub data: ParticleData,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VibrationSource {
    /// The packed block position, copied as it stands.
    Block(i64),
    Entity {
        id: i32,
        y_offset: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParticleData {
    None,
    Block(u32),
    Dust {
        rgb: i32,
        scale: f32,
    },
    Transition {
        from: i32,
        to: i32,
        scale: f32,
    },
    Vibration {
        source: VibrationSource,
        ticks: i32,
    },
    Float(f32),
    Delay(i32),
    Color(i32),
    Trail {
        x: f64,
        y: f64,
        z: f64,
        color: i32,
        duration: i32,
    },
    Spell {
        color: i32,
        power: f32,
    },
    Geyser {
        water_blocks: i32,
        impulse: f32,
    },
}

/// The option data a version reads for one particle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    None,
    Block,
    Item,
    /// Packed rgb and a scale, from 1.21.2.
    DustRgb,
    /// Three colour floats and a scale, up to 1.21.
    DustFloats,
    /// Two packed rgbs and a scale, from 1.21.2.
    TransitionRgb,
    /// Both colours as floats with the scale last, 1.20.5 to 1.21.
    TransitionScaleLast,
    /// Both colours as floats with the scale between them, up to 1.20.3.
    TransitionScaleMid,
    /// A source position, the source type as a string and the arrival, 1.17 and 1.18.
    VibrationWithSource,
    /// The source type as a string, from 1.19.
    VibrationNamed,
    /// The source type as an id, from 1.20.3.
    VibrationTyped,
    Float,
    Delay,
    Color,
    Trail,
    TrailWithDuration,
    Spell,
    Geyser,
    GeyserBase,
}

/// What 26.3 writes. `ViaVersion`'s `ParticleType.Fillers.fill26_2`, which its
/// 26.2 to 26.3 protocol uses for both ends.
const SHAPES_26_2: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust_pillar", Shape::Block),
    ("falling_dust", Shape::Block),
    ("block_crumble", Shape::Block),
    ("dust", Shape::DustRgb),
    ("dust_color_transition", Shape::TransitionRgb),
    ("vibration", Shape::VibrationTyped),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
    ("entity_effect", Shape::Color),
    ("trail", Shape::TrailWithDuration),
    ("tinted_leaves", Shape::Color),
    ("dragon_breath", Shape::Float),
    ("effect", Shape::Spell),
    ("instant_effect", Shape::Spell),
    ("flash", Shape::Color),
    ("geyser", Shape::Geyser),
    ("geyser_base", Shape::GeyserBase),
    ("geyser_poof", Shape::GeyserBase),
    ("geyser_plume", Shape::Geyser),
];

const SHAPES_1_21_9: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust_pillar", Shape::Block),
    ("falling_dust", Shape::Block),
    ("block_crumble", Shape::Block),
    ("dust", Shape::DustRgb),
    ("dust_color_transition", Shape::TransitionRgb),
    ("vibration", Shape::VibrationTyped),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
    ("entity_effect", Shape::Color),
    ("trail", Shape::TrailWithDuration),
    ("tinted_leaves", Shape::Color),
    ("dragon_breath", Shape::Float),
    ("effect", Shape::Spell),
    ("instant_effect", Shape::Spell),
    ("flash", Shape::Color),
];

const SHAPES_1_21_5: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust_pillar", Shape::Block),
    ("falling_dust", Shape::Block),
    ("block_crumble", Shape::Block),
    ("dust", Shape::DustRgb),
    ("dust_color_transition", Shape::TransitionRgb),
    ("vibration", Shape::VibrationTyped),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
    ("entity_effect", Shape::Color),
    ("trail", Shape::TrailWithDuration),
    ("tinted_leaves", Shape::Color),
];

const SHAPES_1_21_4: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust_pillar", Shape::Block),
    ("falling_dust", Shape::Block),
    ("block_crumble", Shape::Block),
    ("dust", Shape::DustRgb),
    ("dust_color_transition", Shape::TransitionRgb),
    ("vibration", Shape::VibrationTyped),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
    ("entity_effect", Shape::Color),
    ("trail", Shape::TrailWithDuration),
];

const SHAPES_1_21_2: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust_pillar", Shape::Block),
    ("falling_dust", Shape::Block),
    ("block_crumble", Shape::Block),
    ("dust", Shape::DustRgb),
    ("dust_color_transition", Shape::TransitionRgb),
    ("vibration", Shape::VibrationTyped),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
    ("entity_effect", Shape::Color),
    ("trail", Shape::Trail),
];

const SHAPES_1_20_5: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust", Shape::DustFloats),
    ("falling_dust", Shape::Block),
    ("dust_color_transition", Shape::TransitionScaleLast),
    ("vibration", Shape::VibrationTyped),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
    ("dust_pillar", Shape::Block),
    ("entity_effect", Shape::Color),
];

const SHAPES_1_20_3: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust", Shape::DustFloats),
    ("falling_dust", Shape::Block),
    ("dust_color_transition", Shape::TransitionScaleMid),
    ("vibration", Shape::VibrationTyped),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
];

const SHAPES_1_19: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("block_marker", Shape::Block),
    ("dust", Shape::DustFloats),
    ("falling_dust", Shape::Block),
    ("dust_color_transition", Shape::TransitionScaleMid),
    ("vibration", Shape::VibrationNamed),
    ("sculk_charge", Shape::Float),
    ("shriek", Shape::Delay),
];

const SHAPES_1_18: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("dust", Shape::DustFloats),
    ("falling_dust", Shape::Block),
    ("dust_color_transition", Shape::TransitionScaleMid),
    ("vibration", Shape::VibrationWithSource),
    ("block_marker", Shape::Block),
];

const SHAPES_1_17: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("dust", Shape::DustFloats),
    ("falling_dust", Shape::Block),
    ("dust_color_transition", Shape::TransitionScaleMid),
    ("vibration", Shape::VibrationWithSource),
];

const SHAPES_1_16: &[(&str, Shape)] = &[
    ("item", Shape::Item),
    ("block", Shape::Block),
    ("dust", Shape::DustFloats),
    ("falling_dust", Shape::Block),
];

fn shapes(version: JavaMinecraftVersion) -> &'static [(&'static str, Shape)] {
    use JavaMinecraftVersion as V;
    match version {
        v if v >= V::V_26_2 => SHAPES_26_2,
        v if v >= V::V_1_21_9 => SHAPES_1_21_9,
        v if v >= V::V_1_21_5 => SHAPES_1_21_5,
        v if v >= V::V_1_21_4 => SHAPES_1_21_4,
        v if v >= V::V_1_21_2 => SHAPES_1_21_2,
        v if v >= V::V_1_20_5 => SHAPES_1_20_5,
        v if v >= V::V_1_20_3 => SHAPES_1_20_3,
        v if v >= V::V_1_19 => SHAPES_1_19,
        v if v >= V::V_1_18 => SHAPES_1_18,
        v if v >= V::V_1_17 => SHAPES_1_17,
        _ => SHAPES_1_16,
    }
}

fn shape_of(id: i32, version: JavaMinecraftVersion) -> Shape {
    let Some(name) = u16::try_from(id)
        .ok()
        .and_then(ParticleKind::from_id)
        .map(|kind| kind.to_name())
    else {
        return Shape::None;
    };
    shapes(version)
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map_or(Shape::None, |(_, shape)| *shape)
}

/// The shape the client reads for `mapped`, found by renumbering the names it
/// carries data for: `ViaVersion` stands a missing particle in with one that
/// takes the same arguments, so the mapped id decides the shape.
fn mapped_shape(mapped: u32, layout: JavaMinecraftVersion, ids: &ComposedMappings) -> Shape {
    for (name, shape) in shapes(layout) {
        let Some(kind) = ParticleKind::from_name(name) else {
            continue;
        };
        if ids.particles.map(u32::from(kind.to_id())) == Some(mapped) {
            return *shape;
        }
    }
    Shape::None
}

fn read_data(r: &mut &[u8], shape: Shape) -> Result<ParticleData, TranslateError> {
    Ok(match shape {
        Shape::None => ParticleData::None,
        // The item agent's `rewrite_item` fills this in; until then the packet
        // goes rather than a 26.3 stack.
        Shape::Item => return Err(TranslateError::Unsupported("item particle")),
        Shape::Block => ParticleData::Block(u32::try_from(VAR_INT.read(r)?.0).unwrap_or(0)),
        Shape::DustRgb => ParticleData::Dust {
            rgb: I32T.read(r)?,
            scale: F32T.read(r)?,
        },
        Shape::TransitionRgb => ParticleData::Transition {
            from: I32T.read(r)?,
            to: I32T.read(r)?,
            scale: F32T.read(r)?,
        },
        Shape::VibrationTyped => {
            let source = if VAR_INT.read(r)?.0 == 0 {
                VibrationSource::Block(I64T.read(r)?)
            } else {
                VibrationSource::Entity {
                    id: VAR_INT.read(r)?.0,
                    y_offset: F32T.read(r)?,
                }
            };
            ParticleData::Vibration {
                source,
                ticks: VAR_INT.read(r)?.0,
            }
        }
        Shape::Float => ParticleData::Float(F32T.read(r)?),
        Shape::Delay => ParticleData::Delay(VAR_INT.read(r)?.0),
        Shape::Color => ParticleData::Color(I32T.read(r)?),
        Shape::TrailWithDuration => ParticleData::Trail {
            x: F64T.read(r)?,
            y: F64T.read(r)?,
            z: F64T.read(r)?,
            color: I32T.read(r)?,
            duration: VAR_INT.read(r)?.0,
        },
        Shape::Spell => ParticleData::Spell {
            color: I32T.read(r)?,
            power: F32T.read(r)?,
        },
        Shape::Geyser => ParticleData::Geyser {
            water_blocks: I32T.read(r)?,
            impulse: 0.0,
        },
        Shape::GeyserBase => ParticleData::Geyser {
            water_blocks: I32T.read(r)?,
            impulse: F32T.read(r)?,
        },
        // Shapes no version at or above 26.2 writes.
        Shape::DustFloats
        | Shape::TransitionScaleLast
        | Shape::TransitionScaleMid
        | Shape::VibrationWithSource
        | Shape::VibrationNamed
        | Shape::Trail => return Err(TranslateError::Unsupported("particle option data")),
    })
}

fn rgb_floats(rgb: i32, out: &mut Vec<u8>) -> Result<(), TranslateError> {
    for shift in [16, 8, 0] {
        F32T.write(out, &(f32::from((rgb >> shift) as u8) / 255.0))?;
    }
    Ok(())
}

/// `false` when the value cannot be said in the shape the client reads, which
/// leaves the particle out rather than writing bytes it cannot parse.
fn write_data(
    out: &mut Vec<u8>,
    data: &ParticleData,
    shape: Shape,
    blockstates: &IdMapping,
) -> Result<bool, TranslateError> {
    match (shape, data) {
        (Shape::None, _) => {}
        (Shape::Block, ParticleData::Block(state)) => {
            let state = blockstates.map(*state).unwrap_or(0);
            VAR_INT.write(out, &VarInt(i32::try_from(state).unwrap_or(0)))?;
        }
        (Shape::DustRgb, ParticleData::Dust { rgb, scale }) => {
            I32T.write(out, rgb)?;
            F32T.write(out, scale)?;
        }
        (Shape::DustFloats, ParticleData::Dust { rgb, scale }) => {
            rgb_floats(*rgb, out)?;
            F32T.write(out, scale)?;
        }
        (Shape::TransitionRgb, ParticleData::Transition { from, to, scale }) => {
            I32T.write(out, from)?;
            I32T.write(out, to)?;
            F32T.write(out, scale)?;
        }
        (Shape::TransitionScaleLast, ParticleData::Transition { from, to, scale }) => {
            rgb_floats(*from, out)?;
            rgb_floats(*to, out)?;
            F32T.write(out, scale)?;
        }
        (Shape::TransitionScaleMid, ParticleData::Transition { from, to, scale }) => {
            rgb_floats(*from, out)?;
            F32T.write(out, scale)?;
            rgb_floats(*to, out)?;
        }
        (Shape::VibrationTyped, ParticleData::Vibration { source, ticks }) => {
            match source {
                VibrationSource::Block(position) => {
                    VAR_INT.write(out, &VarInt(0))?;
                    I64T.write(out, position)?;
                }
                VibrationSource::Entity { id, y_offset } => {
                    VAR_INT.write(out, &VarInt(1))?;
                    VAR_INT.write(out, &VarInt(*id))?;
                    F32T.write(out, y_offset)?;
                }
            }
            VAR_INT.write(out, &VarInt(*ticks))?;
        }
        (Shape::VibrationNamed, ParticleData::Vibration { source, ticks }) => {
            match source {
                VibrationSource::Block(position) => {
                    STRING.write(out, &"minecraft:block".into())?;
                    I64T.write(out, position)?;
                }
                VibrationSource::Entity { id, y_offset } => {
                    STRING.write(out, &"minecraft:entity".into())?;
                    VAR_INT.write(out, &VarInt(*id))?;
                    F32T.write(out, y_offset)?;
                }
            }
            VAR_INT.write(out, &VarInt(*ticks))?;
        }
        // 1.17 and 1.18 want the position the vibration started from, which
        // the packet no longer carries.
        (Shape::VibrationWithSource, _) => return Ok(false),
        (Shape::Float, ParticleData::Float(value)) => F32T.write(out, value)?,
        (Shape::Delay, ParticleData::Delay(value)) => VAR_INT.write(out, &VarInt(*value))?,
        (Shape::Color, ParticleData::Color(value)) => I32T.write(out, value)?,
        (Shape::Trail, ParticleData::Trail { x, y, z, color, .. }) => {
            F64T.write(out, x)?;
            F64T.write(out, y)?;
            F64T.write(out, z)?;
            I32T.write(out, color)?;
        }
        (
            Shape::TrailWithDuration,
            ParticleData::Trail {
                x,
                y,
                z,
                color,
                duration,
            },
        ) => {
            F64T.write(out, x)?;
            F64T.write(out, y)?;
            F64T.write(out, z)?;
            I32T.write(out, color)?;
            VAR_INT.write(out, &VarInt(*duration))?;
        }
        (Shape::Spell, ParticleData::Spell { color, power }) => {
            I32T.write(out, color)?;
            F32T.write(out, power)?;
        }
        (Shape::Geyser, ParticleData::Geyser { water_blocks, .. }) => {
            I32T.write(out, water_blocks)?;
        }
        (
            Shape::GeyserBase,
            ParticleData::Geyser {
                water_blocks,
                impulse,
            },
        ) => {
            I32T.write(out, water_blocks)?;
            F32T.write(out, impulse)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Reads the option data 26.3 writes for `id`.
pub fn read_particle_data(r: &mut &[u8], id: i32) -> Result<ParticleData, TranslateError> {
    read_data(r, shape_of(id, JavaMinecraftVersion::V_26_3))
}

pub fn read_particle(r: &mut &[u8]) -> Result<Particle, TranslateError> {
    let id = VAR_INT.read(r)?.0;
    let data = read_particle_data(r, id)?;
    Ok(Particle { id, data })
}

/// The id `particle` has on `layout`, `None` when the client has no stand in.
#[must_use]
pub fn mapped_id(particle: i32, ids: &ComposedMappings) -> Option<i32> {
    u32::try_from(particle)
        .ok()
        .and_then(|id| ids.particles.map(id))
        .and_then(|id| i32::try_from(id).ok())
}

/// Writes `particle` in the client's numbering and option data shape.
/// `false` when the client cannot be told about it, and nothing is written.
pub fn write_particle(
    out: &mut Vec<u8>,
    particle: &Particle,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<bool, TranslateError> {
    let Some(mapped) = mapped_id(particle.id, ids) else {
        return Ok(false);
    };
    let shape = mapped_shape(u32::try_from(mapped).unwrap_or(0), layout, ids);
    let mut data = Vec::new();
    if !write_data(&mut data, &particle.data, shape, &ids.blockstates)? {
        return Ok(false);
    }
    VAR_INT.write(out, &VarInt(mapped))?;
    out.extend_from_slice(&data);
    Ok(true)
}

/// One particle as 26.3 writes it: the id and its option data.
#[derive(Clone, Copy, Debug)]
pub struct ParticleT;

impl WireType for ParticleT {
    type Value = Particle;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        read_particle(r).map_err(|error| match error {
            TranslateError::Read(error) => error,
            other => ReadingError::Message(other.to_string()),
        })
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        VAR_INT.write(w, &VarInt(v.id))?;
        let shape = shape_of(v.id, JavaMinecraftVersion::V_26_3);
        match write_data(w, &v.data, shape, &IdMapping::IDENTITY) {
            Ok(true) => Ok(()),
            Ok(false) => Err(WritingError::Message("particle option data".to_string())),
            Err(TranslateError::Write(error)) => Err(error),
            Err(other) => Err(WritingError::Message(other.to_string())),
        }
    }
}

pub const PARTICLE: ParticleT = ParticleT;

/// Copies one particle across, renumbering it and rewriting its option data.
/// `false` when the client has no particle to show it as.
pub fn rewrite_particle(
    wrapper: &mut PacketWrapper,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<bool, TranslateError> {
    let particle = wrapper.read(&PARTICLE)?;
    let mut out = Vec::new();
    if !write_particle(&mut out, &particle, layout, ids)? {
        return Ok(false);
    }
    wrapper.write_bytes(&out);
    Ok(true)
}

/// The particle moved to the front of `LEVEL_PARTICLES` in 26.3 and sat after
/// the count from 1.20.5; below that the id leads and the option data trails.
pub fn level_particles(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    use JavaMinecraftVersion as V;

    if layout >= V::V_26_3 {
        if !rewrite_particle(wrapper, layout, ids)? {
            wrapper.cancel();
            return Ok(());
        }
        wrapper.passthrough_all();
        return Ok(());
    }

    if layout >= V::V_1_20_5 {
        wrapper.passthrough(&BOOL)?;
        if layout >= V::V_1_21_4 {
            wrapper.passthrough(&BOOL)?;
        }
        for _ in 0..3 {
            wrapper.passthrough(&F64T)?;
        }
        for _ in 0..4 {
            wrapper.passthrough(&F32T)?;
        }
        wrapper.passthrough(&I32T)?;
        if !rewrite_particle(wrapper, layout, ids)? {
            wrapper.cancel();
        }
        return Ok(());
    }

    let id = if layout >= V::V_1_19 {
        wrapper.read(&VAR_INT)?.0
    } else {
        wrapper.read(&I32T)?
    };
    let Some(mapped) = mapped_id(id, ids) else {
        wrapper.cancel();
        return Ok(());
    };
    if layout >= V::V_1_19 {
        wrapper.write(&VAR_INT, &VarInt(mapped))?;
    } else {
        wrapper.write(&I32T, &mapped)?;
    }

    wrapper.passthrough(&BOOL)?;
    for _ in 0..3 {
        if layout >= V::V_1_15 {
            wrapper.passthrough(&F64T)?;
        } else {
            wrapper.passthrough(&F32T)?;
        }
    }
    for _ in 0..3 {
        wrapper.passthrough(&F32T)?;
    }

    let speed = wrapper.read(&F32T)?;
    let count = wrapper.read(&I32T)?;
    // The option data is the rest of the payload here.
    let mut cursor = wrapper.remaining();
    let data = read_particle_data(&mut cursor, id)?;
    if !cursor.is_empty() {
        return Err(TranslateError::TrailingBytes(cursor.len()));
    }
    wrapper.consume_remaining();
    let shape = mapped_shape(u32::try_from(mapped).unwrap_or(0), layout, ids);

    // 1.20.5 folded the potion colour into the particle's own data; below it
    // the client takes an unused speed as the colour.
    let entity_effect = u16::try_from(id).ok() == Some(ParticleKind::EntityEffect.to_id());
    let speed = match &data {
        ParticleData::Color(colour) if entity_effect && shape == Shape::None && speed == 0.0 => {
            *colour as f32
        }
        _ => speed,
    };

    wrapper.write(&F32T, &speed)?;
    wrapper.write(&I32T, &count)?;
    let mut out = Vec::new();
    if write_data(&mut out, &data, shape, &ids.blockstates)? {
        wrapper.write_bytes(&out);
    } else {
        wrapper.cancel();
    }
    Ok(())
}

/// The explosion carries its particles and its sound; core branches at 1.21.9,
/// 1.21.2, 1.20.5, 1.20.3, 1.19.3 and 1.17 (`java/client/play/explode.rs`).
pub fn explode(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    use JavaMinecraftVersion as V;

    if layout >= V::V_1_21_2 {
        for _ in 0..3 {
            wrapper.passthrough(&F64T)?;
        }
        if layout >= V::V_1_21_9 {
            wrapper.passthrough(&F32T)?;
            wrapper.passthrough(&I32T)?;
        }
        if wrapper.passthrough(&BOOL)? {
            for _ in 0..3 {
                wrapper.passthrough(&F64T)?;
            }
        }
        if !rewrite_particle(wrapper, layout, ids)? || !sound::rewrite_holder(wrapper, layout, ids)?
        {
            wrapper.cancel();
            return Ok(());
        }
        if layout >= V::V_1_21_9 {
            let block_particles = wrapper.passthrough(&VAR_INT)?.0;
            for _ in 0..block_particles {
                if !rewrite_particle(wrapper, layout, ids)? {
                    wrapper.cancel();
                    return Ok(());
                }
                wrapper.passthrough(&F32T)?;
                wrapper.passthrough(&F32T)?;
                wrapper.passthrough(&VAR_INT)?;
            }
        }
        wrapper.passthrough_all();
        return Ok(());
    }

    for _ in 0..3 {
        if layout >= V::V_1_19_3 {
            wrapper.passthrough(&F64T)?;
        } else {
            wrapper.passthrough(&F32T)?;
        }
    }
    wrapper.passthrough(&F32T)?;
    let blocks = if layout >= V::V_1_17 {
        wrapper.passthrough(&VAR_INT)?.0
    } else {
        wrapper.passthrough(&I32T)?
    };
    for _ in 0..blocks {
        for _ in 0..3 {
            wrapper.passthrough(&I8T)?;
        }
    }
    for _ in 0..3 {
        wrapper.passthrough(&F32T)?;
    }

    if layout >= V::V_1_20_3 {
        wrapper.passthrough(&VAR_INT)?;
        for _ in 0..2 {
            if !rewrite_particle(wrapper, layout, ids)? {
                wrapper.cancel();
                return Ok(());
            }
        }
        // Below 1.20.5 the sound is a name, not an id.
        if layout >= V::V_1_20_5 && !sound::rewrite_holder(wrapper, layout, ids)? {
            wrapper.cancel();
            return Ok(());
        }
    }
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::packet::mappings::clientbound::play::{EXPLODE, LEVEL_PARTICLES};
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

    fn mapped(id: i32, version: JavaMinecraftVersion) -> i32 {
        i32::try_from(
            MappingData::get()
                .composed(version)
                .particles
                .map(u32::try_from(id).unwrap())
                .unwrap(),
        )
        .unwrap()
    }

    /// The `LEVEL_PARTICLES` core writes: the id leads below 1.20.5 and
    /// follows the count from it, with the option data always at the end.
    fn particles_payload(id: i32, data: &[u8], version: JavaMinecraftVersion) -> Vec<u8> {
        let mut out = Vec::new();
        if version < JavaMinecraftVersion::V_1_20_5 {
            if version >= JavaMinecraftVersion::V_1_19 {
                out.write_var_int(&VarInt(id)).unwrap();
            } else {
                out.write_i32_be(id).unwrap();
            }
        }
        out.write_bool(true).unwrap();
        if version >= JavaMinecraftVersion::V_1_21_4 {
            out.write_bool(false).unwrap();
        }
        for coordinate in [1.0f64, 2.0, 3.0] {
            out.write_f64_be(coordinate).unwrap();
        }
        for offset in [0.1f32, 0.2, 0.3, 0.5] {
            out.write_f32_be(offset).unwrap();
        }
        out.write_i32_be(4).unwrap();
        if version >= JavaMinecraftVersion::V_1_20_5 {
            out.write_var_int(&VarInt(id)).unwrap();
        }
        out.extend_from_slice(data);
        out
    }

    /// The seam entity metadata reads and writes through: a byte cursor in,
    /// a byte buffer out, and no packet to cancel.
    #[test]
    fn the_slice_seam_reports_what_it_cannot_say() {
        let version = JavaMinecraftVersion::V_1_18_2;
        let ids = MappingData::get().composed(version);

        let mut payload = Vec::new();
        payload
            .write_var_int(&VarInt(i32::from(ParticleKind::Block.to_id())))
            .unwrap();
        payload.write_var_int(&VarInt(1)).unwrap();
        let mut cursor = payload.as_slice();
        let block = read_particle(&mut cursor).unwrap();
        assert!(cursor.is_empty());

        let mut out = Vec::new();
        assert!(write_particle(&mut out, &block, version, ids).unwrap());
        let mut expected = Vec::new();
        expected
            .write_var_int(&VarInt(mapped(
                i32::from(ParticleKind::Block.to_id()),
                version,
            )))
            .unwrap();
        expected
            .write_var_int(&VarInt(
                i32::try_from(ids.blockstates.map(1).unwrap()).unwrap(),
            ))
            .unwrap();
        assert_eq!(out, expected);

        // 1.18 wants the position a vibration started from, which the packet
        // no longer carries, and the geysers are 26.2's.
        let vibration = Particle {
            id: i32::from(ParticleKind::Vibration.to_id()),
            data: ParticleData::Vibration {
                source: VibrationSource::Block(0),
                ticks: 20,
            },
        };
        let geyser = Particle {
            id: i32::from(ParticleKind::GeyserBase.to_id()),
            data: ParticleData::Geyser {
                water_blocks: 1,
                impulse: 0.0,
            },
        };
        let mut out = Vec::new();
        assert!(!write_particle(&mut out, &vibration, version, ids).unwrap());
        assert!(!write_particle(&mut out, &geyser, version, ids).unwrap());
        assert!(out.is_empty());
    }

    /// Flame takes no option data on any version, so only its id moves.
    #[test]
    fn a_plain_particle_is_renumbered_in_every_layout() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_16_2,
        ] {
            let flame = i32::from(ParticleKind::Flame.to_id());
            let out = run(
                level_particles,
                &LEVEL_PARTICLES,
                &particles_payload(flame, &[], version),
                version,
            )
            .unwrap();
            assert_eq!(
                out,
                particles_payload(mapped(flame, version), &[], version),
                "{version}"
            );
        }
    }

    /// 1.21.2 packed the dust colour into one int; below it the client reads
    /// three floats, which is `ParticleRewriter1_21_2.argbToVector`.
    #[test]
    fn the_dust_colour_is_unpacked_below_1_21_2() {
        for version in [JavaMinecraftVersion::V_1_21, JavaMinecraftVersion::V_1_16_2] {
            let dust = i32::from(ParticleKind::Dust.to_id());
            let mut data = Vec::new();
            data.write_i32_be(0x00ff_8000).unwrap();
            data.write_f32_be(1.5).unwrap();

            let mut expected = Vec::new();
            for channel in [1.0f32, 128.0 / 255.0, 0.0] {
                expected.write_f32_be(channel).unwrap();
            }
            expected.write_f32_be(1.5).unwrap();

            let out = run(
                level_particles,
                &LEVEL_PARTICLES,
                &particles_payload(dust, &data, version),
                version,
            )
            .unwrap();
            assert_eq!(
                out,
                particles_payload(mapped(dust, version), &expected, version),
                "{version}"
            );
        }
    }

    /// 1.20.5 gave `entity_effect` its colour; below it the client takes an
    /// unused speed as the colour instead.
    #[test]
    fn the_potion_colour_moves_into_the_speed_below_1_20_5() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let effect = i32::from(ParticleKind::EntityEffect.to_id());
        let speed = 1 + 1 + 24 + 12;

        let mut payload = particles_payload(effect, &[], version);
        payload[speed..speed + 4].copy_from_slice(&0f32.to_be_bytes());
        payload.write_i32_be(64).unwrap();

        let mut expected = particles_payload(mapped(effect, version), &[], version);
        expected[speed..speed + 4].copy_from_slice(&64f32.to_be_bytes());
        assert_eq!(
            run(level_particles, &LEVEL_PARTICLES, &payload, version).unwrap(),
            expected
        );
    }

    /// The geyser particles are 26.2's and have no stand in below it.
    #[test]
    fn a_particle_the_client_lacks_drops_the_packet() {
        let version = JavaMinecraftVersion::V_1_21;
        let geyser = i32::from(ParticleKind::GeyserBase.to_id());
        assert!(
            MappingData::get()
                .composed(version)
                .particles
                .map(u32::try_from(geyser).unwrap())
                .is_none()
        );
        let mut data = Vec::new();
        data.write_i32_be(3).unwrap();
        data.write_f32_be(1.0).unwrap();
        assert!(
            run(
                level_particles,
                &LEVEL_PARTICLES,
                &particles_payload(geyser, &data, version),
                version,
            )
            .is_none()
        );
    }

    /// The explosion core writes: a knockback option, one particle and the
    /// sound holder from 1.21.2, two particles behind the block list from
    /// 1.20.3, and neither below it.
    fn explode_payload(
        particle: i32,
        small: i32,
        sound: i32,
        version: JavaMinecraftVersion,
    ) -> Vec<u8> {
        use JavaMinecraftVersion as V;
        let mut out = Vec::new();
        if version >= V::V_1_21_2 {
            for coordinate in [1.0f64, 2.0, 3.0] {
                out.write_f64_be(coordinate).unwrap();
            }
            if version >= V::V_1_21_9 {
                out.write_f32_be(4.0).unwrap();
                out.write_i32_be(0).unwrap();
            }
            out.write_bool(false).unwrap();
            out.write_var_int(&VarInt(particle)).unwrap();
            out.write_var_int(&VarInt(sound + 1)).unwrap();
            if version >= V::V_1_21_9 {
                out.write_var_int(&VarInt(0)).unwrap();
            }
            return out;
        }

        for coordinate in [1.0f32, 2.0, 3.0] {
            if version >= V::V_1_19_3 {
                out.write_f64_be(f64::from(coordinate)).unwrap();
            } else {
                out.write_f32_be(coordinate).unwrap();
            }
        }
        out.write_f32_be(4.0).unwrap();
        if version >= V::V_1_17 {
            out.write_var_int(&VarInt(0)).unwrap();
        } else {
            out.write_i32_be(0).unwrap();
        }
        for knockback in [0.0f32; 3] {
            out.write_f32_be(knockback).unwrap();
        }
        if version >= V::V_1_20_3 {
            out.write_var_int(&VarInt(1)).unwrap();
            out.write_var_int(&VarInt(small)).unwrap();
            out.write_var_int(&VarInt(particle)).unwrap();
            out.write_var_int(&VarInt(sound + 1)).unwrap();
        }
        out
    }

    #[test]
    fn the_explosion_particle_and_sound_are_renumbered() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21_4,
            JavaMinecraftVersion::V_1_20_5,
        ] {
            let ids = MappingData::get().composed(version);
            let emitter = i32::from(ParticleKind::ExplosionEmitter.to_id());
            let small = i32::from(ParticleKind::Explosion.to_id());
            let out = run(
                explode,
                &EXPLODE,
                &explode_payload(emitter, small, 700, version),
                version,
            )
            .unwrap();
            let sound = i32::try_from(ids.sounds.map(700).unwrap()).unwrap();
            assert_eq!(
                out,
                explode_payload(
                    mapped(emitter, version),
                    mapped(small, version),
                    sound,
                    version
                ),
                "{version}"
            );
        }
    }

    /// The 1.16.2 explosion ends after the knockback, with no particle or
    /// sound to renumber.
    #[test]
    fn the_oldest_explosion_layout_is_copied() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let payload = explode_payload(0, 0, 0, version);
        assert_eq!(run(explode, &EXPLODE, &payload, version).unwrap(), payload);
    }

    /// Sound 108 is one of the 35 the 1.20.5 registry has no stand in for,
    /// and the explosion goes with it.
    #[test]
    fn a_sound_the_client_lacks_drops_the_explosion() {
        let version = JavaMinecraftVersion::V_1_20_5;
        assert!(
            MappingData::get()
                .composed(version)
                .sounds
                .map(108)
                .is_none()
        );
        let emitter = i32::from(ParticleKind::ExplosionEmitter.to_id());
        let small = i32::from(ParticleKind::Explosion.to_id());
        assert!(
            run(
                explode,
                &EXPLODE,
                &explode_payload(emitter, small, 108, version),
                version,
            )
            .is_none()
        );
    }
}
