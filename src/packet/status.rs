//! Rewrites the status response so older clients show the server as joinable.
//!
//! A 26.3 server always reports protocol 777, so an older client would draw a
//! red cross and "Incompatible version!" even though this plugin lets it connect.

use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

/// Rewrites `version.protocol`/`version.name` so the client sees itself as compatible.
/// `None` means the payload wasn't the expected JSON string; caller leaves it alone.
#[must_use]
pub fn rewrite_status_response(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let json = cursor.get_str().ok()?;

    let mut value: serde_json::Value = serde_json::from_str(&json).ok()?;
    let version_obj = value.get_mut("version")?.as_object_mut()?;

    version_obj.insert(
        "protocol".to_string(),
        serde_json::Value::from(version.protocol_version()),
    );
    version_obj.insert(
        "name".to_string(),
        serde_json::Value::from(version.to_string()),
    );

    let rewritten = serde_json::to_string(&value).ok()?;
    let mut out = Vec::with_capacity(rewritten.len() + 5);
    out.write_string(&rewritten).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::rewrite_status_response;
    use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
    use pumpkin_util::version::JavaMinecraftVersion;

    const TIER_4: &[JavaMinecraftVersion] = &[
        JavaMinecraftVersion::V_1_16_2,
        JavaMinecraftVersion::V_1_16_3,
        JavaMinecraftVersion::V_1_16_4,
        JavaMinecraftVersion::V_1_17,
        JavaMinecraftVersion::V_1_17_1,
        JavaMinecraftVersion::V_1_18,
        JavaMinecraftVersion::V_1_18_2,
        JavaMinecraftVersion::V_1_19,
        JavaMinecraftVersion::V_1_19_1,
        JavaMinecraftVersion::V_1_19_3,
        JavaMinecraftVersion::V_1_19_4,
        JavaMinecraftVersion::V_1_20,
        JavaMinecraftVersion::V_1_20_2,
        JavaMinecraftVersion::V_1_20_3,
        JavaMinecraftVersion::V_1_20_5,
    ];

    fn payload(json: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.write_string(json).expect("write status json");
        out
    }

    const SERVER_RESPONSE: &str = concat!(
        r#"{"version":{"name":"26.3","protocol":777},"#,
        r#""players":{"max":20,"online":0,"sample":[]},"#,
        r#""description":{"text":"A Pumpkin Server"},"enforcesSecureChat":false}"#
    );

    #[test]
    fn every_tier_4_version_gets_its_own_protocol_back() {
        for version in TIER_4 {
            let out = rewrite_status_response(&payload(SERVER_RESPONSE), *version)
                .unwrap_or_else(|| panic!("{version} produced no response"));

            let mut cursor = out.as_slice();
            let json = cursor.get_str().expect("status response is a string");
            assert!(
                cursor.is_empty(),
                "{version}: trailing bytes after the string"
            );

            let value: serde_json::Value =
                serde_json::from_str(&json).expect("status response is valid JSON");
            assert_eq!(
                value["version"]["protocol"].as_i64(),
                Some(i64::from(version.protocol_version())),
                "{version}: protocol was not echoed back"
            );
            assert_eq!(
                value["version"]["name"].as_str(),
                Some(version.to_string().as_str()),
                "{version}: version name was not rewritten"
            );
            assert_eq!(
                value["description"]["text"].as_str(),
                Some("A Pumpkin Server")
            );
            assert_eq!(value["players"]["max"].as_i64(), Some(20));
        }
    }

    #[test]
    fn a_payload_that_is_not_a_status_response_is_left_alone() {
        let version = JavaMinecraftVersion::V_1_16_2;
        assert!(rewrite_status_response(&[0xFF, 0xFF], version).is_none());
        assert!(rewrite_status_response(&payload("not json"), version).is_none());
        assert!(rewrite_status_response(&payload(r#"{"players":{}}"#), version).is_none());
        assert!(rewrite_status_response(&payload(r#"{"version":7}"#), version).is_none());
    }
}
