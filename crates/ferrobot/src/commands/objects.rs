//! SecondBot's object commands: rezzing an item (RezObject, RezObjectOnPos,
//! RezObjectParcelCenter and their Async… forms), clicking an object (ClickObject), taking one
//! back (UnRezObject), the objects on the parcel (GetObjectsInParcel), and putting an item in an
//! object (TransferInventoryToObject and its Async… form).
//!
//! Deliberate differences: ClickObject answers "ok", or fails when it cannot see the object
//! (SecondBot prints a C# pair, "[0, ]" when it found nothing); GetObjectsInParcel lists each root
//! object's ID, LocalID, OwnerID and Position (SecondBot serialises LibreMetaverse's whole
//! Primitive), by the parcel's x and y only (SecondBot also compares the height with the parcel's,
//! which is 0, so finds nothing).

use super::inventory::{AsyncReply, async_reply};
use super::reply::{Answer, basic, cs_number, failure, json};
use super::{Ctx, cs_bool, key, owned};
use ferro_client::inventory::InvItem;
use glam::Vec3;
use serde::Serialize;
use uuid::Uuid;

/// Where to rez: by the bot, at a place, or at the parcel's centre.
enum At {
    Bot,
    Place(Vec3),
    ParcelCenter(f32),
}

/// The checks every rez makes before anything else: the item's key, the parcel known.
async fn rez_checks(ctx: &Ctx, c: &str, item: &str) -> Result<Uuid, Answer> {
    let Some(id) = key(item) else {
        return Err(failure(
            c,
            &format!("Invaild item UUID: {item}"),
            &owned(&[item]),
        ));
    };
    super::parcel::current_with(ctx, c, &owned(&[item])).await?;
    Ok(id)
}

/// The rez itself: the item fetched, the place worked out, RezObject sent; the request's id.
async fn rez_core(ctx: &Ctx, id: Uuid, at: &At) -> Result<Uuid, String> {
    let item = match ctx.client.inventory_item(id).await {
        Ok(Some(i)) => i,
        _ => return Err(format!("Unable to find item: {id}")),
    };
    let here = ctx.client.state().position;
    let place = match at {
        At::Bot => here,
        At::Place(p) => *p,
        At::ParcelCenter(zoff) => {
            let p = ctx
                .client
                .parcel(std::time::Duration::from_secs(2))
                .await
                .ok()
                .flatten()
                .ok_or_else(|| "Parcel data not ready".to_owned())?;
            let mid = (p.aabb_min + p.aabb_max) / 2.0;
            Vec3::new(mid.x, mid.y, here.z + zoff)
        }
    };
    ctx.client.rez(item, place).await.map_err(|e| e.to_string())
}

fn coords(x: &str, y: &str, z: &str) -> Option<Vec3> {
    let f = |s: &str| s.trim().parse::<f32>().ok();
    Some(Vec3::new(f(x)?, f(y)?, f(z)?))
}

/// RezObjectOnPos's checks: SecondBot answers "X" for each of the three.
fn place_of(c: &str, item: &str, x: &str, y: &str, z: &str) -> Result<Vec3, Answer> {
    coords(x, y, z).ok_or_else(|| failure(c, "Unable to unpack X cord", &owned(&[item])))
}

/// RezObjectParcelCenter's checks.
fn z_offset(c: &str, item: &str, zoffset: &str) -> Result<f32, Answer> {
    let Ok(z) = zoffset.trim().parse::<f32>() else {
        return Err(failure(
            c,
            &format!("Invaild Z offset: {zoffset}"),
            &owned(&[item]),
        ));
    };
    if !(-2.0..=2.0).contains(&z) {
        let why = "Z offset must be between -2 and 2";
        return Err(failure(c, why, &owned(&[item, zoffset])));
    }
    Ok(z)
}

async fn rez(ctx: &Ctx, c: &str, item: &str, at: At) -> Answer {
    let id = match rez_checks(ctx, c, item).await {
        Ok(id) => id,
        Err(a) => return a,
    };
    match rez_core(ctx, id, &at).await {
        Ok(q) => basic(c, &q.to_string(), &owned(&[item])),
        Err(why) => failure(c, &why, &owned(&[item])),
    }
}

