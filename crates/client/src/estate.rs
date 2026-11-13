//! The estate: its info (`estateupdateinfo`, the answer to the EstateOwnerMessage `getinfo`), its
//! access lists (the EstateAccess capability), and the bodies of its changes.
//!
//! Port of parts of aurora-net/src/estate.rs (Aurora Viewer, see NOTICE.md), from
//! LLFloaterRegionInfo and LLEstateInfoModel (newview/llfloaterregioninfo.cpp,
//! llestateinfomodel.cpp, originally LGPL 2.1).

use ferro_llsd::{Llsd, llsd_map};
use uuid::Uuid;

/// `REGION_FLAGS_*` bits the estate uses (llregionflags.h).
pub mod estate_flags {
    pub const SUN_FIXED: u64 = 1 << 4;
    pub const ALLOW_ACCESS_OVERRIDE: u64 = 1 << 5;
    pub const ALLOW_ENVIRONMENT_OVERRIDE: u64 = 1 << 9;
    pub const EXTERNALLY_VISIBLE: u64 = 1 << 15;
    pub const ALLOW_DIRECT_TELEPORT: u64 = 1 << 20;
    pub const DENY_ANONYMOUS: u64 = 1 << 23;
    pub const ALLOW_VOICE: u64 = 1 << 28;
    pub const DENY_AGEUNVERIFIED: u64 = 1 << 30;
    pub const DENY_BOTS: u64 = 1 << 31;
}

/// `ESTATE_ACCESS_*` flags of `estateaccessdelta` (llregionflags.h).
pub mod estate_access {
    pub const APPLY_TO_ALL_ESTATES: u32 = 1 << 0;
    pub const APPLY_TO_MANAGED_ESTATES: u32 = 1 << 1;
    pub const BANNED_AGENT_ADD: u32 = 1 << 6;
    pub const BANNED_AGENT_REMOVE: u32 = 1 << 7;
    pub const MANAGER_ADD: u32 = 1 << 8;
    pub const MANAGER_REMOVE: u32 = 1 << 9;
}

/// The estate of `estateupdateinfo` (LLEstateInfoModel::update reads the first five strings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EstateInfo {
    pub name: String,
    pub owner: Uuid,
    pub id: u32,
    pub flags: u64,
    /// The fixed sun hour (the string carries it times 1024).
    pub sun_hour: f32,
}

impl EstateInfo {
    pub fn has(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }

    pub fn set(&mut self, flag: u64, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }
}

/// LLDispatcher::unpackMessage: a parameter loses its closing NUL.
pub fn parameter_string(bytes: &[u8]) -> String {
    let b = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

/// `estateupdateinfo`'s strings: name, owner, id, flags, sun hour × 1024, then fields the viewer
/// does not read. None when they are too few.
pub fn parse_estate_update_info(strings: &[String]) -> Option<EstateInfo> {
    if strings.len() < 5 {
        return None;
    }
    let number = |s: &str| s.trim().parse::<u64>().ok();
    Some(EstateInfo {
        name: strings[0].clone(),
        owner: Uuid::parse_str(strings[1].trim()).unwrap_or_default(),
        id: number(&strings[2]).unwrap_or(0) as u32,
        flags: number(&strings[3]).unwrap_or(0),
        sun_hour: strings[4]
            .trim()
            .parse::<f64>()
            .map_or(0.0, |v| (v / 1024.0) as f32),
    })
}

/// `estatechangeinfo`'s strings (LLEstateInfoModel::commitEstateInfoDataserver): name, flags
/// (`"%u"`), sun hour × 1024 (`"%d"`).
pub fn estate_change_strings(e: &EstateInfo) -> Vec<String> {
    vec![
        e.name.clone(),
        (e.flags as u32).to_string(),
        ((e.sun_hour * 1024.0) as i32).to_string(),
    ]
}

/// The EstateChangeInfo capability's body (commitEstateInfoCapsCoro), with the invoice of the last
/// `getinfo`.
pub fn estate_change_body(e: &EstateInfo, invoice: Uuid) -> Llsd {
    use estate_flags as f;
    llsd_map! {
        "estate_name" => e.name.as_str(),
        "sun_hour" => f64::from(e.sun_hour),
        "is_sun_fixed" => e.has(f::SUN_FIXED),
        "is_externally_visible" => e.has(f::EXTERNALLY_VISIBLE),
        "allow_direct_teleport" => e.has(f::ALLOW_DIRECT_TELEPORT),
        "deny_anonymous" => e.has(f::DENY_ANONYMOUS),
        "deny_age_unverified" => e.has(f::DENY_AGEUNVERIFIED),
        "block_bots" => e.has(f::DENY_BOTS),
        "allow_voice_chat" => e.has(f::ALLOW_VOICE),
        "override_public_access" => e.has(f::ALLOW_ACCESS_OVERRIDE),
        "override_environment" => e.has(f::ALLOW_ENVIRONMENT_OVERRIDE),
        "invoice" => invoice,
    }
}

/// The estate's lists of the EstateAccess capability: Managers of `{agent_id}`, BannedAgents of
/// `{id, …}` (requestEstateGetAccessCoro).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EstateAccess {
    pub managers: Vec<Uuid>,
    pub banned: Vec<Uuid>,
}

