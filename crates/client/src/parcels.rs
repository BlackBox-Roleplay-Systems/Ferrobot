//! Parcels: the record of one, read from ParcelProperties, the body that changes it, and access
//! list entries.
//!
//! Port of parts of aurora-net/src/land.rs and of ParcelInfo / ParcelMedia in
//! aurora-net/src/types.rs (Aurora Viewer, see NOTICE.md), themselves from LLViewerParcelMgr
//! (newview/llviewerparcelmgr.cpp) and LLParcel (llinventory/llparcel.cpp), originally LGPL 2.1.

use ferro_llsd::{Llsd, Map};
use glam::Vec3;
use uuid::Uuid;

/// Access list kinds (llparcelflags.h AL_*).
pub const AL_ACCESS: u32 = 1 << 0;
pub const AL_BAN: u32 = 1 << 1;
/// Entries per ParcelAccessListUpdate (PARCEL_MAX_ENTRIES_PER_PACKET).
pub const PARCEL_MAX_ENTRIES_PER_PACKET: usize = 48;

/// Object return kinds (llparcelflags.h RT_*).
pub const RT_OWNER: u32 = 1 << 1;
pub const RT_GROUP: u32 = 1 << 2;
pub const RT_OTHER: u32 = 1 << 3;
pub const RT_LIST: u32 = 1 << 4;

/// PF_FOR_SALE (llparcelflags.h)
pub const PF_FOR_SALE: u32 = 1 << 2;

/// ParcelExtendedFlags bit: media on prims outside the parcel stays hidden.
const PEF_OBSCURE_MOAP: u32 = 1;

/// LibreMetaverse's ParcelFlags names, by bit (SecondBot prints and reads these).
pub const FLAG_NAMES: [(u32, &str); 31] = [
    (1 << 0, "AllowFly"),
    (1 << 1, "AllowOtherScripts"),
    (1 << 2, "ForSale"),
    (1 << 3, "AllowLandmark"),
    (1 << 4, "AllowTerraform"),
    (1 << 5, "AllowDamage"),
    (1 << 6, "CreateObjects"),
    (1 << 7, "ForSaleObjects"),
    (1 << 8, "UseAccessGroup"),
    (1 << 9, "UseAccessList"),
    (1 << 10, "UseBanList"),
    (1 << 11, "UsePassList"),
    (1 << 12, "ShowDirectory"),
    (1 << 13, "AllowDeedToGroup"),
    (1 << 14, "ContributeWithDeed"),
    (1 << 15, "SoundLocal"),
    (1 << 16, "SellParcelObjects"),
    (1 << 17, "AllowPublish"),
    (1 << 18, "MaturePublish"),
    (1 << 19, "UrlWebPage"),
    (1 << 20, "UrlRawHtml"),
    (1 << 21, "RestrictPushObject"),
    (1 << 22, "DenyAnonymous"),
    (1 << 23, "LindenHome"),
    (1 << 25, "AllowGroupScripts"),
    (1 << 26, "CreateGroupObjects"),
    (1 << 27, "AllowAPrimitiveEntry"),
    (1 << 28, "AllowGroupObjectEntry"),
    (1 << 29, "AllowVoiceChat"),
    (1 << 30, "UseEstateVoiceChan"),
    (1 << 31, "DenyAgeUnverified"),
];

/// Parcel media settings (LLParcel::unpackMessage).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParcelMedia {
    pub media_id: Uuid,
    pub auto_scale: bool,
    /// MIME type; `video/vnd.secondlife.qt.legacy` when the block is missing.
    pub mime: String,
    pub desc: String,
    pub width: i32,
    pub height: i32,
    pub looping: bool,
    pub current_url: String,
    pub allow_navigate: bool,
    pub prevent_camera_zoom: bool,
    pub url_timeout: f32,
    pub obscure_moap: bool,
}

