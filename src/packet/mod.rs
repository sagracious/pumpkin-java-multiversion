use pumpkin_util::version::JavaMinecraftVersion;

pub mod block_update;
pub mod chunk_legacy;
pub mod chunk_remap;
pub mod entity;
pub mod join;
pub mod legacy;
pub mod mappings;
pub mod score;
pub mod status;
pub mod update_tags;

/// Oldest client version this plugin admits; below 1.16.2 there's no configuration state and the join/tag/chunk/spawn packet shapes diverge too much to translate.
pub const LOWEST_SUPPORTED: JavaMinecraftVersion = JavaMinecraftVersion::V_1_16_2;

/// Newest client version, the server's own.
pub const HIGHEST_SUPPORTED: JavaMinecraftVersion = JavaMinecraftVersion::V_26_3;

#[must_use]
pub fn is_version_supported(version: JavaMinecraftVersion) -> bool {
    version != JavaMinecraftVersion::Unknown
        && version >= LOWEST_SUPPORTED
        && version <= HIGHEST_SUPPORTED
}
