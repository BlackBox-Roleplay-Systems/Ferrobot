//! The bot's inventory as the grid keeps it: a few folders and items, fetched through
//! FetchInventoryDescendents2 and FetchInventory2, changed by MoveInventoryItem,
//! MoveInventoryFolder and CreateInventoryFolder.

use ferro_llsd::{Llsd, Map};
use uuid::Uuid;

pub const ROOT: Uuid = Uuid::from_u128(0x1000_0000);
pub const OBJECTS: Uuid = Uuid::from_u128(0x1000_0001);
pub const NOTECARDS: Uuid = Uuid::from_u128(0x1000_0002);
pub const TRASH: Uuid = Uuid::from_u128(0x1000_0003);
pub const STUFF: Uuid = Uuid::from_u128(0x1000_0004);
pub const SUB: Uuid = Uuid::from_u128(0x1000_0005);
/// Where uploaded sounds go (FT_SOUND).
pub const SOUNDS: Uuid = Uuid::from_u128(0x1000_0006);
/// A notecard, an object, and a link to the notecard.
pub const NOTECARD: Uuid = Uuid::from_u128(0x2000_0001);
pub const BOX: Uuid = Uuid::from_u128(0x2000_0002);
pub const LINK: Uuid = Uuid::from_u128(0x2000_0003);
pub const NOTECARD_ASSET: Uuid = Uuid::from_u128(0x3000_0001);
/// A notecard made long ago, in the Notecards folder.
pub const OLD_NOTECARD: Uuid = Uuid::from_u128(0x2000_0006);
/// When it was made.
pub const LONG_AGO: i64 = 1_600_000_000;
/// The Current Outfit Folder, My Outfits and an outfit in it.
pub const COF: Uuid = Uuid::from_u128(0x1000_0010);
pub const MY_OUTFITS: Uuid = Uuid::from_u128(0x1000_0011);
pub const PARTY: Uuid = Uuid::from_u128(0x1000_0012);
pub const CLOTHES: Uuid = Uuid::from_u128(0x1000_0013);
/// What is worn: body parts (shape, skin, hair, eyes), a shirt, a hat (an object).
pub const SHAPE: Uuid = Uuid::from_u128(0x2000_0101);
pub const SKIN: Uuid = Uuid::from_u128(0x2000_0102);
pub const HAIR: Uuid = Uuid::from_u128(0x2000_0103);
pub const EYES: Uuid = Uuid::from_u128(0x2000_0104);
pub const SHIRT: Uuid = Uuid::from_u128(0x2000_0105);
pub const HAT: Uuid = Uuid::from_u128(0x2000_0106);
/// The Party outfit's own: another shirt and hair, a necklace.
pub const PARTY_SHIRT: Uuid = Uuid::from_u128(0x2000_0201);
pub const PARTY_HAIR: Uuid = Uuid::from_u128(0x2000_0202);
pub const NECKLACE: Uuid = Uuid::from_u128(0x2000_0203);
/// A gesture in the root, and what it plays.
pub const GESTURE: Uuid = Uuid::from_u128(0x2000_0004);
/// A script in the Objects folder.
pub const SCRIPT: Uuid = Uuid::from_u128(0x2000_0005);
pub const GESTURE_ASSET: Uuid = Uuid::from_u128(0x3000_0004);
pub const GESTURE_ANIMATION: Uuid = Uuid::from_u128(0x3000_0005);
pub const GESTURE_SOUND: Uuid = Uuid::from_u128(0x3000_0006);

/// The Welcome notecard's text: an embedded item in it, and long enough for several packets.
pub fn welcome_text() -> String {
    let mut t = String::from("Welcome\u{F0000} to the test grid.\n");
    for i in 0..150 {
        t.push_str(&format!("line {i:03}\n"));
    }
    t
}

/// The Wave gesture (LLMultiGesture v2): an animation, a sound, a line on channel 7, a short
/// wait.
pub fn gesture() -> Vec<u8> {
    format!(
        "2\n255\n0\n/wave\n\n4\n0\nwave\n{GESTURE_ANIMATION}\n0\n1\nding\n{GESTURE_SOUND}\n0\n2\n/7 hello from a gesture\n0\n3\n0.200000\n1\n\0"
    )
    .into_bytes()
}

