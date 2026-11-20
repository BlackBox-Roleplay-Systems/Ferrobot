//! Wearable assets: the LLWearable text every body part and piece of clothing is stored as
//! (LLWearable::exportStream), for the wearables the bot makes itself.

use std::fmt::Write;
use uuid::Uuid;

/// LLAssetType: clothing and body parts.
pub const ASSET_CLOTHING: i8 = 5;
pub const ASSET_BODYPART: i8 = 13;
/// LLInventoryType::IT_WEARABLE.
pub const INV_WEARABLE: i8 = 18;

/// LLWearableType: the kinds the probe's test outfit uses.
pub const SKIRT: u8 = 12;
pub const UNIVERSAL: u8 = 16;

/// LLAvatarAppearanceDefines' texture indices a wearable can set.
pub mod slot {
    pub const SKIRT: u8 = 18;
    pub const LEFT_ARM_TATTOO: u8 = 35;
    pub const LEFT_LEG_TATTOO: u8 = 36;
    pub const AUX1_TATTOO: u8 = 37;
    pub const AUX2_TATTOO: u8 = 38;
    pub const AUX3_TATTOO: u8 = 39;
}

/// A wearable of `kind`, its visual parameters and textures (index, texture), every permission
/// for its owner and the next owner.
pub fn asset(name: &str, kind: u8, params: &[(u32, f32)], textures: &[(u8, Uuid)]) -> Vec<u8> {
    let mut s = String::new();
    let _ = writeln!(s, "LLWearable version 22");
    let _ = writeln!(s, "{}", name.replace('\n', " "));
    let _ = writeln!(s);
    let _ = writeln!(s, "\tpermissions 0");
    let _ = writeln!(s, "\t{{");
    for (k, v) in [
        ("base_mask", "7fffffff"),
        ("owner_mask", "7fffffff"),
        ("group_mask", "00000000"),
        ("everyone_mask", "00000000"),
        ("next_owner_mask", "7fffffff"),
    ] {
        let _ = writeln!(s, "\t\t{k}\t{v}");
    }
    let _ = writeln!(s, "\t}}");
    let _ = writeln!(s, "\tsale_info\t0");
    let _ = writeln!(s, "\t{{");
    let _ = writeln!(s, "\t\tsale_type\tnot");
    let _ = writeln!(s, "\t\tsale_price\t10");
    let _ = writeln!(s, "\t}}");
    let _ = writeln!(s, "type {kind}");
    let _ = writeln!(s, "parameters {}", params.len());
    for (id, v) in params {
        let _ = writeln!(s, "{id} {v}");
    }
    let _ = writeln!(s, "textures {}", textures.len());
    for (index, id) in textures {
        let _ = writeln!(s, "{index} {id}");
    }
    s.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wearable_names_its_type_parameters_and_textures() {
        let tex = Uuid::from_u128(7);
        let text = String::from_utf8(asset(
            "Probe layer",
            UNIVERSAL,
            &[(1, 0.5)],
            &[(slot::AUX1_TATTOO, tex)],
        ))
        .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "LLWearable version 22");
        assert_eq!(lines[1], "Probe layer");
        assert!(lines.contains(&"type 16"));
        assert!(lines.contains(&"parameters 1"));
        assert!(lines.contains(&"1 0.5"));
        assert!(lines.contains(&"textures 1"));
        assert!(lines.contains(&format!("37 {tex}").as_str()));
    }
}
