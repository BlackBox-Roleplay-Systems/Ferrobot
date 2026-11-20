//! An avatar's appearance as the region sends it (AvatarAppearance): the TextureEntry whose faces
//! hold the bakes viewers draw, and the Current Outfit version they were baked from. With
//! server-side baking the region makes the bakes itself, from the Current Outfit Folder.

use uuid::Uuid;

/// The faces of an avatar's TextureEntry that hold bakes (OpenSim's AvatarAppearance.BAKE_INDICES,
/// LLAvatarAppearanceDefines): head, upper, lower, eyes, skirt, hair, then the five Bakes on Mesh
/// channels (left arm, left leg, aux 1 to 3).
pub const BAKE_FACES: [usize; 11] = [8, 9, 10, 11, 19, 20, 40, 41, 42, 43, 44];

/// The name a viewer gives each bake face when it fetches it from the grid's appearance service
/// (llvoavatar.cpp getImageURL, LLAvatarAppearanceDictionary): `<url>texture/<agent>/<name>/<bake>`.
pub fn bake_channel(face: usize) -> Option<&'static str> {
    Some(match face {
        8 => "head",
        9 => "upper",
        10 => "lower",
        11 => "eyes",
        19 => "skirt",
        20 => "hair",
        40 => "leftarm",
        41 => "leftleg",
        42 => "aux1",
        43 => "aux2",
        44 => "aux3",
        _ => return None,
    })
}

/// How many faces an avatar's TextureEntry covers (the last bake face, 44, and one).
pub const AVATAR_FACES: usize = 45;

/// IMG_DEFAULT_AVATAR: a face with no bake.
pub const DEFAULT_AVATAR_TEXTURE: Uuid = Uuid::from_u128(0xc228d1cf_4b5d_4ba8_84f4_899a0796aa97);

/// The plywood texture, OpenMetaverse's default for a TextureEntry: also no bake.
pub const DEFAULT_TEXTURE: Uuid = Uuid::from_u128(0x89556747_24cb_43ed_920b_47caed15465f);

/// What the region last said an avatar looks like.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Appearance {
    /// The Current Outfit version the bakes were made from (AppearanceData), 0 when not sent.
    pub cof_version: i32,
    /// The texture of every face, the default filled in.
    pub faces: Vec<Uuid>,
}

impl Appearance {
    /// The faces that hold a bake, with its texture.
    pub fn bakes(&self) -> Vec<(usize, Uuid)> {
        BAKE_FACES
            .iter()
            .filter_map(|&f| {
                let id = *self.faces.get(f)?;
                (!id.is_nil() && id != DEFAULT_AVATAR_TEXTURE && id != DEFAULT_TEXTURE)
                    .then_some((f, id))
            })
            .collect()
    }
}

/// The texture of each of the first `count` faces of a TextureEntry (LLTextureEntry's
/// packed form): the default texture, then pairs of a face bit field and the texture of the
/// faces it names, until a zero bit field. None when the bytes end early.
pub fn texture_faces(te: &[u8], count: usize) -> Option<Vec<Uuid>> {
    let default = uuid_at(te, 0)?;
    let mut faces = vec![default; count];
    let mut pos = 16;
    loop {
        // the faces named, seven bits a byte, the high bit set on all but the last
        let mut bits: u64 = 0;
        loop {
            let b = *te.get(pos)?;
            pos += 1;
            bits = (bits << 7) | u64::from(b & 0x7f);
            if b & 0x80 == 0 {
                break;
            }
        }
        if bits == 0 {
            return Some(faces);
        }
        let id = uuid_at(te, pos)?;
        pos += 16;
        for (i, face) in faces.iter_mut().enumerate().take(64) {
            if bits & (1 << i) != 0 {
                *face = id;
            }
        }
    }
}

fn uuid_at(b: &[u8], at: usize) -> Option<Uuid> {
    Some(Uuid::from_bytes(b.get(at..at + 16)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TextureEntry as LLTextureEntry packs it: the default, then (faces, texture) pairs.
    fn packed(default: Uuid, pairs: &[(u64, Uuid)]) -> Vec<u8> {
        let mut out = default.as_bytes().to_vec();
        for (bits, id) in pairs {
            let mut groups = vec![(bits & 0x7f) as u8];
            let mut rest = bits >> 7;
            while rest != 0 {
                groups.push((rest & 0x7f) as u8 | 0x80);
                rest >>= 7;
            }
            groups.reverse();
            out.extend(groups);
            out.extend(id.as_bytes());
        }
        out.push(0);
        // the colour, scale and other sections follow; they are not read
        out.extend([0xff, 0xff, 0xff, 0xff, 0]);
        out
    }

    #[test]
    fn faces_take_the_default_and_then_their_own_texture() {
        let head = Uuid::from_u128(1);
        let arm = Uuid::from_u128(2);
        let te = packed(
            DEFAULT_AVATAR_TEXTURE,
            &[(1 << 8, head), ((1 << 40) | (1 << 41), arm)],
        );
        let faces = texture_faces(&te, AVATAR_FACES).unwrap();
        assert_eq!(faces[0], DEFAULT_AVATAR_TEXTURE);
        assert_eq!(faces[8], head);
        assert_eq!(
            (faces[40], faces[41], faces[42]),
            (arm, arm, DEFAULT_AVATAR_TEXTURE)
        );
        let a = Appearance {
            cof_version: 3,
            faces,
        };
        assert_eq!(a.bakes(), vec![(8, head), (40, arm), (41, arm)]);
    }

    #[test]
    fn a_short_entry_is_refused() {
        assert_eq!(texture_faces(&[0; 10], AVATAR_FACES), None);
        let mut te = packed(DEFAULT_AVATAR_TEXTURE, &[(1 << 9, Uuid::from_u128(5))]);
        te.truncate(20);
        assert_eq!(texture_faces(&te, AVATAR_FACES), None);
    }
}