/// A parcel, as ParcelProperties tells it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParcelInfo {
    pub local_id: i32,
    /// SequenceID of the message (0: the agent's own parcel).
    pub sequence_id: i32,
    pub request_result: i32,
    pub name: String,
    pub desc: String,
    pub owner_id: Uuid,
    pub group_id: Uuid,
    pub is_group_owned: bool,
    pub status: u8,
    pub area: i32,
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
    pub max_prims: i32,
    pub total_prims: i32,
    pub owner_prims: i32,
    pub group_prims: i32,
    pub other_prims: i32,
    pub prim_bonus: f32,
    pub other_clean_time: i32,
    /// PF_* flags (llparcelflags.h).
    pub flags: u32,
    pub sale_price: i32,
    pub auth_buyer: Uuid,
    pub category: i32,
    pub pass_price: i32,
    pub pass_hours: f32,
    pub music_url: String,
    pub media_url: String,
    pub snapshot_id: Uuid,
    pub user_location: Vec3,
    pub user_look_at: Vec3,
    /// LLParcel::ELandingType: 0 blocked, 1 landing point, 2 anywhere.
    pub landing_type: i32,
    pub see_avatars: bool,
    pub any_av_sounds: bool,
    pub group_av_sounds: bool,
    pub media: ParcelMedia,
}

/// U32 fields travel as 4-byte big-endian binaries in LLSD.
pub fn u32_of(v: &Llsd) -> u32 {
    match v {
        Llsd::Binary(b) if b.len() == 4 => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        other => other.as_i32() as u32,
    }
}

/// The ParcelProperties event queue message (LLViewerParcelMgr::processParcelProperties and
/// LLParcel::unpackMessage).
pub fn parse_parcel_properties(b: &Llsd) -> ParcelInfo {
    let pd = &b["ParcelData"][0];
    let vec3 = |v: &Llsd| -> Vec3 {
        let r = Vec3::new(v.at(0).as_f32(), v.at(1).as_f32(), v.at(2).as_f32());
        if r.is_finite() { r } else { Vec3::ZERO }
    };
    let has_md = b.has("MediaData");
    let md = &b["MediaData"][0];
    let ls = &b["MediaLinkSharing"][0];
    // SeeAVs / AnyAVSounds / GroupAVSounds come from newer servers; missing, they are true
    let new_limits = pd.has("SeeAVs") && pd.has("AnyAVSounds") && pd.has("GroupAVSounds");
    let limit = |k: &str| !new_limits || pd[k].as_bool();
    ParcelInfo {
        local_id: pd["LocalID"].as_i32(),
        sequence_id: pd["SequenceID"].as_i32(),
        request_result: pd["RequestResult"].as_i32(),
        name: pd["Name"].to_string_value(),
        desc: pd["Desc"].to_string_value(),
        owner_id: pd["OwnerID"].as_uuid(),
        group_id: pd["GroupID"].as_uuid(),
        is_group_owned: pd["IsGroupOwned"].as_bool(),
        status: pd["Status"].as_i32() as u8,
        area: pd["Area"].as_i32(),
        aabb_min: vec3(&pd["AABBMin"]),
        aabb_max: vec3(&pd["AABBMax"]),
        max_prims: pd["MaxPrims"].as_i32(),
        total_prims: pd["TotalPrims"].as_i32(),
        owner_prims: pd["OwnerPrims"].as_i32(),
        group_prims: pd["GroupPrims"].as_i32(),
        other_prims: pd["OtherPrims"].as_i32(),
        prim_bonus: if pd.has("ParcelPrimBonus") {
            pd["ParcelPrimBonus"].as_f32()
        } else {
            1.0
        },
        other_clean_time: pd["OtherCleanTime"].as_i32(),
        flags: u32_of(&pd["ParcelFlags"]),
        sale_price: pd["SalePrice"].as_i32(),
        auth_buyer: pd["AuthBuyerID"].as_uuid(),
        category: pd["Category"].as_i32(),
        pass_price: pd["PassPrice"].as_i32(),
        pass_hours: pd["PassHours"].as_f32(),
        music_url: pd["MusicURL"].to_string_value(),
        media_url: pd["MediaURL"].to_string_value(),
        snapshot_id: pd["SnapshotID"].as_uuid(),
        user_location: vec3(&pd["UserLocation"]),
        user_look_at: vec3(&pd["UserLookAt"]),
        landing_type: pd["LandingType"].as_i32(),
        see_avatars: limit("SeeAVs"),
        any_av_sounds: limit("AnyAVSounds"),
        group_av_sounds: limit("GroupAVSounds"),
        media: ParcelMedia {
            media_id: pd["MediaID"].as_uuid(),
            auto_scale: pd["MediaAutoScale"].as_bool(),
            // no MediaData block: the legacy QuickTime type, looping
            mime: if has_md {
                md["MediaType"].to_string_value()
            } else {
                "video/vnd.secondlife.qt.legacy".into()
            },
            desc: md["MediaDesc"].to_string_value(),
            width: md["MediaWidth"].as_i32(),
            height: md["MediaHeight"].as_i32(),
            looping: !has_md || md["MediaLoop"].as_bool(),
            current_url: ls["MediaCurrentURL"].to_string_value(),
            allow_navigate: ls["MediaAllowNavigate"].as_bool(),
            prevent_camera_zoom: ls["MediaPreventCameraZoom"].as_bool(),
            url_timeout: ls["MediaURLTimeout"].as_f32(),
            obscure_moap: u32_of(&b["ParcelExtendedFlags"][0]["Flags"]) & PEF_OBSCURE_MOAP != 0,
        },
    }
}