/// The Async… form of a rez: the request's id now; the rez's id (nil when it failed) to the
/// reply target.
async fn rez_async(
    ctx: &Ctx,
    c: &'static str,
    item: &str,
    target: &str,
    at: At,
    args: Vec<String>,
) -> Answer {
    let id = match rez_checks(ctx, c, item).await {
        Ok(id) => id,
        Err(a) => return a,
    };
    let request = Uuid::new_v4().to_string();
    let (ctx2, item2, target2, req) = (
        ctx.clone(),
        item.to_owned(),
        target.to_owned(),
        request.clone(),
    );
    tokio::spawn(async move {
        let done = rez_core(&ctx2, id, &at).await;
        let r = AsyncReply {
            request: req,
            status: done.is_ok(),
            message: item2,
            data: done.map_or_else(|_| Uuid::nil().to_string(), |q| q.to_string()),
        };
        async_reply(&ctx2, &target2, c, r).await;
    });
    basic(c, &request, &args)
}

pub async fn rez_object(ctx: &Ctx, item: &str) -> Answer {
    rez(ctx, "RezObject", item, At::Bot).await
}

pub async fn rez_object_on_pos(ctx: &Ctx, item: &str, x: &str, y: &str, z: &str) -> Answer {
    const C: &str = "RezObjectOnPos";
    if let Err(a) = rez_checks(ctx, C, item).await {
        return a;
    }
    match place_of(C, item, x, y, z) {
        Ok(p) => rez(ctx, C, item, At::Place(p)).await,
        Err(a) => a,
    }
}

pub async fn rez_object_parcel_center(ctx: &Ctx, item: &str, zoffset: &str) -> Answer {
    const C: &str = "RezObjectParcelCenter";
    if key(item).is_none() {
        return failure(C, &format!("Invaild item UUID: {item}"), &owned(&[item]));
    }
    match z_offset(C, item, zoffset) {
        Ok(z) => rez(ctx, C, item, At::ParcelCenter(z)).await,
        Err(a) => a,
    }
}

pub async fn async_rez_object(ctx: &Ctx, item: &str, target: &str) -> Answer {
    let args = owned(&[item, target]);
    rez_async(ctx, "AsyncRezObject", item, target, At::Bot, args).await
}

pub async fn async_rez_object_on_pos(
    ctx: &Ctx,
    item: &str,
    x: &str,
    y: &str,
    z: &str,
    target: &str,
) -> Answer {
    const C: &str = "AsyncRezObjectOnPos";
    let p = match place_of(C, item, x, y, z) {
        Ok(p) => p,
        Err(a) => return a,
    };
    let args = owned(&[item, x, y, z, target]);
    rez_async(ctx, C, item, target, At::Place(p), args).await
}

pub async fn async_rez_object_parcel_center(
    ctx: &Ctx,
    item: &str,
    zoffset: &str,
    target: &str,
) -> Answer {
    const C: &str = "AsyncRezObjectParcelCenter";
    if key(item).is_none() {
        return failure(C, &format!("Invaild item UUID: {item}"), &owned(&[item]));
    }
    let z = match z_offset(C, item, zoffset) {
        Ok(z) => z,
        Err(a) => return a,
    };
    let args = owned(&[item, zoffset, target]);
    rez_async(ctx, C, item, target, At::ParcelCenter(z), args).await
}