/// The assets behind the items: the Welcome notecard's.
pub fn asset(id: Uuid) -> Option<Vec<u8>> {
    (id == NOTECARD_ASSET).then(|| {
        let text = welcome_text();
        format!(
            "Linden text version 2\n{{\nLLEmbeddedItems version 1\n{{\ncount 0\n}}\nText length {}\n{text}}}\n",
            text.len()
        )
        .into_bytes()
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Folder {
    pub id: Uuid,
    pub parent: Uuid,
    pub name: String,
    pub kind: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub id: Uuid,
    pub parent: Uuid,
    pub name: String,
    pub asset_type: i32,
    pub inv_type: i32,
    pub asset_id: Uuid,
    /// A wearable's type in the low byte (LLWearableType).
    pub flags: u32,
    /// When it was made (unix time); 0: when it is fetched.
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct Inventory {
    pub folders: Vec<Folder>,
    pub items: Vec<Item>,
}

/// A wearable or an object of the Clothes folder.
fn wearable(id: Uuid, name: &str, asset_type: i32, flags: u32) -> Item {
    let inv_type = if asset_type == 6 { 6 } else { 18 };
    Item {
        id,
        parent: CLOTHES,
        name: name.into(),
        asset_type,
        inv_type,
        asset_id: Uuid::from_u128(id.as_u128() + 0x1000_0000),
        flags,
        created_at: 0,
    }
}

/// A link in a folder to an item.
fn link(n: u128, folder: Uuid, to: &Item) -> Item {
    Item {
        id: Uuid::from_u128(0x2100_0000 + n),
        parent: folder,
        name: to.name.clone(),
        asset_type: 24,
        inv_type: to.inv_type,
        asset_id: to.id,
        flags: 0,
        created_at: 0,
    }
}

/// The wearables and objects, and the links of the Current Outfit Folder and of Party.
fn clothes() -> Vec<Item> {
    let worn = [
        wearable(SHAPE, "Shape", 13, 0),
        wearable(SKIN, "Skin", 13, 1),
        wearable(HAIR, "Hair", 13, 2),
        wearable(EYES, "Eyes", 13, 3),
        wearable(SHIRT, "Shirt", 5, 4),
        wearable(HAT, "Hat", 6, 0),
    ];
    let party = [
        wearable(PARTY_SHIRT, "Party Shirt", 5, 4),
        wearable(PARTY_HAIR, "Party Hair", 13, 2),
        wearable(NECKLACE, "Necklace", 6, 0),
    ];
    let mut out: Vec<Item> = Vec::new();
    for (n, i) in worn.iter().enumerate() {
        out.push(link(n as u128, COF, i));
    }
    for (n, i) in party.iter().enumerate() {
        out.push(link(100 + n as u128, PARTY, i));
    }
    out.extend(worn);
    out.extend(party);
    out
}

impl Default for Inventory {
    fn default() -> Self {
        let f = |id, parent, name: &str, kind| Folder {
            id,
            parent,
            name: name.into(),
            kind,
        };
        Inventory {
            folders: vec![
                f(ROOT, Uuid::nil(), "My Inventory", 8),
                f(OBJECTS, ROOT, "Objects", 6),
                f(NOTECARDS, ROOT, "Notecards", 7),
                f(TRASH, ROOT, "Trash", 14),
                f(SOUNDS, ROOT, "Sounds", 1),
                f(STUFF, ROOT, "Stuff", -1),
                f(SUB, STUFF, "Sub", -1),
                f(COF, ROOT, "Current Outfit", 46),
                f(MY_OUTFITS, ROOT, "My Outfits", 48),
                f(PARTY, MY_OUTFITS, "Party", 47),
                f(CLOTHES, ROOT, "Clothes", -1),
            ],
            items: vec![
                Item {
                    id: NOTECARD,
                    parent: NOTECARDS,
                    name: "Welcome".into(),
                    asset_type: 7,
                    inv_type: 7,
                    asset_id: NOTECARD_ASSET,
                    flags: 0,
                    created_at: 0,
                },
                Item {
                    id: BOX,
                    parent: OBJECTS,
                    name: "Box".into(),
                    asset_type: 6,
                    inv_type: 6,
                    asset_id: Uuid::from_u128(0x3000_0002),
                    flags: 0,
                    created_at: 0,
                },
                Item {
                    id: SCRIPT,
                    parent: ROOT,
                    name: "Hello Script".into(),
                    asset_type: 10,
                    inv_type: 10,
                    asset_id: Uuid::from_u128(0x3000_0007),
                    flags: 0,
                    created_at: 0,
                },
                Item {
                    id: GESTURE,
                    parent: ROOT,
                    name: "Wave".into(),
                    asset_type: 21,
                    inv_type: 20,
                    asset_id: GESTURE_ASSET,
                    flags: 0,
                    created_at: 0,
                },
                Item {
                    id: LINK,
                    parent: STUFF,
                    name: "Welcome".into(),
                    asset_type: 24,
                    inv_type: 7,
                    asset_id: NOTECARD,
                    flags: 0,
                    created_at: 0,
                },
            ]
            .into_iter()
            .chain(clothes())
            .chain([Item {
                id: OLD_NOTECARD,
                parent: NOTECARDS,
                name: "Old Note".into(),
                asset_type: 7,
                inv_type: 7,
                asset_id: Uuid::from_u128(0x3000_0008),
                flags: 0,
                created_at: LONG_AGO,
            }])
            .collect(),
        }
    }
}

fn item_llsd(i: &Item, owner: Uuid) -> Llsd {
    let mut m = Map::new();
    m.insert("item_id".into(), i.id.into());
    m.insert("parent_id".into(), i.parent.into());
    m.insert("name".into(), i.name.clone().into());
    m.insert("type".into(), i.asset_type.into());
    m.insert("inv_type".into(), i.inv_type.into());
    m.insert("asset_id".into(), i.asset_id.into());
    m.insert("flags".into(), (i.flags as i32).into());
    let created = if i.created_at == 0 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    } else {
        i.created_at
    };
    m.insert("created_at".into(), (created as i32).into());
    let mut perms = Map::new();
    perms.insert("owner_id".into(), owner.into());
    m.insert("permissions".into(), Llsd::Map(perms));
    Llsd::Map(m)
}

impl Inventory {
    /// FetchInventoryDescendents2's answer for these folders; unknown ones go in bad_folders.
    pub fn descendents(&self, folders: &[Uuid], owner: Uuid) -> Llsd {
        let mut out = Vec::new();
        let mut bad = Vec::new();
        for id in folders {
            if !self.folders.iter().any(|f| f.id == *id) {
                bad.push(Llsd::from(*id));
                continue;
            }
            let cats: Vec<Llsd> = self
                .folders
                .iter()
                .filter(|f| f.parent == *id)
                .map(|f| {
                    let mut m = Map::new();
                    m.insert("category_id".into(), f.id.into());
                    m.insert("parent_id".into(), f.parent.into());
                    m.insert("name".into(), f.name.clone().into());
                    m.insert("type_default".into(), f.kind.into());
                    m.insert("version".into(), 1.into());
                    Llsd::Map(m)
                })
                .collect();
            let items: Vec<Llsd> = self
                .items
                .iter()
                .filter(|i| i.parent == *id)
                .map(|i| item_llsd(i, owner))
                .collect();
            let mut m = Map::new();
            m.insert("folder_id".into(), (*id).into());
            m.insert("owner_id".into(), owner.into());
            m.insert("version".into(), 1.into());
            m.insert(
                "descendents".into(),
                ((cats.len() + items.len()) as i32).into(),
            );
            m.insert("categories".into(), Llsd::Array(cats));
            m.insert("items".into(), Llsd::Array(items));
            out.push(Llsd::Map(m));
        }
        let mut v = Map::new();
        v.insert("folders".into(), Llsd::Array(out));
        v.insert("bad_folders".into(), Llsd::Array(bad));
        Llsd::Map(v)
    }

    /// FetchInventory2's answer.
    pub fn items(&self, ids: &[Uuid], owner: Uuid) -> Llsd {
        let items: Vec<Llsd> = self
            .items
            .iter()
            .filter(|i| ids.contains(&i.id))
            .map(|i| item_llsd(i, owner))
            .collect();
        let mut v = Map::new();
        v.insert("agent_id".into(), owner.into());
        v.insert("items".into(), Llsd::Array(items));
        Llsd::Map(v)
    }
}