fn vec3_llsd(v: Vec3) -> Llsd {
    Llsd::Array(vec![
        Llsd::Real(f64::from(v.x)),
        Llsd::Real(f64::from(v.y)),
        Llsd::Real(f64::from(v.z)),
    ])
}

/// The body of the ParcelPropertiesUpdate capability (LLParcel::packMessage, message flags 0x01:
/// the region answers with the parcel). It keeps everything of `p` but what the caller changed.
pub fn update_body(p: &ParcelInfo) -> Llsd {
    let mut m = Map::new();
    let mut put = |k: &str, v: Llsd| {
        m.insert(k.into(), v);
    };
    put("flags", Llsd::Binary(1u32.to_be_bytes().to_vec()));
    put("local_id", p.local_id.into());
    put("parcel_flags", Llsd::Binary(p.flags.to_be_bytes().to_vec()));
    put("sale_price", p.sale_price.into());
    put("name", p.name.clone().into());
    put("description", p.desc.clone().into());
    put("music_url", p.music_url.clone().into());
    put("media_url", p.media_url.clone().into());
    put("media_desc", p.media.desc.clone().into());
    put("media_type", p.media.mime.clone().into());
    put("media_width", p.media.width.into());
    put("media_height", p.media.height.into());
    put("auto_scale", p.media.auto_scale.into());
    put("media_loop", p.media.looping.into());
    put("media_current_url", p.media.current_url.clone().into());
    // obsolete, still sent by the viewer
    put("obscure_media", false.into());
    put("obscure_music", false.into());
    put("media_id", p.media.media_id.into());
    put("media_allow_navigate", p.media.allow_navigate.into());
    put(
        "media_prevent_camera_zoom",
        p.media.prevent_camera_zoom.into(),
    );
    put(
        "media_url_timeout",
        Llsd::Real(f64::from(p.media.url_timeout)),
    );
    put("group_id", p.group_id.into());
    put("pass_price", p.pass_price.into());
    put("pass_hours", Llsd::Real(f64::from(p.pass_hours)));
    put("category", p.category.into());
    put("auth_buyer_id", p.auth_buyer.into());
    put("snapshot_id", p.snapshot_id.into());
    put("user_location", vec3_llsd(p.user_location));
    put("user_look_at", vec3_llsd(p.user_look_at));
    put("landing_type", p.landing_type.into());
    put("see_avs", p.see_avatars.into());
    put("group_av_sounds", p.group_av_sounds.into());
    put("any_av_sounds", p.any_av_sounds.into());
    put("obscure_moap", p.media.obscure_moap.into());
    Llsd::Map(m)
}