/// ClickObject: the point-at effect toward it, then a click (a grab let go at once).
pub async fn click(ctx: &Ctx, target: &str) -> Answer {
    const C: &str = "ClickObject";
    let args = owned(&[target]);
    let Some(id) = key(target) else {
        return failure(C, "Invaild object UUID", &args);
    };
    let Ok(Some(found)) = ctx.client.find_object(id).await else {
        return failure(C, "Unable to see object", &args);
    };
    let _ = ctx.client.point_at(id).await;
    match ctx.client.click_object(found.local_id).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

/// UnRezObject: the object taken into the bot's Objects folder (FT_OBJECT), else its root.
pub async fn unrez(ctx: &Ctx, object: &str) -> Answer {
    const C: &str = "UnRezObject";
    let args = owned(&[object]);
    let Some(id) = key(object) else {
        return failure(C, "Invaild object uuid", &args);
    };
    let Ok(Some(found)) = ctx.client.find_object(id).await else {
        return failure(C, "Unable to find object", &args);
    };
    let root = ctx.client.inventory_root();
    let folder = ctx
        .client
        .folder_contents(root)
        .await
        .ok()
        .and_then(|c| c.folders.into_iter().find(|f| f.type_default == 6))
        .map_or(root, |f| f.id);
    match ctx.client.derez(found.local_id, folder).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

/// LibreMetaverse's Vector3, as System.Text.Json writes it.
#[derive(Serialize)]
struct Vector3 {
    #[serde(rename = "X")]
    x: serde_json::Value,
    #[serde(rename = "Y")]
    y: serde_json::Value,
    #[serde(rename = "Z")]
    z: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ObjectEntry {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "LocalID")]
    local_id: u32,
    #[serde(rename = "OwnerID")]
    owner_id: String,
    position: Vector3,
}

pub async fn objects_in_parcel(ctx: &Ctx) -> Answer {
    const C: &str = "GetObjectsInParcel";
    let p = match super::parcel::current_with(ctx, C, &[]).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let roots = ctx.client.root_objects().await.unwrap_or_default();
    let mut inside: Vec<ObjectEntry> = roots
        .iter()
        .filter(|r| {
            let at = r.position;
            at.x > p.aabb_min.x && at.x < p.aabb_max.x && at.y > p.aabb_min.y && at.y < p.aabb_max.y
        })
        .map(|r| ObjectEntry {
            id: r.full_id.to_string(),
            local_id: r.local_id,
            owner_id: r.owner.to_string(),
            position: Vector3 {
                x: cs_number(f64::from(r.position.x)),
                y: cs_number(f64::from(r.position.y)),
                z: cs_number(f64::from(r.position.z)),
            },
        })
        .collect();
    if inside.is_empty() {
        return failure(C, "No objects found in the parcel", &[]);
    }
    inside.sort_by_key(|o| o.local_id);
    basic(C, &json(&inside), &[])
}

/// The item put into the object: a script by RezScript (running or not), anything else into
/// its contents. The answer, or why there is none.
async fn transfer_core(
    ctx: &Ctx,
    item: Uuid,
    object: Uuid,
    running: bool,
) -> Result<String, String> {
    let found: InvItem = match ctx.client.inventory_item(item).await {
        Ok(Some(i)) => i,
        _ => return Err("Unable to find inventory".into()),
    };
    let Ok(Some(target)) = ctx.client.find_object(object).await else {
        return Err("Unable to find object".into());
    };
    let script = found.asset_type == 10;
    let running = running && script;
    ctx.client
        .give_to_object(target.local_id, found, running)
        .await
        .map_err(|e| e.to_string())?;
    Ok(if script {
        format!(
            "Transfering script [state: {}]",
            if running { "True" } else { "False" }
        )
    } else {
        "Transfering inventory".to_owned()
    })
}

fn transfer_checks(
    c: &str,
    item: &str,
    object: &str,
    running: &str,
) -> Result<(Uuid, Uuid, bool), Answer> {
    let args = owned(&[item, object, running]);
    let Some(i) = key(item) else {
        return Err(failure(c, "Invaild item uuid", &args));
    };
    let Some(o) = key(object) else {
        return Err(failure(c, "Invaild object uuid", &args));
    };
    let Some(r) = cs_bool(running) else {
        return Err(failure(c, "Invaild running", &args));
    };
    Ok((i, o, r))
}

pub async fn transfer_to_object(ctx: &Ctx, item: &str, object: &str, running: &str) -> Answer {
    const C: &str = "TransferInventoryToObject";
    let args = owned(&[item, object, running]);
    let (i, o, r) = match transfer_checks(C, item, object, running) {
        Ok(v) => v,
        Err(a) => return a,
    };
    match transfer_core(ctx, i, o, r).await {
        Ok(text) => basic(C, &text, &args),
        Err(why) => failure(C, &why, &args),
    }
}

pub async fn async_transfer_to_object(
    ctx: &Ctx,
    item: &str,
    object: &str,
    running: &str,
    target: &str,
) -> Answer {
    const C: &str = "AsyncTransferInventoryToObject";
    let (i, o, r) = match transfer_checks(C, item, object, running) {
        Ok(v) => v,
        Err(a) => return a,
    };
    let request = Uuid::new_v4().to_string();
    let (ctx2, item2, target2, req) = (
        ctx.clone(),
        item.to_owned(),
        target.to_owned(),
        request.clone(),
    );
    tokio::spawn(async move {
        let done = transfer_core(&ctx2, i, o, r).await;
        let r = AsyncReply {
            request: req,
            status: done.is_ok(),
            message: item2,
            data: done.unwrap_or_else(|why| why),
        };
        async_reply(&ctx2, &target2, C, r).await;
    });
    basic(C, &request, &owned(&[item, object, running, target]))
}
