//! The test region's parcel: what ParcelProperties says of it (on the event queue, as the grid
//! sends it), and what ParcelPropertiesUpdate changes.

use ferro_llsd::{Llsd, Map};
use uuid::Uuid;

/// The parcel's global key (RemoteParcelRequest).
pub const PARCEL_ID: Uuid = Uuid::from_u128(0x9a2c_0001);
pub const PARCEL_LOCAL_ID: i32 = 7;
pub const DWELL: f32 = 42.5;

#[derive(Debug, Clone, PartialEq)]
pub struct GridParcel {
    pub name: String,
    pub desc: String,
    pub flags: u32,
    pub sale_price: i32,
    pub auth_buyer: Uuid,
    pub music_url: String,
    pub media_url: String,
    pub snapshot_id: Uuid,
    pub landing_type: i32,
    pub user_location: [f64; 3],
    pub group_id: Uuid,
    pub see_avs: bool,
    pub any_av_sounds: bool,
    pub group_av_sounds: bool,
}

impl Default for GridParcel {
    fn default() -> Self {
        GridParcel {
            name: "Plaza".into(),
            desc: "The test parcel".into(),
            // AllowFly, AllowLandmark, CreateObjects
            flags: 1 | 8 | 64,
            sale_price: 0,
            auth_buyer: Uuid::nil(),
            music_url: String::new(),
            media_url: String::new(),
            snapshot_id: Uuid::nil(),
            landing_type: 2,
            user_location: [0.0; 3],
            group_id: Uuid::nil(),
            see_avs: true,
            any_av_sounds: true,
            group_av_sounds: true,
        }
    }
}

fn u32_llsd(v: u32) -> Llsd {
    Llsd::Binary(v.to_be_bytes().to_vec())
}

fn vec3(v: [f64; 3]) -> Llsd {
    Llsd::Array(v.iter().map(|f| Llsd::Real(*f)).collect())
}

impl GridParcel {
    /// The ParcelProperties event queue body (LLViewerParcelMgr::processParcelProperties reads it).
    pub fn properties(&self, sequence: i32, owner: Uuid) -> Llsd {
        let mut d = Map::new();
        let mut put = |k: &str, v: Llsd| {
            d.insert(k.into(), v);
        };
        put("LocalID", PARCEL_LOCAL_ID.into());
        put("SequenceID", sequence.into());
        put("RequestResult", 0.into());
        put("Name", self.name.clone().into());
        put("Desc", self.desc.clone().into());
        put("OwnerID", owner.into());
        put("GroupID", self.group_id.into());
        put("IsGroupOwned", false.into());
        put("Area", 16384.into());
        put("AABBMin", vec3([64.0, 64.0, 0.0]));
        put("AABBMax", vec3([192.0, 192.0, 0.0]));
        put("MaxPrims", 3750.into());
        put("TotalPrims", 12.into());
        put("OwnerPrims", 10.into());
        put("GroupPrims", 0.into());
        put("OtherPrims", 2.into());
        put("ParcelPrimBonus", Llsd::Real(1.5));
        put("ParcelFlags", u32_llsd(self.flags));
        put("SalePrice", self.sale_price.into());
        put("AuthBuyerID", self.auth_buyer.into());
        put("MusicURL", self.music_url.clone().into());
        put("MediaURL", self.media_url.clone().into());
        put("SnapshotID", self.snapshot_id.into());
        put("UserLocation", vec3(self.user_location));
        put("UserLookAt", vec3([0.0; 3]));
        put("LandingType", self.landing_type.into());
        put("SeeAVs", self.see_avs.into());
        put("AnyAVSounds", self.any_av_sounds.into());
        put("GroupAVSounds", self.group_av_sounds.into());
        let mut body = Map::new();
        body.insert("ParcelData".into(), Llsd::Array(vec![Llsd::Map(d)]));
        Llsd::Map(body)
    }

    /// A ParcelPropertiesUpdate body, applied (LLParcel::unpackMessage's fields).
    pub fn apply(&mut self, b: &Llsd) {
        let u32_of = |v: &Llsd| match v {
            Llsd::Binary(x) if x.len() == 4 => u32::from_be_bytes([x[0], x[1], x[2], x[3]]),
            other => other.as_i32() as u32,
        };
        self.name = b["name"].to_string_value();
        self.desc = b["description"].to_string_value();
        self.flags = u32_of(&b["parcel_flags"]);
        self.sale_price = b["sale_price"].as_i32();
        self.auth_buyer = b["auth_buyer_id"].as_uuid();
        self.music_url = b["music_url"].to_string_value();
        self.media_url = b["media_url"].to_string_value();
        self.snapshot_id = b["snapshot_id"].as_uuid();
        self.landing_type = b["landing_type"].as_i32();
        let l = &b["user_location"];
        self.user_location = [l.at(0).as_f64(), l.at(1).as_f64(), l.at(2).as_f64()];
        self.group_id = b["group_id"].as_uuid();
        self.see_avs = b["see_avs"].as_bool();
        self.any_av_sounds = b["any_av_sounds"].as_bool();
        self.group_av_sounds = b["group_av_sounds"].as_bool();
    }
}
