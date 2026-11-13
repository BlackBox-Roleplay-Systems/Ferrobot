//! Port of parts of aurora-net/src/objects.rs (Aurora Viewer, see NOTICE.md).
//!
//! Where objects stand, from the object updates (LLViewerObject::processUpdateMessage,
//! LLViewerObjectList::processCompressedObjectUpdate, originally LGPL 2.1). A bot draws nothing:
//! it keeps only the place of the region's root objects and of the avatars, so that it knows
//! where it and the others are, also when seated on something.
//!
//! An update names an object's owner only when it carries a sound, and never its name: those
//! come from ObjectProperties, which the region sends for the objects the bot selects, and
//! ObjectPropertiesFamily (LLSelectMgr::processObjectProperties and
//! processObjectPropertiesFamily). They are asked for as Aurora's Area Search does
//! (aurora-viewer/src/app/area_search.rs, after FSAreaSearch::requestObjectProperties): the root
//! objects selected then at once deselected, in packets of 254, at most three packets unanswered,
//! a round every half second, and asked again only after 30 s without an answer.

use ferro_msg::field_str;
use ferro_msg::msgs::{
    ObjectProperties, ObjectPropertiesFamily, improved_terse_object_update, object_update,
};
use glam::{Quat, Vec3};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// What an update says about where an object is.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub local_id: u32,
    /// Full and compressed updates name the object; terse ones do not.
    pub full_id: Option<Uuid>,
    /// The object it is linked to or sits on (0: none); terse updates do not say.
    pub parent_id: Option<u32>,
    pub position: Vec3,
    pub rotation: Quat,
    /// An avatar (LL_PCODE_LEGACY_AVATAR, or a terse update's avatar flag).
    pub avatar: bool,
    /// Its owner (full and compressed updates).
    pub owner: Option<Uuid>,
    /// Its state byte: an attachment's point (full and compressed updates).
    pub state: u8,
    /// The inventory item an attachment was worn from (its AttachItemID name value).
    pub attach_item: Option<Uuid>,
    /// A sculpted prim, not a mesh (its sculpt extra parameter).
    pub sculpt: bool,
    /// The value of its first name value, which names an avatar or an attachment.
    pub label: Option<String>,
}

/// An attachment's point from the state byte (LLViewerObject::getAttachmentPointNumber: its
/// nibbles swapped), as Aurora's ObjectUpdate::attachment_point.
pub fn attachment_point(state: u8) -> u8 {
    ((state & 0xF0) >> 4) | ((state & 0x0F) << 4)
}

/// The AttachItemID of the name values ("AttachItemID STRING RW SV <key>").
fn attach_item(name_values: &str) -> Option<Uuid> {
    name_values.lines().find_map(|line| {
        let mut it = line.splitn(5, ' ');
        (it.next()? == "AttachItemID").then_some(())?;
        Uuid::parse_str(it.nth(3)?.trim()).ok()
    })
}

