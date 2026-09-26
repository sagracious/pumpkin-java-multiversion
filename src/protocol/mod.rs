#![allow(non_camel_case_types)]

use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::Protocol;

pub mod v1_12_1to1_12;
pub mod v1_12_2to1_12_1;
pub mod v1_12to1_11_1;
pub mod v1_13_1to1_13;
pub mod v1_13_2to1_13_1;
pub mod v1_13to1_12_2;
pub mod v1_14_1to1_14;
pub mod v1_14_2to1_14_1;
pub mod v1_14_3to1_14_2;
pub mod v1_14_4to1_14_3;
pub mod v1_14to1_13_2;
pub mod v1_15_1to1_15;
pub mod v1_15_2to1_15_1;
pub mod v1_15to1_14_4;
pub mod v1_16_1to1_16;
pub mod v1_16_2to1_16_1;
pub mod v1_16_3to1_16_2;
pub mod v1_16_4to1_16_3;
pub mod v1_16to1_15_2;
pub mod v1_17_1to1_17;
pub mod v1_17to1_16_4;
pub mod v1_18_2to1_18;
pub mod v1_18to1_17_1;
pub mod v1_19_1to1_19;
pub mod v1_19_3to1_19_1;
pub mod v1_19_4to1_19_3;
pub mod v1_19to1_18_2;
pub mod v1_20_2to1_20;
pub mod v1_20_3to1_20_2;
pub mod v1_20_5to1_20_3;
pub mod v1_20to1_19_4;
pub mod v1_21_11to1_21_9;
pub mod v1_21_2to1_21;
pub mod v1_21_4to1_21_2;
pub mod v1_21_5to1_21_4;
pub mod v1_21_6to1_21_5;
pub mod v1_21_7to1_21_6;
pub mod v1_21_9to1_21_7;
pub mod v1_21to1_20_5;
pub mod v26_1to1_21_11;
pub mod v26_2to26_1;
pub mod v26_3to26_2;

/// Every step endpoint, newest first.
pub const VERSIONS: &[JavaMinecraftVersion] = &[
    JavaMinecraftVersion::V_26_3,
    JavaMinecraftVersion::V_26_2,
    JavaMinecraftVersion::V_26_1,
    JavaMinecraftVersion::V_1_21_11,
    JavaMinecraftVersion::V_1_21_9,
    JavaMinecraftVersion::V_1_21_7,
    JavaMinecraftVersion::V_1_21_6,
    JavaMinecraftVersion::V_1_21_5,
    JavaMinecraftVersion::V_1_21_4,
    JavaMinecraftVersion::V_1_21_2,
    JavaMinecraftVersion::V_1_21,
    JavaMinecraftVersion::V_1_20_5,
    JavaMinecraftVersion::V_1_20_3,
    JavaMinecraftVersion::V_1_20_2,
    JavaMinecraftVersion::V_1_20,
    JavaMinecraftVersion::V_1_19_4,
    JavaMinecraftVersion::V_1_19_3,
    JavaMinecraftVersion::V_1_19_1,
    JavaMinecraftVersion::V_1_19,
    JavaMinecraftVersion::V_1_18_2,
    JavaMinecraftVersion::V_1_18,
    JavaMinecraftVersion::V_1_17_1,
    JavaMinecraftVersion::V_1_17,
    JavaMinecraftVersion::V_1_16_4,
    JavaMinecraftVersion::V_1_16_3,
    JavaMinecraftVersion::V_1_16_2,
    JavaMinecraftVersion::V_1_16_1,
    JavaMinecraftVersion::V_1_16,
    JavaMinecraftVersion::V_1_15_2,
    JavaMinecraftVersion::V_1_15_1,
    JavaMinecraftVersion::V_1_15,
    JavaMinecraftVersion::V_1_14_4,
    JavaMinecraftVersion::V_1_14_3,
    JavaMinecraftVersion::V_1_14_2,
    JavaMinecraftVersion::V_1_14_1,
    JavaMinecraftVersion::V_1_14,
    JavaMinecraftVersion::V_1_13_2,
    JavaMinecraftVersion::V_1_13_1,
    JavaMinecraftVersion::V_1_13,
    JavaMinecraftVersion::V_1_12_2,
    JavaMinecraftVersion::V_1_12_1,
    JavaMinecraftVersion::V_1_12,
    JavaMinecraftVersion::V_1_11_1,
];

/// The chain in ViaBackwards order.
pub static STEPS: &[&dyn Protocol] = &[
    &v26_3to26_2::Protocol26_3To26_2,
    &v26_2to26_1::Protocol26_2To26_1,
    &v26_1to1_21_11::Protocol26_1To1_21_11,
    &v1_21_11to1_21_9::Protocol1_21_11To1_21_9,
    &v1_21_9to1_21_7::Protocol1_21_9To1_21_7,
    &v1_21_7to1_21_6::Protocol1_21_7To1_21_6,
    &v1_21_6to1_21_5::Protocol1_21_6To1_21_5,
    &v1_21_5to1_21_4::Protocol1_21_5To1_21_4,
    &v1_21_4to1_21_2::Protocol1_21_4To1_21_2,
    &v1_21_2to1_21::Protocol1_21_2To1_21,
    &v1_21to1_20_5::Protocol1_21To1_20_5,
    &v1_20_5to1_20_3::Protocol1_20_5To1_20_3,
    &v1_20_3to1_20_2::Protocol1_20_3To1_20_2,
    &v1_20_2to1_20::Protocol1_20_2To1_20,
    &v1_20to1_19_4::Protocol1_20To1_19_4,
    &v1_19_4to1_19_3::Protocol1_19_4To1_19_3,
    &v1_19_3to1_19_1::Protocol1_19_3To1_19_1,
    &v1_19_1to1_19::Protocol1_19_1To1_19,
    &v1_19to1_18_2::Protocol1_19To1_18_2,
    &v1_18_2to1_18::Protocol1_18_2To1_18,
    &v1_18to1_17_1::Protocol1_18To1_17_1,
    &v1_17_1to1_17::Protocol1_17_1To1_17,
    &v1_17to1_16_4::Protocol1_17To1_16_4,
    &v1_16_4to1_16_3::Protocol1_16_4To1_16_3,
    &v1_16_3to1_16_2::Protocol1_16_3To1_16_2,
    &v1_16_2to1_16_1::Protocol1_16_2To1_16_1,
    &v1_16_1to1_16::Protocol1_16_1To1_16,
    &v1_16to1_15_2::Protocol1_16To1_15_2,
    &v1_15_2to1_15_1::Protocol1_15_2To1_15_1,
    &v1_15_1to1_15::Protocol1_15_1To1_15,
    &v1_15to1_14_4::Protocol1_15To1_14_4,
    &v1_14_4to1_14_3::Protocol1_14_4To1_14_3,
    &v1_14_3to1_14_2::Protocol1_14_3To1_14_2,
    &v1_14_2to1_14_1::Protocol1_14_2To1_14_1,
    &v1_14_1to1_14::Protocol1_14_1To1_14,
    &v1_14to1_13_2::Protocol1_14To1_13_2,
    &v1_13_2to1_13_1::Protocol1_13_2To1_13_1,
    &v1_13_1to1_13::Protocol1_13_1To1_13,
    &v1_13to1_12_2::Protocol1_13To1_12_2,
    &v1_12_2to1_12_1::Protocol1_12_2To1_12_1,
    &v1_12_1to1_12::Protocol1_12_1To1_12,
    &v1_12to1_11_1::Protocol1_12To1_11_1,
];
