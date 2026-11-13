//! Port of parts of aurora-net/src/objects.rs (Aurora Viewer, see NOTICE.md).
//!
//! Where objects stand, from the object updates (LLViewerObject::processUpdateMessage,
//! LLViewerObjectList::processCompressedObjectUpdate, originally LGPL 2.1). A bot draws nothing:
//! it keeps only the place of the region's root objects and of its own avatar, so that it knows
//! where it is, also when it sits on something.

use ferro_msg::msgs::{improved_terse_object_update, object_update};
use glam::{Quat, Vec3};
use std::collections::HashMap;
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
}

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
    Some(Placed {
        local_id: b.id,
        full_id: Some(b.full_id),
        parent_id: Some(b.parent_id),
        position: sane_vec(position),
        rotation: sane_quat(rotation),
    })
}

/// An ObjectUpdateCompressed blob: its fixed head, then the optional parts, of which only the
/// parent matters here.
pub fn from_compressed(data: &[u8]) -> Option<Placed> {
    let mut r = Reader { d: data, p: 0 };
    let full_id = r.uuid()?;
    let local_id = r.u32()?;
    let _pcode = r.u8()?;
    let _state = r.u8()?;
    let _crc = r.u32()?;
    let _material = r.u8()?;
    let _click_action = r.u8()?;
    let _scale = r.vec3()?;
    let position = r.vec3()?;
    let rotation = ferro_msg::unpack_quat(r.vec3()?);
    let flags = r.u32()?;
    let _owner = r.uuid()?;
    if flags & 0x80 != 0 {
        let _angular_velocity = r.vec3()?;
    }
    let parent_id = if flags & 0x20 != 0 { r.u32()? } else { 0 };
    Some(Placed {
        local_id,
        full_id: Some(full_id),
        parent_id: Some(parent_id),
        position: sane_vec(position),
        rotation: sane_quat(rotation),
    })
}

/// An ImprovedTerseObjectUpdate block: a move.
pub fn from_terse(b: &improved_terse_object_update::ObjectData) -> Option<Placed> {
    let mut r = Reader { d: &b.data, p: 0 };
    let local_id = r.u32()?;
    let _state = r.u8()?;
    if r.u8()? != 0 {
        // an avatar's foot plane
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
    })
}

#[derive(Debug, Clone, Copy)]
struct Place {
    parent: u32,
    position: Vec3,
    rotation: Quat,
}

/// The places the bot keeps: the region's root objects and its own avatar.
#[derive(Debug, Default)]
pub struct Places {
    objects: HashMap<u32, Place>,
    /// The bot's avatar in this region, once a full update has named it.
    own: Option<u32>,
}

impl Places {
    /// An update from the region. A seated avatar is a child of the linkset's root, so keeping
    /// the roots is enough to place it.
    pub fn update(&mut self, p: &Placed, agent: Uuid) {
        if p.full_id == Some(agent) {
            self.own = Some(p.local_id);
        }
        let own = self.own == Some(p.local_id);
        match p.parent_id {
            Some(parent) if parent == 0 || own => {
                self.objects.insert(
                    p.local_id,
                    Place {
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
    }

    pub fn clear(&mut self) {
        self.objects.clear();
        self.own = None;
    }

    /// The bot's place in the region: its own, or on what it sits on (the root's place plus the
    /// offset turned by the root's rotation, as SecondBot's AvatarPositions). None while unknown.
    pub fn own_position(&self) -> Option<Vec3> {
        let me = self.objects.get(&self.own?)?;
        if me.parent == 0 {
            return Some(me.position);
        }
        let root = self.objects.get(&me.parent)?;
        Some(root.position + root.rotation * me.position)
    }

    /// The local id of what the bot sits on, if anything.
    pub fn own_parent(&self) -> Option<u32> {
        let me = self.objects.get(&self.own?)?;
        (me.parent != 0).then_some(me.parent)
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
        places.update(&p, AGENT);
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
        assert_eq!(places.own_position(), Some(Vec3::new(100.0, 100.0, 21.0)));
        // the seat gone, the place is unknown until the next update
        places.kill(3);
        assert_eq!(places.own_position(), None);
        // a child prim is not kept
        places.update(&full(9, Uuid::from_u128(9), 3, false, Vec3::ONE), AGENT);
        assert!(!places.objects.contains_key(&9));
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
    }
}