/// The first name value's value ("FirstName STRING RW DS Bob": "Bob"), as LibreMetaverse's
/// NameValues[0].Value.
fn first_name_value(name_values: &str) -> Option<String> {
    let line = name_values.lines().next()?;
    let value = line.splitn(5, ' ').nth(4)?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// PARAMS_SCULPT (llprimitive.h): the sculpt texture, then the type.
const PARAMS_SCULPT: u16 = 0x30;
/// LL_SCULPT_TYPE_MASK and LL_SCULPT_TYPE_MESH (llprimitive.h).
const SCULPT_TYPE_MASK: u8 = 0x07;
const SCULPT_TYPE_MESH: u8 = 5;

/// Whether extra parameters (LLViewerObject::unpackParameterEntry: a count, then each entry's
/// type, size and data) make a sculpted prim: a sculpt texture and a sculpt type that is not
/// mesh, as LibreMetaverse's Primitive.Type tells PrimType.Sculpt.
fn sculpt_entry(r: &mut Reader<'_>) -> Option<bool> {
    let mut sculpt = false;
    for _ in 0..r.u8()? {
        let kind = r.u16()?;
        let data = r.binary()?;
        if kind == PARAMS_SCULPT && data.len() >= 17 {
            let texture = Uuid::from_slice(&data[..16]).ok()?;
            let t = data[16] & SCULPT_TYPE_MASK;
            sculpt = !texture.is_nil() && t != 0 && t != SCULPT_TYPE_MESH;
        }
    }
    Some(sculpt)
}

/// LL_PCODE_LEGACY_AVATAR (llprimitive/object_flags.h)
const PCODE_AVATAR: u8 = 47;

/// A little-endian reader over an update's data, as Aurora's `Dp`.
struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    fn vec3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn uuid(&mut self) -> Option<Uuid> {
        Uuid::from_slice(self.take(16)?).ok()
    }
    /// A NUL-terminated string.
    fn cstr(&mut self) -> Option<String> {
        let rest = self.d.get(self.p..)?;
        let end = rest.iter().position(|&c| c == 0)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.p += end + 1;
        Some(s)
    }
    /// An S32-sized binary.
    fn binary(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// Aurora's u16_to_f32 (LLViewerObject's U16_to_F32).
fn u16_to_f32(v: u16, lower: f32, upper: f32) -> f32 {
    let delta = upper - lower;
    let val = f32::from(v) / 65535.0 * delta + lower;
    // within one quantum of zero is zero
    if val.abs() < delta / 65535.0 {
        0.0
    } else {
        val
    }
}

fn sane_vec(v: Vec3) -> Vec3 {
    if v.is_finite() { v } else { Vec3::ZERO }
}

fn sane_quat(q: Quat) -> Quat {
    let n = q.normalize();
    if n.is_finite() { n } else { Quat::IDENTITY }
}

/// A full ObjectUpdate block: its ObjectData is 60 or 124 bytes, 76 or 140 for an avatar (a foot
/// plane first); the place comes first, then the velocities.
pub fn from_full(b: &object_update::ObjectData) -> Option<Placed> {
    let mut r = Reader {
        d: &b.object_data,
        p: 0,
    };
    match b.object_data.len() {
        76 | 140 => {
            r.take(16)?;
        }
        60 | 124 => {}
        _ => return None,
    }
    let position = r.vec3()?;
    let _velocity = r.vec3()?;
    let _acceleration = r.vec3()?;
    let rotation = ferro_msg::unpack_quat(r.vec3()?);
    let name_values = ferro_msg::field_str(&b.name_value);
    let mut extra = Reader {
        d: &b.extra_params,
        p: 0,
    };
    Some(Placed {
        local_id: b.id,
        full_id: Some(b.full_id),
        parent_id: Some(b.parent_id),
        position: sane_vec(position),
        rotation: sane_quat(rotation),
        avatar: b.p_code == PCODE_AVATAR,
        owner: Some(b.owner_id),
        state: b.state,
        attach_item: attach_item(&name_values),
        sculpt: sculpt_entry(&mut extra).unwrap_or(false),
        label: first_name_value(&name_values),
    })
}

/// An ObjectUpdateCompressed blob: its fixed head, then the optional parts, of which only the
/// parent matters here.
pub fn from_compressed(data: &[u8]) -> Option<Placed> {
    let mut r = Reader { d: data, p: 0 };
    let full_id = r.uuid()?;
    let local_id = r.u32()?;
    let pcode = r.u8()?;
    let state = r.u8()?;
    let _crc = r.u32()?;
    let _material = r.u8()?;
    let _click_action = r.u8()?;
    let _scale = r.vec3()?;
    let position = r.vec3()?;
    let rotation = ferro_msg::unpack_quat(r.vec3()?);
    let flags = r.u32()?;
    let owner = r.uuid()?;
    if flags & 0x80 != 0 {
        let _angular_velocity = r.vec3()?;
    }
    let parent_id = if flags & 0x20 != 0 { r.u32()? } else { 0 };
    // what lies before the name values, skipped as Aurora's parse_compressed reads it; a blob
    // cut short only loses them
    let mut sculpt = false;
    let name_values = (|| {
        if flags & 0x2 != 0 {
            r.u8()?;
        } else if flags & 0x1 != 0 {
            r.u32()?;
            r.binary()?;
        }
        if flags & 0x4 != 0 {
            r.cstr()?;
            r.take(4)?;
        }
        if flags & 0x200 != 0 {
            r.cstr()?;
        }
        if flags & 0x8 != 0 {
            r.take(86)?;
        }
        sculpt = sculpt_entry(&mut r)?;
        if flags & 0x10 != 0 {
            r.take(16 + 4 + 1 + 4)?;
        }
        if flags & 0x100 != 0 {
            return r.cstr();
        }
        None
    })()
    .unwrap_or_default();
    Some(Placed {
        local_id,
        full_id: Some(full_id),
        parent_id: Some(parent_id),
        position: sane_vec(position),
        rotation: sane_quat(rotation),
        avatar: pcode == PCODE_AVATAR,
        owner: Some(owner),
        state,
        attach_item: attach_item(&name_values),
        sculpt,
        label: first_name_value(&name_values),
    })
}

/// An ImprovedTerseObjectUpdate block: a move.
pub fn from_terse(b: &improved_terse_object_update::ObjectData) -> Option<Placed> {
    let mut r = Reader { d: &b.data, p: 0 };
    let local_id = r.u32()?;
    let _state = r.u8()?;
    let avatar = r.u8()? != 0;
    if avatar {
        // its foot plane
        r.take(16)?;
    }
    let position = r.vec3()?;
    let mut u = [0u16; 10];
    for v in &mut u {
        *v = r.u16()?;
    }
    // velocity and acceleration (6), then the rotation (4)
    let q = |i: usize| u16_to_f32(u[i], -1.0, 1.0);
    Some(Placed {
        local_id,
        full_id: None,
        parent_id: None,
        position: sane_vec(position),
        rotation: sane_quat(Quat::from_xyzw(q(6), q(7), q(8), q(9))),
        avatar,
        owner: None,
        state: 0,
        attach_item: None,
        sculpt: false,
        label: None,
    })
}

#[derive(Debug, Clone, Copy)]
struct Place {
    full_id: Option<Uuid>,
    parent: u32,
    position: Vec3,
    rotation: Quat,
}

/// What the region tells of an object when asked: who owns it, and its name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Props {
    /// Nil from ObjectPropertiesFamily, which does not carry it.
    pub creator: Uuid,
    /// The group, for an object deeded to a group.
    pub owner: Uuid,
    pub group: Uuid,
    pub last_owner: Uuid,
    pub name: String,
    pub description: String,
}

/// The objects an ObjectProperties tells of (the ones the bot selected).
pub fn from_properties(m: &ObjectProperties) -> Vec<(Uuid, Props)> {
    m.object_data
        .iter()
        .map(|b| {
            (
                b.object_id,
                Props {
                    creator: b.creator_id,
                    owner: b.owner_id,
                    group: b.group_id,
                    last_owner: b.last_owner_id,
                    name: field_str(&b.name),
                    description: field_str(&b.description),
                },
            )
        })
        .collect()
}

/// The object an ObjectPropertiesFamily tells of (RequestObjectPropertiesFamily's answer).
pub fn from_family(m: &ObjectPropertiesFamily) -> (Uuid, Props) {
    let b = &m.object_data;
    (
        b.object_id,
        Props {
            creator: Uuid::nil(),
            owner: b.owner_id,
            group: b.group_id,
            last_owner: b.last_owner_id,
            name: field_str(&b.name),
            description: field_str(&b.description),
        },
    )
}

/// ObjectSelect packets of at most this many objects (Aurora's MAX_PER_PACKET and Area Search's
/// PACKET).
pub const SELECT_PACKET: usize = 254;
/// At most this many objects asked about and not answered (Area Search's IN_FLIGHT).
const SELECT_IN_FLIGHT: usize = SELECT_PACKET * 3;
/// An object asked about and not answered for this long may be asked again (Area Search's
/// TIMEOUT).
const SELECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Between two rounds of requests (Area Search's snapshot every 500 ms).
const SELECT_SPACING: Duration = Duration::from_millis(500);

/// The objects whose properties the bot wants, and the pace at which it asks for them.
#[derive(Debug, Default)]
pub struct PropertyAsks {
    /// Waiting their turn, by local id.
    queue: VecDeque<u32>,
    queued: HashSet<u32>,
    /// Asked, and when.
    sent: HashMap<u32, Instant>,
    last_round: Option<Instant>,
}

impl PropertyAsks {
    /// Ask about this object, unless it is already waiting (one asked and still unanswered is
    /// skipped at its turn).
    pub fn want(&mut self, local_id: u32) {
        if self.queued.insert(local_id) {
            self.queue.push_back(local_id);
        }
    }

    pub fn answered(&mut self, local_id: u32) {
        self.sent.remove(&local_id);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The objects to select now, at most one packet, or nothing when the last round was too
    /// recent or too many are unanswered. `wanted` says whether one still needs asking (it may
    /// have been answered or killed since).
    pub fn round(&mut self, now: Instant, wanted: impl Fn(u32) -> bool) -> Vec<u32> {
        if self.queue.is_empty()
            || self
                .last_round
                .is_some_and(|t| now.saturating_duration_since(t) < SELECT_SPACING)
        {
            return Vec::new();
        }
        self.sent
            .retain(|id, at| now.saturating_duration_since(*at) < SELECT_TIMEOUT && wanted(*id));
        let room = SELECT_IN_FLIGHT
            .saturating_sub(self.sent.len())
            .min(SELECT_PACKET);
        let mut out = Vec::new();
        while out.len() < room {
            let Some(id) = self.queue.pop_front() else {
                break;
            };
            self.queued.remove(&id);
            if wanted(id) && !self.sent.contains_key(&id) {
                self.sent.insert(id, now);
                out.push(id);
            }
        }
        if !out.is_empty() {
            self.last_round = Some(now);
        }
        out
    }
}

/// Any object the region told of: its key, what it is linked to or sits on, its owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Known {
    pub local_id: u32,
    pub full_id: Uuid,
    pub parent: u32,
    pub owner: Uuid,
    pub state: u8,
    pub attach_item: Option<Uuid>,
    /// Where it was last placed (its offset from the root, for a child).
    pub position: Vec3,
}

/// A sculpted prim, as SecondBot's ListSculptys reports it: a linked one by its linkset's root.
#[derive(Debug, Clone, PartialEq)]
pub struct Sculpt {
    pub linked: bool,
    pub name: Option<String>,
    pub owner: Uuid,
    pub key: Uuid,
    pub position: Vec3,
}

/// Something the bot wears: where, worn from which item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Worn {
    pub point: u8,
    pub item: Uuid,
    pub local_id: u32,
    /// The attachment's own key (an RLV restriction names it).
    pub object: Uuid,
}

/// A root object and where it stands (GetObjectsInParcel).
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub full_id: Uuid,
    pub local_id: u32,
    /// Its properties' owner once known, else its updates' (nil unless it has a sound).
    pub owner: Uuid,
    pub position: Vec3,
    pub props: Option<Props>,
}