/// One entry of an access or ban list (LLAccessEntry): `time` is when it ends, 0 never.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessEntry {
    pub id: Uuid,
    pub time: i32,
    pub flags: u32,
}

/// An access list in ParcelAccessListUpdate messages (sendParcelAccessListUpdate): the number of
/// sections as the viewer counts them, and an empty list still one message with a null entry, so
/// that the region clears it.
pub fn access_list_chunks(entries: &[AccessEntry]) -> (i32, Vec<Vec<AccessEntry>>) {
    let sections = entries.len().div_ceil(PARCEL_MAX_ENTRIES_PER_PACKET) as i32;
    if entries.is_empty() {
        let null = AccessEntry {
            id: Uuid::nil(),
            time: 0,
            flags: 0,
        };
        return (sections, vec![vec![null]]);
    }
    (
        sections,
        entries
            .chunks(PARCEL_MAX_ENTRIES_PER_PACKET)
            .map(<[_]>::to_vec)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parcel_properties_as_the_viewer_reads_them() {
        let xml = r#"<llsd><map><key>ParcelData</key><array><map>
            <key>LocalID</key><integer>7</integer><key>SequenceID</key><integer>0</integer>
            <key>Name</key><string>Plaza</string><key>Desc</key><string>A square</string>
            <key>ParcelFlags</key><binary encoding="base64">AAAABQ==</binary>
            <key>AABBMin</key><array><real>0</real><real>64</real><real>0</real></array>
            <key>AABBMax</key><array><real>128</real><real>128</real><real>0</real></array>
            <key>SalePrice</key><integer>250</integer><key>TotalPrims</key><integer>12</integer>
            </map></array></map></llsd>"#;
        let v = ferro_llsd::from_xml(xml.as_bytes()).expect("llsd");
        let p = parse_parcel_properties(&v);
        assert_eq!(
            (p.local_id, p.name.as_str(), p.desc.as_str()),
            (7, "Plaza", "A square")
        );
        assert_eq!(p.flags, 5, "AllowFly and ForSale");
        assert_eq!(p.aabb_max - p.aabb_min, Vec3::new(128.0, 64.0, 0.0));
        assert_eq!((p.sale_price, p.total_prims), (250, 12));
        assert_eq!(p.prim_bonus, 1.0);
        assert!(p.see_avatars && p.media.looping, "older servers' defaults");
    }

    #[test]
    fn the_update_keeps_what_it_is_not_told_to_change() {
        let p = ParcelInfo {
            local_id: 7,
            name: "Plaza".into(),
            flags: 5,
            ..Default::default()
        };
        let b = update_body(&p);
        assert_eq!(b["local_id"].as_i32(), 7);
        assert_eq!(b["name"].as_str(), "Plaza");
        assert_eq!(u32_of(&b["parcel_flags"]), 5);
        assert_eq!(u32_of(&b["flags"]), 1);
    }

    #[test]
    fn access_lists_in_messages() {
        let e = |n: u128| AccessEntry {
            id: Uuid::from_u128(n),
            time: 0,
            flags: AL_BAN,
        };
        let (sections, chunks) = access_list_chunks(&(1..=50).map(e).collect::<Vec<_>>());
        assert_eq!((sections, chunks.len(), chunks[1].len()), (2, 2, 2));
        let (sections, chunks) = access_list_chunks(&[]);
        assert_eq!((sections, chunks[0][0].id), (0, Uuid::nil()));
    }
}
