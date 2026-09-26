use pumpkin_data::particle::Particle as ParticleKind;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{ReadingError, WritingError};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::item::{
    read_native_item_value, rewrite_item_value, rewrite_item_value_with_connection,
};
use crate::api::rewriter::sound;
use crate::api::types::{BOOL, F32T, F64T, I8T, I32T, I64T, STRING, VAR_INT, WireType};
use crate::api::{ComposedMappings, MappingData, PacketWrapper, TranslateError, UserConnection};

/// A particle as 26.3 wrote it: the registry id and its option data.
#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
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
        /// The source block position required by the 1.17/1.18 wire shape.
        /// Modern packets carry the particle's origin as its outer position.
        origin: Option<i64>,
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
    /// A native 26.3 item stack embedded in an item particle.
    Item(Vec<u8>),
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
    /// An origin position, a named target source and the arrival, 1.17 and 1.18.
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

fn read_data(
    r: &mut &[u8],
    shape: Shape,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<ParticleData, TranslateError> {
    Ok(match shape {
        Shape::None => ParticleData::None,
        Shape::Item => ParticleData::Item(
            read_native_item_value(r, layout, ids)
                .ok_or(TranslateError::Unsupported("particle item"))?,
        ),
        Shape::Block => {
            let state = VAR_INT.read(r)?.0;
            ParticleData::Block(
                u32::try_from(state)
                    .map_err(|_| TranslateError::Unsupported("particle block state id"))?,
            )
        }
        Shape::DustRgb => ParticleData::Dust {
            rgb: I32T.read(r)?,
            scale: F32T.read(r)?,
        },
        Shape::DustFloats => ParticleData::Dust {
            rgb: read_rgb_floats(r)?,
            scale: F32T.read(r)?,
        },
        Shape::TransitionRgb => ParticleData::Transition {
            from: I32T.read(r)?,
            to: I32T.read(r)?,
            scale: F32T.read(r)?,
        },
        Shape::TransitionScaleLast => ParticleData::Transition {
            from: read_rgb_floats(r)?,
            to: read_rgb_floats(r)?,
            scale: F32T.read(r)?,
        },
        Shape::TransitionScaleMid => ParticleData::Transition {
            from: read_rgb_floats(r)?,
            scale: F32T.read(r)?,
            to: read_rgb_floats(r)?,
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
                origin: None,
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
        Shape::Trail => ParticleData::Trail {
            x: F64T.read(r)?,
            y: F64T.read(r)?,
            z: F64T.read(r)?,
            color: I32T.read(r)?,
            duration: 0,
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
        Shape::VibrationNamed => {
            let source_name = STRING.read(r)?;
            let source = if source_name.ends_with(":block") {
                VibrationSource::Block(I64T.read(r)?)
            } else if source_name.ends_with(":entity") {
                VibrationSource::Entity {
                    id: VAR_INT.read(r)?.0,
                    y_offset: F32T.read(r)?,
                }
            } else {
                return Err(TranslateError::Unsupported("vibration source"));
            };
            ParticleData::Vibration {
                origin: None,
                source,
                ticks: VAR_INT.read(r)?.0,
            }
        }
        Shape::VibrationWithSource => {
            let origin = I64T.read(r)?;
            let source_name = STRING.read(r)?;
            let source = if source_name.ends_with(":block") {
                VibrationSource::Block(I64T.read(r)?)
            } else if source_name.ends_with(":entity") {
                VibrationSource::Entity {
                    id: VAR_INT.read(r)?.0,
                    y_offset: 0.0,
                }
            } else {
                return Err(TranslateError::Unsupported("vibration source"));
            };
            ParticleData::Vibration {
                origin: Some(origin),
                source,
                ticks: VAR_INT.read(r)?.0,
            }
        }
    })
}

fn read_rgb_floats(r: &mut &[u8]) -> Result<i32, TranslateError> {
    let mut rgb = 0i32;
    for _ in 0..3 {
        let channel = F32T.read(r)?;
        if !channel.is_finite() {
            return Err(TranslateError::Unsupported("non-finite particle color"));
        }
        rgb = (rgb << 8) | (channel.mul_add(255.0, 0.5).clamp(0.0, 255.0) as i32);
    }
    Ok(rgb)
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
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    connection: Option<&mut UserConnection>,
) -> Result<bool, TranslateError> {
    match (shape, data) {
        (Shape::None, ParticleData::None) => {}
        (Shape::None, _) => return Ok(false),
        (Shape::Block, ParticleData::Block(state)) => {
            let state = ids.blockstates.map(*state).unwrap_or(0);
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
        (Shape::VibrationTyped, ParticleData::Vibration { source, ticks, .. }) => {
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
        (Shape::VibrationNamed, ParticleData::Vibration { source, ticks, .. }) => {
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
        (
            Shape::VibrationWithSource,
            ParticleData::Vibration {
                origin: Some(origin),
                source,
                ticks,
            },
        ) => {
            I64T.write(out, origin)?;
            match source {
                VibrationSource::Block(position) => {
                    STRING.write(out, &"minecraft:block".into())?;
                    I64T.write(out, position)?;
                }
                VibrationSource::Entity { id, .. } => {
                    STRING.write(out, &"minecraft:entity".into())?;
                    VAR_INT.write(out, &VarInt(*id))?;
                }
            }
            VAR_INT.write(out, &VarInt(*ticks))?;
        }
        (Shape::VibrationWithSource, ParticleData::Vibration { origin: None, .. }) => {
            return Ok(false);
        }
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
        (Shape::Item, ParticleData::Item(item)) => {
            let mut input = item.as_slice();
            let rewritten = match connection {
                Some(connection) => {
                    rewrite_item_value_with_connection(&mut input, layout, ids, connection)
                }
                None => rewrite_item_value(&mut input, layout, ids),
            };
            let Some(rewritten) = rewritten else {
                return Ok(false);
            };
            if !input.is_empty() {
                return Ok(false);
            }
            out.extend_from_slice(&rewritten);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Reads the option data 26.3 writes for `id`.
pub fn read_particle_data(r: &mut &[u8], id: i32) -> Result<ParticleData, TranslateError> {
    let layout = JavaMinecraftVersion::V_26_3;
    read_data(
        r,
        shape_of(id, layout),
        layout,
        MappingData::get().composed(layout),
    )
}

pub fn read_particle(r: &mut &[u8]) -> Result<Particle, TranslateError> {
    let id = VAR_INT.read(r)?.0;
    let data = read_particle_data(r, id)?;
    Ok(Particle { id, data })
}

/// Reads a particle whose id and option data use `layout`, then returns its
/// canonical 26.3 id and values. Shapes that do not carry enough information
/// for the canonical form fail closed.
pub fn read_particle_for_layout(
    r: &mut &[u8],
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<Particle, TranslateError> {
    let wire_id = VAR_INT.read(r)?.0;
    let canonical_id = u32::try_from(wire_id)
        .ok()
        .and_then(|id| ids.particles_inverse().map(id))
        .and_then(|id| i32::try_from(id).ok())
        .ok_or(TranslateError::Unsupported("particle id reverse mapping"))?;
    let mut data = read_data(r, shape_of(canonical_id, layout), layout, ids)?;
    if let ParticleData::Block(wire_state) = &data {
        let canonical_state =
            ids.blockstates_inverse()
                .map(*wire_state)
                .ok_or(TranslateError::Unsupported(
                    "particle block state reverse mapping",
                ))?;
        data = ParticleData::Block(canonical_state);
    }
    Ok(Particle {
        id: canonical_id,
        data,
    })
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
    write_particle_inner(out, particle, layout, ids, None)
}

/// Writes a particle and records client-inexpressible data in its nested item
/// stack using the owning player's backup cache.
pub fn write_particle_with_connection(
    out: &mut Vec<u8>,
    particle: &Particle,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    connection: &mut UserConnection,
) -> Result<bool, TranslateError> {
    write_particle_inner(out, particle, layout, ids, Some(connection))
}

fn write_particle_inner(
    out: &mut Vec<u8>,
    particle: &Particle,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    connection: Option<&mut UserConnection>,
) -> Result<bool, TranslateError> {
    let Some(mapped) = mapped_id(particle.id, ids) else {
        return Ok(false);
    };
    let shape = mapped_shape(u32::try_from(mapped).unwrap_or(0), layout, ids);
    let mut data = Vec::new();
    if !write_data(&mut data, &particle.data, shape, layout, ids, connection)? {
        return Ok(false);
    }
    VAR_INT.write(out, &VarInt(mapped))?;
    out.extend_from_slice(&data);
    Ok(true)
}

/// Packs a particle packet's position as a block position for the 1.17/1.18
/// vibration option. Those layouts carry the particle origin in the option.
fn vibration_origin(x: f64, y: f64, z: f64) -> Option<i64> {
    fn coordinate(value: f64, min: i32, max: i32) -> Option<i32> {
        let value = value.floor();
        if !value.is_finite() || value < f64::from(min) || value > f64::from(max) {
            return None;
        }
        Some(value as i32)
    }

    let x = coordinate(x, -(1 << 25), (1 << 25) - 1)?;
    let y = coordinate(y, -(1 << 11), (1 << 11) - 1)?;
    let z = coordinate(z, -(1 << 25), (1 << 25) - 1)?;
    Some(
        ((i64::from(x) & 0x3ff_ffff) << 38)
            | ((i64::from(z) & 0x3ff_ffff) << 12)
            | (i64::from(y) & 0xfff),
    )
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
        match write_data(
            w,
            &v.data,
            shape,
            JavaMinecraftVersion::V_26_3,
            MappingData::get().composed(JavaMinecraftVersion::V_26_3),
            None,
        ) {
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
    rewrite_particle_with_origin(wrapper, layout, ids, None, None)
}

/// Rewrites one packet particle with access to the player's item backup cache.
pub fn rewrite_particle_packet_with_connection(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<bool, TranslateError> {
    rewrite_particle_with_origin(wrapper, layout, ids, None, Some(connection))
}

fn rewrite_particle_with_origin(
    wrapper: &mut PacketWrapper,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    origin: Option<i64>,
    connection: Option<&mut UserConnection>,
) -> Result<bool, TranslateError> {
    let mut particle = wrapper.read(&PARTICLE)?;
    if layout < JavaMinecraftVersion::V_1_19
        && let ParticleData::Vibration {
            origin: particle_origin,
            ..
        } = &mut particle.data
    {
        *particle_origin = origin;
    }
    let mut out = Vec::new();
    let written = match connection {
        Some(connection) => {
            write_particle_with_connection(&mut out, &particle, layout, ids, connection)?
        }
        None => write_particle(&mut out, &particle, layout, ids)?,
    };
    if !written {
        return Ok(false);
    }
    wrapper.write_bytes(&out);
    Ok(true)
}

/// The particle moved to the front of `LEVEL_PARTICLES` in 26.3 and sat after
/// the count from 1.20.5; below that the id leads and the option data trails.
pub fn level_particles(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    use JavaMinecraftVersion as V;

    if layout >= V::V_26_3 {
        if !rewrite_particle_packet_with_connection(wrapper, connection, layout, ids)? {
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
        let x = wrapper.passthrough(&F64T)?;
        let y = wrapper.passthrough(&F64T)?;
        let z = wrapper.passthrough(&F64T)?;
        for _ in 0..4 {
            wrapper.passthrough(&F32T)?;
        }
        wrapper.passthrough(&I32T)?;
        let origin = (layout < V::V_1_19)
            .then(|| vibration_origin(x, y, z))
            .flatten();
        if !rewrite_particle_with_origin(wrapper, layout, ids, origin, Some(connection))? {
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
    let (x, y, z) = if layout >= V::V_1_15 {
        (
            wrapper.passthrough(&F64T)?,
            wrapper.passthrough(&F64T)?,
            wrapper.passthrough(&F64T)?,
        )
    } else {
        (
            f64::from(wrapper.passthrough(&F32T)?),
            f64::from(wrapper.passthrough(&F32T)?),
            f64::from(wrapper.passthrough(&F32T)?),
        )
    };
    for _ in 0..3 {
        wrapper.passthrough(&F32T)?;
    }

    let speed = wrapper.read(&F32T)?;
    let count = wrapper.read(&I32T)?;
    // The option data is the rest of the payload here.
    let mut cursor = wrapper.remaining();
    let mut data = read_particle_data(&mut cursor, id)?;
    if !cursor.is_empty() {
        return Err(TranslateError::TrailingBytes(cursor.len()));
    }
    wrapper.consume_remaining();
    let shape = mapped_shape(u32::try_from(mapped).unwrap_or(0), layout, ids);
    if layout < V::V_1_19
        && let ParticleData::Vibration {
            origin: particle_origin,
            ..
        } = &mut data
    {
        *particle_origin = vibration_origin(x, y, z);
    }

    // 1.20.5 folded the potion colour into the particle's own data; below it
    // the client takes an unused speed as the colour.
    let entity_effect = u16::try_from(id).ok() == Some(ParticleKind::EntityEffect.to_id());
    let mut speed = speed;
    if entity_effect && shape == Shape::None {
        if let ParticleData::Color(colour) = &data {
            if speed == 0.0 {
                speed = *colour as f32;
            }
            // Older clients carry this color in the packet's speed field and
            // have no particle option bytes to read.
            data = ParticleData::None;
        }
    }

    wrapper.write(&F32T, &speed)?;
    wrapper.write(&I32T, &count)?;
    let mut out = Vec::new();
    if write_data(&mut out, &data, shape, layout, ids, Some(connection))? {
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
    connection: &mut UserConnection,
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
        if !rewrite_particle_packet_with_connection(wrapper, connection, layout, ids)?
            || !sound::rewrite_holder(wrapper, layout, ids)?
        {
            wrapper.cancel();
            return Ok(());
        }
        if layout >= V::V_1_21_9 {
            let block_particles = wrapper.passthrough(&VAR_INT)?.0;
            for _ in 0..block_particles {
                if !rewrite_particle_packet_with_connection(wrapper, connection, layout, ids)? {
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
            if !rewrite_particle_packet_with_connection(wrapper, connection, layout, ids)? {
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
    use crate::api::rewriter::item::ClientboundItemT;
    use crate::api::rewriter::item::StructuredItemRewriter;
    use crate::api::rewriter::item_backup::restore_full_item;
    use crate::api::types::{Item, ItemComponent, ItemT};
    use crate::packet::mappings::clientbound::play::{EXPLODE, LEVEL_PARTICLES};
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::item::Item as DataItem;
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

    #[test]
    fn item_particles_rewrite_the_nested_stack_for_old_clients() {
        let source_item = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND.id),
            added: Vec::new(),
            removed: Vec::new(),
        };
        let mut item_bytes = Vec::new();
        ItemT::for_version(JavaMinecraftVersion::V_26_3)
            .write(&mut item_bytes, &source_item)
            .unwrap();
        let source_particle = Particle {
            id: i32::from(ParticleKind::Item.to_id()),
            data: ParticleData::Item(item_bytes),
        };

        for version in [JavaMinecraftVersion::V_26_2, JavaMinecraftVersion::V_1_16_2] {
            let ids = MappingData::get().composed(version);
            let mut output = Vec::new();
            assert!(write_particle(&mut output, &source_particle, version, ids).unwrap());

            let mut input = output.as_slice();
            assert_eq!(
                VAR_INT.read(&mut input).unwrap().0,
                mapped(i32::from(ParticleKind::Item.to_id()), version)
            );
            let item = ClientboundItemT::new(version, ids)
                .read(&mut input)
                .unwrap();
            assert!(input.is_empty());
            let Item::Structured { id, .. } = item else {
                panic!("diamond item particle remains a structured stack");
            };
            assert_eq!(id, i32::from(DataItem::DIAMOND.id), "{version}");
        }
    }

    #[test]
    fn item_particles_read_and_rewrite_nested_stacks_from_older_layouts() {
        let source_item = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND.id),
            added: Vec::new(),
            removed: Vec::new(),
        };

        for version in [JavaMinecraftVersion::V_26_2, JavaMinecraftVersion::V_1_16_2] {
            let ids = MappingData::get().composed(version);
            let target_item = StructuredItemRewriter::to_version(&source_item, version, ids);
            let mut payload = Vec::new();
            VAR_INT
                .write(
                    &mut payload,
                    &VarInt(mapped(i32::from(ParticleKind::Item.to_id()), version)),
                )
                .unwrap();
            ItemT::for_version(version)
                .write(&mut payload, &target_item)
                .unwrap();

            let mut input = payload.as_slice();
            let particle = read_particle_for_layout(&mut input, version, ids).unwrap();
            assert!(input.is_empty(), "{version}");
            assert_eq!(particle.id, i32::from(ParticleKind::Item.to_id()));

            let mut output = Vec::new();
            assert!(write_particle(&mut output, &particle, version, ids).unwrap());
            let mut translated = output.as_slice();
            assert_eq!(
                VAR_INT.read(&mut translated).unwrap().0,
                mapped(i32::from(ParticleKind::Item.to_id()), version),
                "{version}"
            );
            let item = ClientboundItemT::new(version, ids)
                .read(&mut translated)
                .unwrap();
            assert!(translated.is_empty(), "{version}");
            assert_eq!(
                item.item_id(),
                Some(i32::from(DataItem::DIAMOND.id)),
                "{version}"
            );
        }
    }

    #[test]
    fn item_particles_use_the_player_backup_cache() {
        let version = JavaMinecraftVersion::V_1_21_4;
        let ids = MappingData::get().composed(version);
        let item = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND.id),
            added: vec![ItemComponent {
                id: i32::from(DataComponent::AttackAnimation.to_id()),
                data: vec![1, 6],
            }],
            removed: Vec::new(),
        };
        let mut item_bytes = Vec::new();
        ItemT::for_version(JavaMinecraftVersion::V_26_3)
            .write(&mut item_bytes, &item)
            .unwrap();
        let particle = Particle {
            id: i32::from(ParticleKind::Item.to_id()),
            data: ParticleData::Item(item_bytes),
        };
        let mut connection = UserConnection::new(22, version);
        let mut output = Vec::new();
        assert!(
            write_particle_with_connection(&mut output, &particle, version, ids, &mut connection,)
                .unwrap()
        );

        let mut read = output.as_slice();
        VAR_INT.read(&mut read).unwrap(); // Particle id
        let mut returned = ClientboundItemT::new(version, ids).read(&mut read).unwrap();
        assert!(read.is_empty());
        restore_full_item(&connection, &mut returned, version, ids);
        let Item::Structured { added, .. } = returned else {
            panic!("the item particle contains a structured item");
        };
        assert!(added.contains(&ItemComponent {
            id: i32::from(DataComponent::AttackAnimation.to_id()),
            data: vec![1, 6],
        }));
    }

    #[test]
    fn a_particle_without_a_known_shape_does_not_drop_option_bytes() {
        let mut output = Vec::new();
        assert!(
            !write_data(
                &mut output,
                &ParticleData::Color(0x1122_3344),
                Shape::None,
                JavaMinecraftVersion::V_26_2,
                MappingData::get().composed(JavaMinecraftVersion::V_26_2),
                None,
            )
            .unwrap()
        );
        assert!(output.is_empty());
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

        // 1.18 needs an outer particle position to synthesize the vibration
        // origin; this direct seam has none. Geysers are 26.2's.
        let vibration = Particle {
            id: i32::from(ParticleKind::Vibration.to_id()),
            data: ParticleData::Vibration {
                origin: None,
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

    #[test]
    fn level_vibrations_get_their_origin_from_the_packet_position_for_1_18() {
        let version = JavaMinecraftVersion::V_1_18_2;
        let id = i32::from(ParticleKind::Vibration.to_id());
        let destination = 0x0000_0004_0000_0005i64;
        let mut data = Vec::new();
        VAR_INT.write(&mut data, &VarInt(0)).unwrap();
        I64T.write(&mut data, &destination).unwrap();
        VAR_INT.write(&mut data, &VarInt(20)).unwrap();

        let output = run(
            level_particles,
            &LEVEL_PARTICLES,
            &particles_payload(id, &data, version),
            version,
        )
        .unwrap();
        let mut translated = output.as_slice();
        assert_eq!(I32T.read(&mut translated).unwrap(), mapped(id, version));
        assert!(BOOL.read(&mut translated).unwrap());
        assert_eq!(F64T.read(&mut translated).unwrap(), 1.0);
        assert_eq!(F64T.read(&mut translated).unwrap(), 2.0);
        assert_eq!(F64T.read(&mut translated).unwrap(), 3.0);
        for expected in [0.1f32, 0.2, 0.3, 0.5] {
            assert_eq!(F32T.read(&mut translated).unwrap(), expected);
        }
        assert_eq!(I32T.read(&mut translated).unwrap(), 4);
        let origin = ((1i64 & 0x3ff_ffff) << 38) | ((3i64 & 0x3ff_ffff) << 12) | (2i64 & 0xfff);
        assert_eq!(I64T.read(&mut translated).unwrap(), origin);
        assert_eq!(
            STRING.read(&mut translated).unwrap().as_ref(),
            "minecraft:block"
        );
        assert_eq!(I64T.read(&mut translated).unwrap(), destination);
        assert_eq!(VAR_INT.read(&mut translated).unwrap().0, 20);
        assert!(translated.is_empty());
    }

    #[test]
    fn legacy_vibration_particle_data_keeps_its_origin_position() {
        let version = JavaMinecraftVersion::V_1_18_2;
        let ids = MappingData::get().composed(version);
        let id = i32::from(ParticleKind::Vibration.to_id());
        let origin = 0x0000_0001_0000_0002i64;
        let destination = 0x0000_0004_0000_0005i64;
        let mut input = Vec::new();
        VAR_INT
            .write(&mut input, &VarInt(mapped(id, version)))
            .unwrap();
        I64T.write(&mut input, &origin).unwrap();
        STRING.write(&mut input, &"minecraft:block".into()).unwrap();
        I64T.write(&mut input, &destination).unwrap();
        VAR_INT.write(&mut input, &VarInt(20)).unwrap();

        let mut cursor = input.as_slice();
        let particle = read_particle_for_layout(&mut cursor, version, ids).unwrap();
        assert!(cursor.is_empty());
        assert!(matches!(
            &particle.data,
            ParticleData::Vibration {
                origin: Some(found),
                source: VibrationSource::Block(found_destination),
                ticks: 20,
            } if *found == origin && *found_destination == destination
        ));

        let mut output = Vec::new();
        assert!(write_particle(&mut output, &particle, version, ids).unwrap());
        assert_eq!(output, input);

        let mut entity_input = Vec::new();
        VAR_INT
            .write(&mut entity_input, &VarInt(mapped(id, version)))
            .unwrap();
        I64T.write(&mut entity_input, &origin).unwrap();
        STRING
            .write(&mut entity_input, &"minecraft:entity".into())
            .unwrap();
        VAR_INT.write(&mut entity_input, &VarInt(45)).unwrap();
        VAR_INT.write(&mut entity_input, &VarInt(20)).unwrap();
        let mut cursor = entity_input.as_slice();
        let particle = read_particle_for_layout(&mut cursor, version, ids).unwrap();
        assert!(cursor.is_empty());
        assert!(matches!(
            &particle.data,
            ParticleData::Vibration {
                origin: Some(found),
                source: VibrationSource::Entity { id: 45, .. },
                ticks: 20,
            } if *found == origin
        ));
        let mut output = Vec::new();
        assert!(write_particle(&mut output, &particle, version, ids).unwrap());
        assert_eq!(output, entity_input);
    }

    #[test]
    fn vibration_origin_uses_floor_and_rejects_unrepresentable_positions() {
        let origin = vibration_origin(-0.25, -64.1, 3.99).unwrap();
        let expected =
            ((-1i64 & 0x3ff_ffff) << 38) | ((3i64 & 0x3ff_ffff) << 12) | (-65i64 & 0xfff);
        assert_eq!(origin, expected);
        assert!(vibration_origin(f64::NAN, 0.0, 0.0).is_none());
        assert!(vibration_origin(0.0, 2048.0, 0.0).is_none());
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
    fn the_potion_colour_moves_into_speed_without_particle_option_bytes_below_1_20_5() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let effect = i32::from(ParticleKind::EntityEffect.to_id());
        let speed = 1 + 1 + 24 + 12;

        for original_speed in [0.0f32, 1.5] {
            let mut payload = particles_payload(effect, &[], version);
            payload[speed..speed + 4].copy_from_slice(&original_speed.to_be_bytes());
            payload.write_i32_be(64).unwrap();

            let mut expected = particles_payload(mapped(effect, version), &[], version);
            let expected_speed = if original_speed == 0.0 {
                64.0
            } else {
                original_speed
            };
            expected[speed..speed + 4].copy_from_slice(&expected_speed.to_be_bytes());
            assert_eq!(
                run(level_particles, &LEVEL_PARTICLES, &payload, version).unwrap(),
                expected
            );
        }
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