/// The places the bot keeps: the region's root objects and the avatars; and every object by its
/// key, children and attachments too.
#[derive(Debug, Default)]
pub struct Places {
    objects: HashMap<u32, Place>,
    known: HashMap<u32, Known>,
    by_key: HashMap<Uuid, u32>,
    /// The avatars among them, the bot's own too.
    avatars: HashSet<u32>,
    /// The bot's avatar in this region, once a full update has named it.
    own: Option<u32>,
    /// The sculpted prims among the known objects.
    sculpts: HashSet<u32>,
    /// Their first name values, when they have one.
    labels: HashMap<u32, String>,
    /// What ObjectProperties and ObjectPropertiesFamily told of them.
    props: HashMap<u32, Props>,
}

impl Places {
    /// An update from the region. A seated avatar is a child of the linkset's root, so keeping
    /// the roots is enough to place it.
    pub fn update(&mut self, p: &Placed, agent: Uuid) {
        if p.full_id == Some(agent) {
            self.own = Some(p.local_id);
        }
        if p.avatar && p.parent_id.is_some() {
            self.avatars.insert(p.local_id);
        }
        if let (Some(full_id), Some(parent)) = (p.full_id, p.parent_id) {
            if let Some(old) = self.known.get(&p.local_id)
                && old.full_id != full_id
            {
                self.by_key.remove(&old.full_id);
                self.props.remove(&p.local_id);
            }
            // an update that names another owner than the properties: it changed hands
            if let Some(owner) = p.owner.filter(|o| !o.is_nil())
                && self
                    .props
                    .get(&p.local_id)
                    .is_some_and(|props| props.owner != owner)
            {
                self.props.remove(&p.local_id);
            }
            self.known.insert(
                p.local_id,
                Known {
                    local_id: p.local_id,
                    full_id,
                    parent,
                    owner: p.owner.unwrap_or_default(),
                    state: p.state,
                    attach_item: p.attach_item,
                    position: p.position,
                },
            );
            self.by_key.insert(full_id, p.local_id);
            if p.sculpt {
                self.sculpts.insert(p.local_id);
            } else {
                self.sculpts.remove(&p.local_id);
            }
            match &p.label {
                Some(l) => self.labels.insert(p.local_id, l.clone()),
                None => self.labels.remove(&p.local_id),
            };
        }
        let avatar = self.avatars.contains(&p.local_id);
        match p.parent_id {
            Some(parent) if parent == 0 || avatar => {
                self.objects.insert(
                    p.local_id,
                    Place {
                        full_id: p.full_id,
                        parent,
                        position: p.position,
                        rotation: p.rotation,
                    },
                );
            }
            // a child prim: not kept, and no longer a root if it was one
            Some(_) => {
                self.objects.remove(&p.local_id);
            }
            // a terse update moves what is known
            None => {
                if let Some(o) = self.objects.get_mut(&p.local_id) {
                    o.position = p.position;
                    o.rotation = p.rotation;
                }
            }
        }
    }