pub fn parse_estate_access(v: &Llsd) -> EstateAccess {
    let ids = |key: &str, field: &str| -> Vec<Uuid> {
        let list = &v[key];
        (0..list.len())
            .map(|i| list.at(i)[field].as_uuid())
            .filter(|id| !id.is_nil())
            .collect()
    };
    EstateAccess {
        managers: ids("Managers", "agent_id"),
        banned: ids("BannedAgents", "id"),
    }
}

/// `estateaccessdelta`'s strings (sendEstateAccessDelta): who asks, the flags, who changes.
pub fn access_delta_strings(agent: Uuid, flags: u32, id: Uuid) -> Vec<String> {
    vec![agent.to_string(), flags.to_string(), id.to_string()]
}

/// `simulatormessage`'s strings (LLPanelRegionGeneralInfo::onMessageCommit): grid x and y
/// (unused, "-1"), the sender, their name, the text.
pub fn region_message_strings(agent: Uuid, name: &str, text: &str) -> Vec<String> {
    vec![
        "-1".to_owned(),
        "-1".to_owned(),
        agent.to_string(),
        name.to_owned(),
        text.to_owned(),
    ]
}

/// The region's rules of the General panel, as "Apply" sends them (sendUpdate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionRules {
    pub block_terraform: bool,
    pub block_fly: bool,
    pub block_fly_over: bool,
    pub allow_damage: bool,
    pub restrict_pushobject: bool,
    pub allow_land_resell: bool,
    pub allow_parcel_changes: bool,
    pub block_parcel_search: bool,
    pub agent_limit: f32,
    pub object_bonus: f32,
    /// SIM_ACCESS_PG (13), _MATURE (21) or _ADULT (42).
    pub sim_access: u8,
}

/// LLFloaterRegionInfo's estate messages write their numbers with C's `"%f"`.
fn c_float(v: f32) -> String {
    format!("{v:.6}")
}

fn yes_no(v: bool) -> String {
    if v { "Y" } else { "N" }.to_owned()
}

impl RegionRules {
    /// `setregioninfo`'s strings, without the capability: terraform, fly, damage, resell, agent
    /// limit, object bonus, access, push, subdivide.
    pub fn strings(&self) -> Vec<String> {
        vec![
            yes_no(self.block_terraform),
            yes_no(self.block_fly),
            yes_no(self.allow_damage),
            yes_no(self.allow_land_resell),
            c_float(self.agent_limit),
            c_float(self.object_bonus),
            self.sim_access.to_string(),
            yes_no(self.restrict_pushobject),
            yes_no(self.allow_parcel_changes),
        ]
    }

    /// The DispatchRegionInfo capability's body.
    pub fn body(&self) -> Llsd {
        llsd_map! {
            "block_terraform" => self.block_terraform,
            "block_fly" => self.block_fly,
            "block_fly_over" => self.block_fly_over,
            "allow_damage" => self.allow_damage,
            "allow_land_resell" => self.allow_land_resell,
            "agent_limit" => f64::from(self.agent_limit),
            "prim_bonus" => f64::from(self.object_bonus),
            "sim_access" => i32::from(self.sim_access),
            "restrict_pushobject" => self.restrict_pushobject,
            "allow_parcel_changes" => self.allow_parcel_changes,
            "block_parcel_search" => self.block_parcel_search,
        }
    }
}

/// The estate's covenant (EstateCovenantReply).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Covenant {
    pub id: Uuid,
    pub timestamp: u32,
    pub estate_name: String,
    pub estate_owner: Uuid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estate_info_as_the_viewer_reads_it() {
        let s: Vec<String> = [
            "Test Estate",
            &Uuid::from_u128(5).to_string(),
            "101",
            "1048592",
            "6144",
            "x",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let e = parse_estate_update_info(&s).expect("an estate");
        assert_eq!(
            (e.name.as_str(), e.id, e.sun_hour),
            ("Test Estate", 101, 6.0)
        );
        assert!(e.has(estate_flags::SUN_FIXED) && e.has(estate_flags::ALLOW_DIRECT_TELEPORT));
        assert_eq!(
            estate_change_strings(&e),
            ["Test Estate", "1048592", "6144"]
        );
        assert!(parse_estate_update_info(&s[..3]).is_none());
        assert_eq!(parameter_string(b"name\0"), "name");
    }

    #[test]
    fn region_rules_in_both_forms() {
        let r = RegionRules {
            block_terraform: true,
            block_fly: false,
            block_fly_over: false,
            allow_damage: true,
            restrict_pushobject: false,
            allow_land_resell: true,
            allow_parcel_changes: true,
            block_parcel_search: false,
            agent_limit: 40.0,
            object_bonus: 1.5,
            sim_access: 21,
        };
        assert_eq!(
            r.strings(),
            ["Y", "N", "Y", "Y", "40.000000", "1.500000", "21", "N", "Y"]
        );
        assert_eq!(r.body()["sim_access"].as_i32(), 21);
    }
}
