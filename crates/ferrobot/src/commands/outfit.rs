//! SecondBot's Attach, Detach, SwapOutfit, SwapOutfitByPath and Outfit: what the bot wears,
//! kept in its Current Outfit Folder.
//!
//! Deliberate difference: Detach "*" takes off every attachment (SecondBot's skips them all).
//! As SecondBot's, SwapOutfitByPath and Outfit answer as SwapOutfit once the folder is found.

use super::reply::{Answer, basic, failure};
use super::{Ctx, key, owned};

/// LibreMetaverse's AttachmentPoint names, by number (1 to 55), as SecondBot's Detach compares.
const POINTS: [&str; 55] = [
    "Chest",
    "Skull",
    "LeftShoulder",
    "RightShoulder",
    "LeftHand",
    "RightHand",
    "LeftFoot",
    "RightFoot",
    "Spine",
    "Pelvis",
    "Mouth",
    "Chin",
    "LeftEar",
    "RightEar",
    "LeftEyeball",
    "RightEyeball",
    "Nose",
    "RightUpperArm",
    "RightForearm",
    "LeftUpperArm",
    "LeftForearm",
    "RightHip",
    "RightUpperLeg",
    "RightLowerLeg",
    "LeftHip",
    "LeftUpperLeg",
    "LeftLowerLeg",
    "Stomach",
    "LeftPec",
    "RightPec",
    "HUDCenter2",
    "HUDTopRight",
    "HUDTop",
    "HUDTopLeft",
    "HUDCenter",
    "HUDBottomLeft",
    "HUDBottom",
    "HUDBottomRight",
    "Neck",
    "Root",
    "LeftHandRing",
    "RightHandRing",
    "TailBase",
    "TailTip",
    "LeftWing",
    "RightWing",
    "Jaw",
    "AltLeftEar",
    "AltRightEar",
    "AltLeftEye",
    "AltRightEye",
    "Tongue",
    "Groin",
    "LeftHindFoot",
    "RightHindFoot",
];

fn point_name(point: u8) -> &'static str {
    match point {
        1..=55 => POINTS[usize::from(point) - 1],
        _ => "Default",
    }
}

pub async fn attach(ctx: &Ctx, item: &str) -> Answer {
    const C: &str = "Attach";
    let args = owned(&[item]);
    let Some(id) = key(item) else {
        return failure(C, "invaild item uuid", &args);
    };
    match ctx.client.attach(id).await {
        Ok(()) => basic(C, "Ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

/// Detach: an item's key, an attachment point's name, or "*" for everything.
pub async fn detach(ctx: &Ctx, item: &str) -> Answer {
    const C: &str = "Detach";
    let args = owned(&[item]);
    if let Some(id) = key(item) {
        return match ctx.client.detach(id).await {
            Ok(()) => basic(C, "Ok", &args),
            Err(e) => failure(C, &e.to_string(), &args),
        };
    }
    let worn = ctx.client.worn().await.unwrap_or_default();
    let mut items: Vec<uuid::Uuid> = worn
        .iter()
        .filter(|w| item == "*" || point_name(w.point) == item)
        .map(|w| w.item)
        .collect();
    items.dedup();
    if items.is_empty() {
        return failure(C, "invaild item uuid pr invaild attach point", &args);
    }
    for id in items {
        if let Err(e) = ctx.client.detach(id).await {
            return failure(C, &e.to_string(), &args);
        }
    }
    basic(C, "ok", &args)
}

pub async fn swap_outfit(ctx: &Ctx, folder: &str) -> Answer {
    const C: &str = "SwapOutfit";
    let args = owned(&[folder]);
    let Some(id) = key(folder) else {
        return failure(C, "invaild folder uuid", &args);
    };
    match ctx.client.replace_outfit(id).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => {
            log::warn!("{C}: {e}");
            let why = "target folder is empty or so full I cant get it in 5 secs...";
            failure(C, why, &args)
        }
    }
}

/// SwapOutfitByPath: the folder by its path from the root, then SwapOutfit.
pub async fn swap_outfit_by_path(ctx: &Ctx, path: &str) -> Answer {
    match ctx.client.folder_by_path(path).await {
        Ok(Some(f)) => swap_outfit(ctx, &f.id.to_string()).await,
        _ => failure(
            "SwapOutfitByPath",
            "Named folder value is empty",
            &owned(&[path]),
        ),
    }
}

/// Outfit: a folder of "My Outfits".
pub async fn outfit(ctx: &Ctx, name: &str) -> Answer {
    swap_outfit_by_path(ctx, &format!("My Outfits/{name}")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_points_by_libremetaverse_name() {
        assert_eq!(point_name(1), "Chest");
        assert_eq!(point_name(2), "Skull");
        assert_eq!(point_name(39), "Neck");
        assert_eq!(point_name(55), "RightHindFoot");
        assert_eq!(point_name(0), "Default");
    }
}