    pub fn kill(&mut self, local_id: u32) {
        self.objects.remove(&local_id);
        self.avatars.remove(&local_id);
        if let Some(k) = self.known.remove(&local_id) {
            self.by_key.remove(&k.full_id);
        }
        self.sculpts.remove(&local_id);
        self.labels.remove(&local_id);
        self.props.remove(&local_id);
    }

    /// An object's properties, from the region; its local id when the object is known.
    pub fn set_props(&mut self, key: Uuid, props: Props) -> Option<u32> {
        let local_id = *self.by_key.get(&key)?;
        self.props.insert(local_id, props);
        Some(local_id)
    }

    /// A root object (not an avatar, nor anything attached or linked) whose properties the bot
    /// does not know yet: its local id.
    pub fn unnamed_root(&self, key: Uuid) -> Option<u32> {
        let id = *self.by_key.get(&key)?;
        self.lacks_props(id).then_some(id)
    }

    /// Whether this object is a known root without properties yet.
    pub fn lacks_props(&self, local_id: u32) -> bool {
        self.known.get(&local_id).is_some_and(|k| k.parent == 0)
            && !self.avatars.contains(&local_id)
            && !self.props.contains_key(&local_id)
    }

    /// The best known owner: the properties', else the updates'.
    fn owner_of(&self, k: &Known) -> Uuid {
        self.props.get(&k.local_id).map_or(k.owner, |p| p.owner)
    }

    /// The first name value, else the properties' name.
    fn name_of(&self, local_id: u32) -> Option<String> {
        self.labels
            .get(&local_id)
            .cloned()
            .or_else(|| self.props.get(&local_id).map(|p| p.name.clone()))
    }

    pub fn clear(&mut self) {
        self.objects.clear();
        self.avatars.clear();
        self.known.clear();
        self.by_key.clear();
        self.own = None;
        self.sculpts.clear();
        self.labels.clear();
        self.props.clear();
    }

