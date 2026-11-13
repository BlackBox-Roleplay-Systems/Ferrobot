//! SecondBot's ParcelCommands, on the parcel the bot stands on (its SetupCurrentParcel): what the
//! region told of it, asked again when the bot has nothing yet.

use super::Ctx;
use super::reply::{Answer, basic, cs_bool_text, cs_float, failure, json_pairs};
use ferro_client::parcels::{FLAG_NAMES, PF_FOR_SALE, ParcelInfo};
use std::time::Duration;

/// SecondBot waits two seconds for parcel data it does not have yet.
const PARCEL_WAIT: Duration = Duration::from_secs(2);

/// The bot's parcel, or the failure SecondBot answers.
async fn current(ctx: &Ctx, command: &str) -> Result<ParcelInfo, Answer> {
    if !ctx.client.state().online {
        return Err(failure(command, "Error not in a sim", &[]));
    }
    match ctx.client.parcel(PARCEL_WAIT).await {
        Ok(Some(p)) => Ok(p),
        _ => Err(failure(command, "Parcel data not ready", &[])),
    }
}

pub async fn current_id(ctx: &Ctx) -> Answer {
    const C: &str = "GetCurrentParcelId";
    match current(ctx, C).await {
        Ok(p) => basic(C, &p.local_id.to_string(), &[]),
        Err(a) => a,
    }
}

pub async fn current_uuid(ctx: &Ctx) -> Answer {
    const C: &str = "GetCurrentParcelUUID";
    if let Err(a) = current(ctx, C).await {
        return a;
    }
    match ctx.client.parcel_global_id().await {
        Ok(id) => basic(C, &id.to_string(), &[]),
        Err(e) => {
            log::debug!("{C}: {e}");
            failure(C, "Unable to get parcel uuid", &[])
        }
    }
}

/// The size of its bounding box, "width,height".
pub async fn current_size(ctx: &Ctx) -> Answer {
    const C: &str = "GetCurrentParcelSize";
    match current(ctx, C).await {
        Ok(p) => {
            let d = p.aabb_max - p.aabb_min;
            basic(C, &format!("{},{}", cs_float(d.x), cs_float(d.y)), &[])
        }
        Err(a) => a,
    }
}

pub async fn desc(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelDesc";
    match current(ctx, C).await {
        Ok(p) => basic(C, &p.desc, &[]),
        Err(a) => a,
    }
}

/// Every flag by LibreMetaverse's name, "True" or "False".
pub async fn flags(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelFlags";
    match current(ctx, C).await {
        Ok(p) => {
            let pairs: Vec<(&str, String)> = FLAG_NAMES
                .iter()
                .map(|(bit, name)| (*name, cs_bool_text(p.flags & bit != 0)))
                .collect();
            basic(C, &json_pairs(&pairs), &[])
        }
        Err(a) => a,
    }
}

pub async fn prim_counts(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelPrimCounts";
    match current(ctx, C).await {
        Ok(p) => basic(
            C,
            &json_pairs(&[
                ("TotalPrims", p.total_prims.to_string()),
                ("GroupPrims", p.group_prims.to_string()),
                ("OwnerPrims", p.owner_prims.to_string()),
                ("OtherPrims", p.other_prims.to_string()),
                ("MaxPrims", p.max_prims.to_string()),
                ("ParcelPrimBonus", cs_float(p.prim_bonus)),
            ]),
            &[],
        ),
        Err(a) => a,
    }
}

pub async fn sale_details(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelSaleDetails";
    match current(ctx, C).await {
        Ok(p) => basic(
            C,
            &json_pairs(&[
                ("ForSale", cs_bool_text(p.flags & PF_FOR_SALE != 0)),
                ("Cost", p.sale_price.to_string()),
                ("SaleTarget", p.auth_buyer.to_string()),
            ]),
            &[],
        ),
        Err(a) => a,
    }
}

/// The parcel's dwell, asked of the region (SecondBot prints what LibreMetaverse last heard,
/// 0 until something asked).
pub async fn traffic(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelTraffic";
    match current(ctx, C).await {
        Ok(p) => {
            let dwell = ctx.client.parcel_dwell(p.local_id).await.unwrap_or(0.0);
            basic(C, &cs_float(dwell), &[])
        }
        Err(a) => a,
    }
}
