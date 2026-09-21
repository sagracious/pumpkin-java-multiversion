//! `RESET_SCORE` for clients that predate it.
//!
//! 1.20.3 split score removal out of `SET_SCORE` into its own packet. Every
//! client below that has no `RESET_SCORE`, so scores would never clear
//! without this; those versions remove a score with `SET_SCORE` action 1
//! instead, which is what this rewrite produces.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

/// First version with `RESET_SCORE`; anything older gets the rewrite.
pub const FIRST_WITH_RESET_SCORE: JavaMinecraftVersion = JavaMinecraftVersion::V_1_20_3;

/// Turns a `RESET_SCORE` payload into a `SET_SCORE` action-1 payload; an absent objective
/// becomes the empty string, which 1.20.2 reads as "every objective".
#[must_use]
pub fn rewrite_reset_score(payload: &[u8]) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let owner = cursor.get_str().ok()?;
    let objective = cursor.get_option(|c| c.get_str()).ok()?;
    if !cursor.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(payload.len() + 1);
    out.write_string(&owner).ok()?;
    out.write_var_int(&VarInt(1)).ok()?;
    out.write_string(objective.as_deref().unwrap_or("")).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::rewrite_reset_score;

    #[test]
    fn reset_with_objective_becomes_remove_action() {
        let mut payload = Vec::new();
        payload.push(3);
        payload.extend_from_slice(b"bob");
        payload.push(1);
        payload.push(5);
        payload.extend_from_slice(b"kills");
        let out = rewrite_reset_score(&payload).unwrap();
        assert_eq!(out, b"\x03bob\x01\x05kills");
    }

    #[test]
    fn reset_without_objective_uses_the_empty_objective() {
        let out = rewrite_reset_score(b"\x03bob\x00").unwrap();
        assert_eq!(out, b"\x03bob\x01\x00");
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        assert!(rewrite_reset_score(b"\x03bob\x00\x00").is_none());
        assert!(rewrite_reset_score(b"\x03bo").is_none());
    }
}