    /// The sculpted prims (SecondBot's ListSculptys): each by itself, or by its linkset's root
    /// when the root is known (its key, owner, name and place). The name is the first name value,
    /// as SecondBot's, else the properties' name; the owner the properties', else the updates'.
    pub fn sculpts(&self) -> Vec<Sculpt> {
        self.sculpts
            .iter()
            .filter_map(|id| {
                let k = self.known.get(id)?;
                let own = Sculpt {
                    linked: false,
                    name: self.name_of(*id),
                    owner: self.owner_of(k),
                    key: k.full_id,
                    position: self.objects.get(id).map_or(k.position, |o| o.position),
                };
                if k.parent == 0 {
                    return Some(own);
                }
                let Some(root) = self.known.get(&k.parent) else {
                    return Some(own);
                };
                Some(Sculpt {
                    linked: true,
                    // SecondBot keeps the prim's own name when the root has none
                    name: self.name_of(k.parent).or(own.name),
                    owner: self.owner_of(root),
                    key: root.full_id,
                    position: self
                        .objects
                        .get(&k.parent)
                        .map_or(root.position, |o| o.position),
                })
            })
            .collect()
    }

    /// An object by its key: a prim of a linkset, an attachment, anything the region sent.
    pub fn find(&self, key: Uuid) -> Option<Known> {
        self.known.get(self.by_key.get(&key)?).copied()
    }

    /// What the bot wears: the objects attached to its avatar that name their item.
    pub fn worn(&self) -> Vec<Worn> {
        let Some(me) = self.own else {
            return Vec::new();
        };
        self.known
            .values()
            .filter(|k| k.parent == me)
            .filter_map(|k| {
                Some(Worn {
                    point: attachment_point(k.state),
                    item: k.attach_item?,
                    local_id: k.local_id,
                    object: k.full_id,
                })
            })
            .collect()
    }

    /// The root objects (not avatars) and where they stand.
    pub fn roots(&self) -> Vec<Root> {
        self.objects
            .iter()
            .filter(|(id, _)| !self.avatars.contains(id))
            .filter_map(|(id, o)| {
                let k = self.known.get(id)?;
                (o.parent == 0).then(|| Root {
                    full_id: k.full_id,
                    local_id: *id,
                    owner: self.owner_of(k),
                    position: o.position,
                    props: self.props.get(id).cloned(),
                })
            })
            .collect()
    }

    /// Where something kept stands in the region: its own place, or on what it sits on (the
    /// root's place plus the offset turned by the root's rotation, as SecondBot's
    /// AvatarPositions). None while unknown.
    fn position_of(&self, local_id: u32) -> Option<Vec3> {
        let me = self.objects.get(&local_id)?;
        if me.parent == 0 {
            return Some(me.position);
        }
        let root = self.objects.get(&me.parent)?;
        Some(root.position + root.rotation * me.position)
    }

    /// The bot's place in the region.
    pub fn own_position(&self) -> Option<Vec3> {
        self.position_of(self.own?)
    }

    /// The other avatars and where they stand (those that can be placed).
    pub fn avatar_positions(&self) -> HashMap<Uuid, Vec3> {
        self.avatars
            .iter()
            .filter(|id| Some(**id) != self.own)
            .filter_map(|id| {
                let key = self.objects.get(id)?.full_id?;
                Some((key, self.position_of(*id)?))
            })
            .collect()
    }

    /// The local id of what the bot sits on, if anything.
    pub fn own_parent(&self) -> Option<u32> {
        let me = self.objects.get(&self.own?)?;
        (me.parent != 0).then_some(me.parent)
    }

