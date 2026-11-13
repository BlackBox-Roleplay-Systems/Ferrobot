//! Port of parts of aurora-net/src/objects.rs (Aurora Viewer, see NOTICE.md).
//!
//! Where objects stand, from the object updates (LLViewerObject::processUpdateMessage,
//! LLViewerObjectList::processCompressedObjectUpdate, originally LGPL 2.1). A bot draws nothing:
//! it keeps only the place of the region's root objects and of the avatars, so that it knows
//! where it and the others are, also when seated on something.

use ferro_msg::msgs::{improved_terse_object_update, object_update};
use glam::{Quat, Vec3};
use std::collections::{HashMap, HashSet};
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
}

/// A root object and where it stands (GetObjectsInParcel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Root {
    pub full_id: Uuid,
    pub local_id: u32,
    pub owner: Uuid,
    pub position: Vec3,
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
    }

    pub fn clear(&mut self) {
        self.objects.clear();
        self.avatars.clear();
        self.known.clear();
        self.by_key.clear();
        self.own = None;
        self.sculpts.clear();
        self.labels.clear();
    }

    /// The sculpted prims (SecondBot's ListSculptys): each by itself, or by its linkset's root
    /// when the root is known (its key, owner, name and place).
    pub fn sculpts(&self) -> Vec<Sculpt> {
        self.sculpts
            .iter()
            .filter_map(|id| {
                let k = self.known.get(id)?;
                let own = Sculpt {
                    linked: false,
                    name: self.labels.get(id).cloned(),
                    owner: k.owner,
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
                    name: self.labels.get(&k.parent).cloned().or(own.name),
                    owner: root.owner,
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
                (o.parent == 0).then_some(Root {
                    full_id: k.full_id,
                    local_id: *id,
                    owner: k.owner,
                    position: o.position,
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
                local_id: 20
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
}
