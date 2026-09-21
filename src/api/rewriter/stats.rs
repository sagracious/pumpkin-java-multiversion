use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::VAR_INT;
use crate::api::{ComposedMappings, IdMapping, PacketWrapper, TranslateError, UserConnection};

/// The id space a statistic's second field belongs to.
/// `ViaVersion`'s `StatisticsRewriter.getRegistryTypeForStatistic`.
fn space(category: i32, ids: &ComposedMappings) -> &IdMapping {
    match category {
        0 => &ids.blocks,
        1..=5 => &ids.items,
        6 | 7 => &ids.entities,
        _ => &ids.statistics,
    }
}

/// Renumbers every statistic and leaves out the ones the client has no id for.
pub fn award_stats(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    let count = wrapper.read(&VAR_INT)?.0;
    let mut kept = Vec::with_capacity(usize::try_from(count).unwrap_or(0).min(1024));
    for _ in 0..count {
        let category = wrapper.read(&VAR_INT)?.0;
        let statistic = wrapper.read(&VAR_INT)?.0;
        let value = wrapper.read(&VAR_INT)?.0;
        let mapped = u32::try_from(statistic)
            .ok()
            .and_then(|id| space(category, ids).map(id))
            .and_then(|id| i32::try_from(id).ok());
        if let Some(statistic) = mapped {
            kept.push((category, statistic, value));
        }
    }

    wrapper.write(&VAR_INT, &VarInt(i32::try_from(kept.len()).unwrap_or(0)))?;
    for (category, statistic, value) in kept {
        wrapper.write(&VAR_INT, &VarInt(category))?;
        wrapper.write(&VAR_INT, &VarInt(statistic))?;
        wrapper.write(&VAR_INT, &VarInt(value))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::packet::mappings::clientbound::play::AWARD_STATS;
    use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};

    fn payload(stats: &[(i32, i32, i32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.write_var_int(&VarInt(i32::try_from(stats.len()).unwrap()))
            .unwrap();
        for (category, statistic, value) in stats {
            out.write_var_int(&VarInt(*category)).unwrap();
            out.write_var_int(&VarInt(*statistic)).unwrap();
            out.write_var_int(&VarInt(*value)).unwrap();
        }
        out
    }

    fn run(stats: &[(i32, i32, i32)], version: JavaMinecraftVersion) -> Vec<(i32, i32, i32)> {
        let ids = MappingData::get().composed(version);
        let bytes = payload(stats);
        let mut wrapper = PacketWrapper::new(&AWARD_STATS, &bytes);
        let mut connection = UserConnection::new(0, version);
        award_stats(&mut wrapper, &mut connection, version, ids).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut cursor = out.as_slice();
        let count = cursor.get_var_int().unwrap().0;
        let mut read = Vec::new();
        for _ in 0..count {
            read.push((
                cursor.get_var_int().unwrap().0,
                cursor.get_var_int().unwrap().0,
                cursor.get_var_int().unwrap().0,
            ));
        }
        assert!(cursor.is_empty());
        read
    }

    /// Mined takes a block id, the five item categories an item id, killed and
    /// killed by an entity id and custom the statistic registry.
    #[test]
    fn every_category_goes_through_its_own_id_space() {
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_16_2,
        ] {
            let ids = MappingData::get().composed(version);
            let stats = [(0, 1, 7), (3, 1, 7), (6, 1, 7), (8, 1, 7)];
            let expected: Vec<(i32, i32, i32)> = stats
                .iter()
                .map(|(category, statistic, value)| {
                    let mapped = space(*category, ids)
                        .map(u32::try_from(*statistic).unwrap())
                        .unwrap();
                    (*category, i32::try_from(mapped).unwrap(), *value)
                })
                .collect();
            assert_eq!(run(&stats, version), expected, "{version}");
        }
    }

    /// Custom statistic 17 is one of the two 1.16.2 does not have; the entry
    /// goes and the count drops with it.
    #[test]
    fn a_statistic_the_client_lacks_is_left_out() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let ids = MappingData::get().composed(version);
        assert!(ids.statistics.map(17).is_none());

        let kept = i32::try_from(ids.statistics.map(0).unwrap()).unwrap();
        assert_eq!(
            run(&[(8, 17, 1), (8, 0, 2)], version),
            vec![(8, kept, 2)],
            "{version}"
        );
    }
}