    /// What the bot sits on: None while its avatar is unknown, then standing (`Some(None)`) or
    /// seated (the seat's key, nil when the seat itself is not known).
    pub fn own_seat(&self) -> Option<Option<Uuid>> {
        let me = self.objects.get(&self.own?)?;
        if me.parent == 0 {
            return Some(None);
        }
        let seat = self.objects.get(&me.parent).and_then(|o| o.full_id);
        Some(Some(seat.unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: Uuid = Uuid::from_u128(0xa6e7);

    fn full(local_id: u32, full_id: Uuid, parent_id: u32, avatar: bool, pos: Vec3) -> Placed {
        let mut data = Vec::new();
        if avatar {
            data.extend([0u8; 16]);
        }
        for v in [pos, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO] {
            for f in v.to_array() {
                data.extend(f.to_le_bytes());
            }
        }
        let b = object_update::ObjectData {
            id: local_id,
            full_id,
            parent_id,
            p_code: if avatar { PCODE_AVATAR } else { 9 },
            object_data: data,
            ..Default::default()
        };
        from_full(&b).expect("a full update")
    }

    #[test]
    fn a_full_update_places_the_avatar() {
        let p = full(7, AGENT, 0, true, Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(p.position, Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(p.rotation, Quat::IDENTITY);
        let mut places = Places::default();
        assert_eq!(places.own_position(), None);
        assert_eq!(places.own_seat(), None);
        places.update(&p, AGENT);
        assert_eq!(places.own_seat(), Some(None));
        assert_eq!(places.own_position(), Some(Vec3::new(10.0, 20.0, 30.0)));
        // a block of another size is not read
        let b = object_update::ObjectData {
            object_data: vec![0; 10],
            ..Default::default()
        };
        assert_eq!(from_full(&b), None);
    }

    #[test]
    fn terse_updates_move_it() {
        let mut places = Places::default();
        places.update(&full(7, AGENT, 0, true, Vec3::new(1.0, 2.0, 3.0)), AGENT);
        let mut data = Vec::new();
        data.extend(7u32.to_le_bytes());
        data.extend([0, 1]);
        data.extend([0u8; 16]);
        for f in [50.0f32, 60.0, 25.0] {
            data.extend(f.to_le_bytes());
        }
        // velocities at zero, then an identity rotation
        data.extend([0xff, 0x7f].repeat(9));
        data.extend(u16::MAX.to_le_bytes());
        data.extend([0xff, 0x7f].repeat(3));
        let b = improved_terse_object_update::ObjectData {
            data,
            texture_entry: vec![],
        };
        let t = from_terse(&b).expect("a terse update");
        assert_eq!((t.local_id, t.parent_id), (7, None));
        places.update(&t, AGENT);
        assert_eq!(places.own_position(), Some(Vec3::new(50.0, 60.0, 25.0)));
        // a short block is not read
        let b = improved_terse_object_update::ObjectData {
            data: vec![1, 2, 3],
            texture_entry: vec![],
        };
        assert_eq!(from_terse(&b), None);
    }

    #[test]
    fn seated_it_is_where_its_seat_is() {
        let mut places = Places::default();
        let chair = Uuid::from_u128(0xc4a1);
        places.update(
            &full(3, chair, 0, false, Vec3::new(100.0, 100.0, 20.0)),
            AGENT,
        );
        places.update(&full(7, AGENT, 3, true, Vec3::new(0.0, 0.0, 1.0)), AGENT);
        assert_eq!(places.own_parent(), Some(3));
        assert_eq!(places.own_seat(), Some(Some(chair)));
        assert_eq!(places.own_position(), Some(Vec3::new(100.0, 100.0, 21.0)));
        // the seat gone, the place is unknown until the next update
        places.kill(3);
        assert_eq!(places.own_position(), None);
        // a child prim is not kept as a place, but known by its key
        places.update(&full(9, Uuid::from_u128(9), 3, false, Vec3::ONE), AGENT);
        assert!(!places.objects.contains_key(&9));
        assert_eq!(
            places
                .find(Uuid::from_u128(9))
                .map(|k| (k.local_id, k.parent)),
            Some((9, 3))
        );
        assert!(places.find(Uuid::from_u128(10)).is_none());
    }

    #[test]
    fn what_the_bot_wears() {
        let mut places = Places::default();
        places.update(&full(7, AGENT, 0, true, Vec3::ONE), AGENT);
        let item = Uuid::from_u128(0x17e3);
        let b = object_update::ObjectData {
            id: 20,
            full_id: Uuid::from_u128(20),
            parent_id: 7,
            p_code: 9,
            // the chest (1), nibbles swapped
            state: 0x10,
            name_value: format!("AttachItemID STRING RW SV {item}\0").into_bytes(),
            object_data: vec![0; 60],
            ..Default::default()
        };
        places.update(&from_full(&b).expect("an attachment"), AGENT);
        assert_eq!(
            places.worn(),
            vec![Worn {
                point: 1,
                item,
                local_id: 20,
                object: Uuid::from_u128(20),
            }]
        );
        assert_eq!(attachment_point(0x10), 1);
        assert_eq!(attachment_point(0x23), 0x32);
    }

    #[test]
    fn roots_are_objects_not_avatars() {
        let mut places = Places::default();
        places.update(
            &full(3, Uuid::from_u128(3), 0, false, Vec3::new(5.0, 6.0, 7.0)),
            AGENT,
        );
        places.update(&full(7, AGENT, 0, true, Vec3::ONE), AGENT);
        let roots = places.roots();
        assert_eq!(roots.len(), 1);
        assert_eq!(
            (roots[0].full_id, roots[0].position),
            (Uuid::from_u128(3), Vec3::new(5.0, 6.0, 7.0))
        );
        places.kill(3);
        assert!(places.roots().is_empty() && places.find(Uuid::from_u128(3)).is_none());
    }

    #[test]
    fn other_avatars_standing_and_seated() {
        let mut places = Places::default();
        let (a, b) = (Uuid::from_u128(0xa), Uuid::from_u128(0xb));
        places.update(&full(7, AGENT, 0, true, Vec3::new(1.0, 1.0, 1.0)), AGENT);
        places.update(
            &full(3, Uuid::from_u128(3), 0, false, Vec3::new(50.0, 50.0, 20.0)),
            AGENT,
        );
        places.update(&full(10, a, 0, true, Vec3::new(10.0, 20.0, 30.0)), AGENT);
        places.update(&full(11, b, 3, true, Vec3::new(0.0, 0.0, 1.0)), AGENT);
        let at = places.avatar_positions();
        assert_eq!(at.len(), 2, "the bot left out");
        assert_eq!(at[&a], Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(at[&b], Vec3::new(50.0, 50.0, 21.0));
        places.kill(10);
        assert!(!places.avatar_positions().contains_key(&a));
    }

    #[test]
    fn a_compressed_update_with_a_parent() {
        let mut d = Vec::new();
        d.extend(Uuid::from_u128(5).as_bytes());
        d.extend(5u32.to_le_bytes());
        d.extend([9, 0]);
        d.extend(0u32.to_le_bytes());
        d.extend([3, 0]);
        for f in [1.0f32, 1.0, 1.0, 40.0, 50.0, 60.0, 0.0, 0.0, 0.0] {
            d.extend(f.to_le_bytes());
        }
        d.extend(0x20u32.to_le_bytes());
        d.extend(Uuid::nil().as_bytes());
        d.extend(2u32.to_le_bytes());
        let p = from_compressed(&d).expect("a compressed update");
        assert_eq!(p.parent_id, Some(2));
        assert_eq!(p.position, Vec3::new(40.0, 50.0, 60.0));
        assert_eq!(from_compressed(&d[..20]), None);
        // with extra parameters that make a sculpty, then name values
        d[64..68].copy_from_slice(&(0x20u32 | 0x100).to_le_bytes());
        d.extend(sculpt_params(1));
        d.extend(b"Name STRING RW SV Lamp\0");
        let p = from_compressed(&d).expect("a compressed update");
        assert!(p.sculpt);
        assert_eq!(p.label.as_deref(), Some("Lamp"));
    }

    /// One extra parameter: the sculpt texture and this type.
    fn sculpt_params(kind: u8) -> Vec<u8> {
        let mut e = vec![1];
        e.extend(PARAMS_SCULPT.to_le_bytes());
        e.extend(17u32.to_le_bytes());
        e.extend(Uuid::from_u128(0x5c).as_bytes());
        e.push(kind);
        e
    }

    #[test]
    fn sculpties_by_themselves_or_by_their_root() {
        let sculpted = |local_id: u32, key: u128, parent: u32, kind: u8| {
            let mut data = Vec::new();
            for v in [
                Vec3::new(1.0, 2.0, 3.0),
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
            ] {
                for f in v.to_array() {
                    data.extend(f.to_le_bytes());
                }
            }
            let b = object_update::ObjectData {
                id: local_id,
                full_id: Uuid::from_u128(key),
                parent_id: parent,
                p_code: 9,
                owner_id: Uuid::from_u128(0x0e),
                object_data: data,
                extra_params: sculpt_params(kind),
                ..Default::default()
            };
            from_full(&b).expect("a full update")
        };
        let mut places = Places::default();
        // a sculpty alone, a mesh, and a sculpted child of a known root
        places.update(&sculpted(1, 0x1, 0, 1), AGENT);
        places.update(&sculpted(2, 0x2, 0, SCULPT_TYPE_MESH), AGENT);
        places.update(
            &full(3, Uuid::from_u128(0x3), 0, false, Vec3::new(9.0, 9.0, 9.0)),
            AGENT,
        );
        places.update(&sculpted(4, 0x4, 3, 2), AGENT);
        let mut found = places.sculpts();
        found.sort_by_key(|s| s.key);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].key, Uuid::from_u128(0x1));
        assert!(!found[0].linked);
        assert_eq!(found[0].owner, Uuid::from_u128(0x0e));
        assert_eq!(found[0].position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(found[1].key, Uuid::from_u128(0x3));
        assert!(found[1].linked);
        assert_eq!(found[1].position, Vec3::new(9.0, 9.0, 9.0));
        // no longer sculpted, then gone
        places.update(&full(1, Uuid::from_u128(0x1), 0, false, Vec3::ZERO), AGENT);
        places.kill(4);
        assert!(places.sculpts().is_empty());
    }

    #[test]
    fn first_name_values() {
        assert_eq!(
            first_name_value("FirstName STRING RW DS Bob\nLastName STRING RW DS Smith"),
            Some("Bob".into())
        );
        assert_eq!(first_name_value(""), None);
        assert_eq!(first_name_value("Odd"), None);
    }

    fn props(owner: u128, name: &str) -> Props {
        Props {
            owner: Uuid::from_u128(owner),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn properties_name_and_own_the_roots() {
        let mut places = Places::default();
        let (lamp, owner) = (Uuid::from_u128(3), Uuid::from_u128(0x0e));
        places.update(&full(3, lamp, 0, false, Vec3::new(5.0, 6.0, 7.0)), AGENT);
        places.update(&full(4, Uuid::from_u128(4), 3, false, Vec3::ONE), AGENT);
        places.update(&full(7, AGENT, 0, true, Vec3::ONE), AGENT);
        // only the root, not its child nor an avatar, is asked about
        assert_eq!(places.unnamed_root(lamp), Some(3));
        assert_eq!(places.unnamed_root(Uuid::from_u128(4)), None);
        assert_eq!(places.unnamed_root(AGENT), None);
        assert_eq!(places.roots()[0].owner, Uuid::nil());

        let m = ObjectProperties {
            object_data: vec![ferro_msg::msgs::object_properties::ObjectData {
                object_id: lamp,
                creator_id: Uuid::from_u128(0xc),
                owner_id: owner,
                name: b"Lamp\0".to_vec(),
                description: b"A lamp\0".to_vec(),
                ..Default::default()
            }],
        };
        let [(key, p)] = &from_properties(&m)[..] else {
            panic!("one object");
        };
        assert_eq!(
            (p.name.as_str(), p.description.as_str()),
            ("Lamp", "A lamp")
        );
        assert_eq!(places.set_props(*key, p.clone()), Some(3));
        assert_eq!(places.unnamed_root(lamp), None);
        let root = &places.roots()[0];
        assert_eq!(root.owner, owner);
        assert_eq!(
            root.props.as_ref().map(|p| p.creator),
            Some(Uuid::from_u128(0xc))
        );
        // an object never seen is not kept
        assert_eq!(places.set_props(Uuid::from_u128(99), props(1, "x")), None);

        // an update naming the same owner keeps them, another owner drops them
        let mut same = full(3, lamp, 0, false, Vec3::ZERO);
        same.owner = Some(owner);
        places.update(&same, AGENT);
        assert!(places.roots()[0].props.is_some());
        let mut sold = same.clone();
        sold.owner = Some(Uuid::from_u128(0x0f));
        places.update(&sold, AGENT);
        assert_eq!(places.roots()[0].props, None);
        assert_eq!(places.roots()[0].owner, Uuid::from_u128(0x0f));

        // a local id given to another object forgets them, and so does a kill
        places.set_props(lamp, props(0x0e, "Lamp"));
        places.update(&full(3, Uuid::from_u128(5), 0, false, Vec3::ZERO), AGENT);
        assert_eq!(places.roots()[0].props, None);
        places.set_props(Uuid::from_u128(5), props(0x0e, "Box"));
        places.kill(3);
        assert!(!places.props.contains_key(&3));
    }

    #[test]
    fn a_family_answer() {
        let mut m = ObjectPropertiesFamily::default();
        m.object_data.object_id = Uuid::from_u128(3);
        m.object_data.owner_id = Uuid::from_u128(0x0e);
        m.object_data.group_id = Uuid::from_u128(0x6);
        m.object_data.name = b"Sign\0".to_vec();
        let (key, p) = from_family(&m);
        assert_eq!(key, Uuid::from_u128(3));
        assert_eq!(p.creator, Uuid::nil());
        assert_eq!(
            (p.owner, p.group),
            (Uuid::from_u128(0x0e), Uuid::from_u128(0x6))
        );
        assert_eq!(p.name, "Sign");
    }

    #[test]
    fn sculpties_take_their_roots_properties() {
        let mut places = Places::default();
        places.update(&full(3, Uuid::from_u128(3), 0, false, Vec3::ONE), AGENT);
        let b = object_update::ObjectData {
            id: 4,
            full_id: Uuid::from_u128(4),
            parent_id: 3,
            p_code: 9,
            object_data: vec![0; 60],
            extra_params: sculpt_params(1),
            ..Default::default()
        };
        places.update(&from_full(&b).expect("a sculpted child"), AGENT);
        assert_eq!(places.sculpts()[0].name, None);
        places.set_props(Uuid::from_u128(3), props(0x0e, "Bench"));
        let s = &places.sculpts()[0];
        assert_eq!(
            (s.name.as_deref(), s.owner),
            (Some("Bench"), Uuid::from_u128(0x0e))
        );
    }

    #[test]
    fn asks_are_paced() {
        let mut asks = PropertyAsks::default();
        let t0 = Instant::now();
        for id in 0..1000 {
            asks.want(id);
        }
        // asked twice, queued once
        asks.want(0);
        let first = asks.round(t0, |_| true);
        assert_eq!(first.len(), SELECT_PACKET);
        assert_eq!(first[0], 0);
        // too soon for another round
        assert!(
            asks.round(t0 + Duration::from_millis(100), |_| true)
                .is_empty()
        );
        let mut t = t0;
        let mut sent = first.len();
        for _ in 0..5 {
            t += SELECT_SPACING;
            sent += asks.round(t, |_| true).len();
        }
        // no more than three packets unanswered
        assert_eq!(sent, SELECT_IN_FLIGHT);
        // answers make room
        for id in 0..10 {
            asks.answered(id);
        }
        t += SELECT_SPACING;
        assert_eq!(asks.round(t, |_| true).len(), 10);
        // objects answered or gone meanwhile are skipped, and free their place
        t += SELECT_SPACING;
        let next = asks.round(t, |id| id % 2 == 0);
        assert!(!next.is_empty() && next.iter().all(|id| id % 2 == 0));
        // an object asked and never answered is asked again after the timeout
        let mut asks = PropertyAsks::default();
        asks.want(7);
        assert_eq!(asks.round(t0, |_| true), vec![7]);
        asks.want(7);
        assert!(asks.round(t0 + SELECT_SPACING, |_| true).is_empty());
        asks.want(7);
        assert_eq!(asks.round(t0 + SELECT_TIMEOUT, |_| true), vec![7]);
    }
}
