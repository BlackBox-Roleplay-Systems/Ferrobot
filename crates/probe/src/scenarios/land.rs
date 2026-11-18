//! Land: parcels divided and joined, the ground raised, lowered and reverted, as the viewer's
//! land tools do it, each judged by what the region then says (its parcels, its overlay, the
//! ground's patches). The work is kept to a 32 m square of the home region (`--land-at`), plus a
//! 16 m square at its east edge, and put back as it was.
//!
//! `land` needs an account that owns that land; `land-guard` one that does not (another
//! resident's land: nothing it asks may change it); `land-faults` asks what a viewer never asks
//! and may leave the region damaged when the region is wrong (a parcel of no land, a terrain
//! lock never released).

use super::{Probe, wait_for};
use ferro_client::land::action;
use ferro_client::parcels::{self, ParcelInfo, RegionParcels};
use ferro_client::{Client, ParcelAction};
use glam::Vec3;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long a parcel change may take to show in the region's answers.
const PARCEL_WAIT: Duration = Duration::from_secs(10);
/// How long the ground may take to change: the region sends changed patches on its terrain tick
/// (OpenSim's UpdateTerrainEveryNFrames, a few seconds).
const GROUND_WAIT: Duration = Duration::from_secs(15);
/// How long to watch for a change that must not come.
const NO_CHANGE_WAIT: Duration = Duration::from_secs(6);
/// PF_ALLOW_TERRAFORM: anyone may edit this parcel's ground.
const PF_ALLOW_TERRAFORM: u32 = 1 << 4;
/// A raise of this much (seconds, OpenSim's brush strength) at the brush's centre.
const RAISE: f32 = 1.0;
/// A change of the ground smaller than this is the patches' rounding.
const GROUND_EPSILON: f32 = 0.2;

/// A rectangle of the region, in metres.
#[derive(Debug, Clone, Copy)]
struct Rect {
    west: f32,
    south: f32,
    east: f32,
    north: f32,
}

impl Rect {
    fn new(west: f32, south: f32, east: f32, north: f32) -> Rect {
        Rect {
            west,
            south,
            east,
            north,
        }
    }
}

/// The parcels as the bot found them: their count, and each one's local id and area.
fn shape(p: &RegionParcels) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = p.parcels.iter().map(|q| (q.local_id, q.area)).collect();
    v.sort_unstable();
    v
}

/// The parcel covering a point.
fn at(p: &RegionParcels, x: f32, y: f32) -> Option<&ParcelInfo> {
    let (cx, cy) = ((x / 4.0) as usize, (y / 4.0) as usize);
    let id = *p.map.get(cy * parcels::MAP_SIZE + cx)?;
    p.parcels.iter().find(|q| q.local_id == id)
}

impl Probe {
    /// The square's south-west corner.
    fn land_square(&self) -> (f32, f32) {
        (self.opts.land_at.0, self.opts.land_at.1)
    }

    /// To the square, the region's parcels learnt, the ground's patches there come.
    async fn land_setup(&mut self, client: &Client, what: &str) -> Option<RegionParcels> {
        let home = self.opts.home.clone();
        let (x0, y0) = self.land_square();
        if !self
            .teleport(client, what, &home, Vec3::new(x0 + 16.0, y0 + 16.0, 40.0))
            .await
        {
            return None;
        }
        let t = Instant::now();
        let ground = wait_for(Duration::from_secs(10), || {
            client.ground_height(x0 + 8.0, y0 + 24.0).is_some()
        })
        .await;
        if !ground {
            self.emit(
                client,
                "land-ground",
                false,
                t,
                "no ground patches came".into(),
            );
            return None;
        }
        match client.region_parcels().await {
            Ok(p) => Some(p),
            Err(e) => {
                self.emit(client, "land-parcels", false, t, e.to_string());
                None
            }
        }
    }

    /// The region's parcels, asked again until `done` says they are as expected (or the wait
    /// ends): the last answer.
    async fn parcels_until(
        &self,
        client: &Client,
        done: impl Fn(&RegionParcels) -> bool,
    ) -> Option<RegionParcels> {
        let t = Instant::now();
        let mut last = None;
        while t.elapsed() < PARCEL_WAIT {
            tokio::time::sleep(Duration::from_millis(700)).await;
            if let Ok(p) = client.region_parcels().await {
                let ok = done(&p);
                last = Some(p);
                if ok {
                    break;
                }
            }
        }
        last
    }

    /// The ground at a point, waited for to pass `done` (or the wait to end): the last height.
    async fn ground_until(
        &self,
        client: &Client,
        x: f32,
        y: f32,
        wait: Duration,
        done: impl Fn(f32) -> bool,
    ) -> Option<f32> {
        let t = Instant::now();
        let mut last = client.ground_height(x, y);
        while t.elapsed() < wait {
            last = client.ground_height(x, y);
            if last.is_some_and(&done) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        last
    }

    async fn divide(&self, client: &Client, r: Rect) -> Result<(), String> {
        client
            .parcel_act(ParcelAction::Divide {
                west: r.west,
                south: r.south,
                east: r.east,
                north: r.north,
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn join(&self, client: &Client, r: Rect) -> Result<(), String> {
        client
            .parcel_act(ParcelAction::Join {
                west: r.west,
                south: r.south,
                east: r.east,
                north: r.north,
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// A land tool on a point (the brush) or a rectangle, `local_id` -1 as the viewer sends it.
    async fn terraform(
        &self,
        client: &Client,
        what: u8,
        r: Rect,
        seconds: f32,
        local_id: i32,
    ) -> Result<(), String> {
        let height = client.ground_height(r.west, r.south).unwrap_or(20.0);
        client
            .parcel_act(ParcelAction::ModifyLand {
                action: what,
                brush_size: 4.0,
                seconds,
                height,
                local_id,
                west: r.west,
                south: r.south,
                east: r.east,
                north: r.north,
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// `land`: divide, divide at the region's edge, join both back; raise, lower, revert.
    pub(super) async fn land(&mut self, client: &Client) {
        let Some(start) = self.land_setup(client, "land-setup").await else {
            return;
        };
        let (x0, y0) = self.land_square();
        let me = client.state().agent_id;
        let t = Instant::now();
        let Some(mine) = at(&start, x0 + 16.0, y0 + 16.0).cloned() else {
            self.emit(client, "land-parcels", false, t, "no parcel there".into());
            return;
        };
        let edge_owned = at(&start, 250.0, y0 + 8.0).is_some_and(|p| p.local_id == mine.local_id);
        if mine.owner_id != me || !edge_owned {
            self.emit(
                client,
                "land-parcels",
                false,
                t,
                format!(
                    "the land needs to be the bot's, from ({x0}, {y0}) to the east edge: parcel \
                     {} is {}'s",
                    mine.local_id, mine.owner_id
                ),
            );
            return;
        }
        let before = shape(&start);
        self.emit(
            client,
            "land-parcels",
            true,
            t,
            format!("{} parcels, the bot's {} m²", before.len(), mine.area),
        );

        // the square's south-west quarter becomes a parcel
        let t = Instant::now();
        let overlays = client.state().overlay.updates;
        let quarter = Rect::new(x0, y0, x0 + 16.0, y0 + 16.0);
        let note = match self.divide(client, quarter).await {
            Err(e) => e,
            Ok(()) => {
                let n = before.len();
                let p = self
                    .parcels_until(client, |p| p.parcels.len() == n + 1)
                    .await;
                judge_divide(p.as_ref(), &mine, x0 + 8.0, y0 + 8.0, me, n)
            }
        };
        let divided = note.is_empty();
        self.emit(client, "land-divide", divided, t, note);
        if divided {
            // the region sends its overlay again: a line on the new parcel's west and south
            let t = Instant::now();
            let came = wait_for(Duration::from_secs(5), || {
                client.state().overlay.updates > overlays
            })
            .await;
            let o = client.state().overlay.clone();
            let (cx, cy) = ((x0 / 4.0) as usize, (y0 / 4.0) as usize);
            let note = if !came {
                "no ParcelOverlay came".into()
            } else if o.width().is_none() {
                "the overlay came in part".into()
            } else if o.west_line(cx, cy) != Some(true) || o.south_line(cx, cy) != Some(true) {
                format!(
                    "no property line at the new parcel's corner (west {:?}, south {:?})",
                    o.west_line(cx, cy),
                    o.south_line(cx, cy)
                )
            } else if o.west_line(cx + 4, cy) != Some(true) {
                "no property line on the new parcel's east side".into()
            } else {
                String::new()
            };
            self.emit(client, "land-overlay", note.is_empty(), t, note);
        }

        // a 16 m square at the region's east edge
        let t = Instant::now();
        let edge = Rect::new(240.0, y0, 256.0, y0 + 16.0);
        let n = client
            .state()
            .region_parcels
            .as_ref()
            .map_or(before.len(), |p| p.parcels.len());
        let note = match self.divide(client, edge).await {
            Err(e) => e,
            Ok(()) => {
                let p = self
                    .parcels_until(client, |p| p.parcels.len() == n + 1)
                    .await;
                judge_divide(p.as_ref(), &mine, 248.0, y0 + 8.0, me, n)
            }
        };
        let edge_divided = note.is_empty();
        self.emit(client, "land-divide-edge", edge_divided, t, note);

        // both back into the bot's parcel
        if divided {
            let t = Instant::now();
            let note = self
                .join_back(
                    client,
                    Rect::new(x0, y0, x0 + 20.0, y0 + 16.0),
                    x0 + 8.0,
                    y0 + 8.0,
                    x0 + 24.0,
                    y0 + 24.0,
                )
                .await;
            self.emit(client, "land-join", note.is_empty(), t, note);
        }
        if edge_divided {
            let t = Instant::now();
            let note = self
                .join_back(
                    client,
                    Rect::new(236.0, y0, 256.0, y0 + 16.0),
                    248.0,
                    y0 + 8.0,
                    232.0,
                    y0 + 8.0,
                )
                .await;
            self.emit(client, "land-join-edge", note.is_empty(), t, note);
        }
        let t = Instant::now();
        let after = client.region_parcels().await.map(|p| shape(&p));
        let note = match after {
            Ok(a)
                if a.len() == before.len()
                    && a.iter().map(|p| p.1).sum::<i32>()
                        == before.iter().map(|p| p.1).sum::<i32>() =>
            {
                String::new()
            }
            Ok(a) => format!("the parcels were {before:?}, now {a:?}"),
            Err(e) => e.to_string(),
        };
        self.emit(client, "land-as-before", note.is_empty(), t, note);

        // the owner's own ban list: a stranger in, then out (what land-guard tries in the
        // owner's name)
        let t = Instant::now();
        let note = match self.ban_stranger(client, &mine, None).await {
            Ok(true) => String::new(),
            Ok(false) => "the stranger never showed in the ban list".into(),
            Err(e) => e,
        };
        self.emit(client, "land-ban", note.is_empty(), t, note);

        // the ground: raised, lowered, reverted at the square's north-west quarter's middle
        let (gx, gy) = (x0 + 8.0, y0 + 24.0);
        let Some(h0) = client.ground_height(gx, gy) else {
            return;
        };
        let point = Rect::new(gx, gy, gx, gy);
        let t = Instant::now();
        let note = match self
            .terraform(client, action::RAISE, point, RAISE, -1)
            .await
        {
            Err(e) => e,
            Ok(()) => {
                let h = self
                    .ground_until(client, gx, gy, GROUND_WAIT, |h| h > h0 + RAISE / 2.0)
                    .await;
                match h {
                    Some(h) if h > h0 + RAISE / 2.0 => String::new(),
                    h => format!("the ground stayed at {h:?} (was {h0:.2})"),
                }
            }
        };
        let raised = note.is_empty();
        self.emit(client, "land-raise", raised, t, note);
        if raised {
            let high = client.ground_height(gx, gy).unwrap_or(h0);
            let t = Instant::now();
            let note = match self
                .terraform(client, action::LOWER, point, RAISE, -1)
                .await
            {
                Err(e) => e,
                Ok(()) => {
                    let h = self
                        .ground_until(client, gx, gy, GROUND_WAIT, |h| h < high - RAISE / 2.0)
                        .await;
                    match h {
                        Some(h) if h < high - RAISE / 2.0 => String::new(),
                        h => format!("the ground stayed at {h:?} (was {high:.2})"),
                    }
                }
            };
            self.emit(client, "land-lower", note.is_empty(), t, note);
        }
        // raised again, then the quarter back to the region's baked ground: where it was,
        // unless it was edited before this run
        let t = Instant::now();
        let quarter = Rect::new(x0, y0 + 16.0, x0 + 16.0, y0 + 32.0);
        let lifted = match self
            .terraform(client, action::RAISE, point, RAISE, -1)
            .await
        {
            Ok(()) => self
                .ground_until(client, gx, gy, GROUND_WAIT, |h| h > h0 + RAISE / 2.0)
                .await
                .is_some_and(|h| h > h0 + RAISE / 2.0),
            Err(_) => false,
        };
        let note = if !lifted {
            "the ground did not rise to be reverted".into()
        } else {
            match self
                .terraform(client, action::REVERT, quarter, 1.0, -1)
                .await
            {
                Err(e) => e,
                Ok(()) => {
                    let h = self
                        .ground_until(client, gx, gy, GROUND_WAIT, |h| {
                            (h - h0).abs() < GROUND_EPSILON
                        })
                        .await;
                    match h {
                        Some(h) if (h - h0).abs() < GROUND_EPSILON => String::new(),
                        h => format!(
                            "reverted to {h:?}, the ground was {h0:.2} (baked differently, or \
                             not reverted)"
                        ),
                    }
                }
            }
        };
        self.emit(client, "land-revert", note.is_empty(), t, note);
    }

    /// Join the parcels `r` touches, then judge: one parcel covers both points, the bot's.
    async fn join_back(
        &self,
        client: &Client,
        r: Rect,
        ax: f32,
        ay: f32,
        bx: f32,
        by: f32,
    ) -> String {
        if let Err(e) = self.join(client, r).await {
            return e;
        }
        let one = |p: &RegionParcels| match (at(p, ax, ay), at(p, bx, by)) {
            (Some(a), Some(b)) => a.local_id == b.local_id,
            _ => false,
        };
        match self.parcels_until(client, one).await {
            Some(p) if one(&p) => String::new(),
            Some(p) => format!(
                "still two parcels: {:?} and {:?}",
                at(&p, ax, ay).map(|q| (q.local_id, q.area)),
                at(&p, bx, by).map(|q| (q.local_id, q.area))
            ),
            None => "the region did not answer".into(),
        }
    }

    /// `land-guard`: another resident's land. Dividing, joining, terraforming it and changing
    /// its ban list in its owner's name must change nothing.
    pub(super) async fn land_guard(&mut self, client: &Client) {
        let Some(start) = self.land_setup(client, "guard-setup").await else {
            return;
        };
        let (x0, y0) = self.land_square();
        let me = client.state().agent_id;
        let t = Instant::now();
        let Some(theirs) = at(&start, x0 + 16.0, y0 + 16.0).cloned() else {
            self.emit(client, "guard-parcels", false, t, "no parcel there".into());
            return;
        };
        if theirs.owner_id == me {
            self.emit(
                client,
                "guard-parcels",
                false,
                t,
                "the bot owns this land: land-guard needs an account that does not".into(),
            );
            return;
        }
        let before = shape(&start);
        self.emit(client, "guard-parcels", true, t, String::new());

        let t = Instant::now();
        let note = match self
            .divide(client, Rect::new(x0, y0, x0 + 16.0, y0 + 16.0))
            .await
        {
            Err(e) => e,
            Ok(()) => self.still(client, &before).await,
        };
        self.emit(client, "guard-divide", note.is_empty(), t, note);

        let t = Instant::now();
        let note = match self.join(client, Rect::new(0.0, 0.0, 256.0, 256.0)).await {
            Err(e) => e,
            Ok(()) => self.still(client, &before).await,
        };
        self.emit(client, "guard-join", note.is_empty(), t, note);

        let (gx, gy) = (x0 + 8.0, y0 + 24.0);
        let t = Instant::now();
        if theirs.flags & PF_ALLOW_TERRAFORM != 0 {
            self.emit(
                client,
                "guard-terraform",
                true,
                t,
                "skipped: the parcel lets anyone edit its ground".into(),
            );
        } else if let Some(h0) = client.ground_height(gx, gy) {
            let note = match self
                .terraform(client, action::RAISE, Rect::new(gx, gy, gx, gy), RAISE, -1)
                .await
            {
                Err(e) => e,
                Ok(()) => {
                    let h = self
                        .ground_until(client, gx, gy, NO_CHANGE_WAIT, |h| {
                            (h - h0).abs() > GROUND_EPSILON
                        })
                        .await;
                    match h {
                        Some(h) if (h - h0).abs() > GROUND_EPSILON => {
                            format!("a visitor moved the ground from {h0:.2} to {h:.2}")
                        }
                        _ => String::new(),
                    }
                }
            };
            self.emit(client, "guard-terraform", note.is_empty(), t, note);
        }

        // the ban list changed in the owner's name: the region must go by the circuit's agent
        let t = Instant::now();
        let note = match self
            .ban_stranger(client, &theirs, Some(theirs.owner_id))
            .await
        {
            Ok(false) => String::new(),
            Ok(true) => {
                "a visitor changed the ban list by naming the owner in ParcelAccessListUpdate"
                    .into()
            }
            Err(e) => e,
        };
        self.emit(client, "guard-ban-as-owner", note.is_empty(), t, note);
    }

    /// The parcels after a change that must not come: the same as `before`.
    async fn still(&self, client: &Client, before: &[(i32, i32)]) -> String {
        tokio::time::sleep(NO_CHANGE_WAIT / 2).await;
        match client.region_parcels().await {
            Ok(p) if shape(&p) == before => String::new(),
            Ok(p) => format!(
                "a visitor changed the parcels: {before:?} became {:?}",
                shape(&p)
            ),
            Err(e) => e.to_string(),
        }
    }

    /// A stranger added to the parcel's ban list (in `claim`'s name when given, else the bot's)
    /// and, when it went in, taken out again the same way: whether it went in.
    async fn ban_stranger(
        &self,
        client: &Client,
        parcel: &ParcelInfo,
        claim: Option<Uuid>,
    ) -> Result<bool, String> {
        let list = client
            .parcel_access_list(parcel.local_id, parcels::AL_BAN)
            .await
            .map_err(|e| format!("the ban list: {e}"))?;
        let stranger = Uuid::new_v4();
        let mut banned = list.clone();
        banned.push(parcels::AccessEntry {
            id: stranger,
            time: 0,
            flags: parcels::AL_BAN,
        });
        let send = |entries: Vec<parcels::AccessEntry>| async move {
            client.faults().claim_agent(claim);
            let sent = client
                .parcel_act(ParcelAction::AccessList {
                    local_id: parcel.local_id,
                    flags: parcels::AL_BAN,
                    entries,
                })
                .await;
            client.faults().claim_agent(None);
            sent.map_err(|e| e.to_string())
        };
        send(banned).await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let now = client
            .parcel_access_list(parcel.local_id, parcels::AL_BAN)
            .await
            .map_err(|e| format!("the ban list again: {e}"))?;
        if !now.iter().any(|e| e.id == stranger) {
            return Ok(false);
        }
        send(list).await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let after = client
            .parcel_access_list(parcel.local_id, parcels::AL_BAN)
            .await
            .map_err(|e| format!("the ban list at the end: {e}"))?;
        if after.iter().any(|e| e.id == stranger) {
            return Err(format!(
                "the stranger went in, but the change taking it out was lost ({} still banned)",
                after.len()
            ));
        }
        Ok(true)
    }

    /// `land-faults`: what a viewer never sends. A divide of no land must make no parcel; a
    /// terraform kept to a parcel that does not exist must leave the land tools working.
    pub(super) async fn land_faults(&mut self, client: &Client) {
        let Some(start) = self.land_setup(client, "faults-setup").await else {
            return;
        };
        let (x0, y0) = self.land_square();
        let me = client.state().agent_id;
        let t = Instant::now();
        let Some(mine) = at(&start, x0 + 16.0, y0 + 16.0).cloned() else {
            self.emit(client, "faults-parcels", false, t, "no parcel there".into());
            return;
        };
        if mine.owner_id != me {
            self.emit(
                client,
                "faults-parcels",
                false,
                t,
                format!("the land at ({x0}, {y0}) needs to be the bot's"),
            );
            return;
        }
        self.emit(client, "faults-parcels", true, t, String::new());

        // a divide of a point: no land, so no parcel; one that comes has no cell on the map,
        // only its local id answers (ParcelDwellRequest). Ghosts of earlier runs are stored:
        // only a new one counts
        let t = Instant::now();
        let note = match self.ghosts(client).await {
            Err(e) => e,
            Ok(old) => match self
                .divide(
                    client,
                    Rect::new(x0 + 20.0, y0 + 20.0, x0 + 20.0, y0 + 20.0),
                )
                .await
            {
                Err(e) => e,
                Ok(()) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    match self.ghosts(client).await {
                        Err(e) => e,
                        Ok(now) => {
                            let new: Vec<i32> =
                                now.into_iter().filter(|g| !old.contains(g)).collect();
                            if new.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    "a parcel of no land, local id {new:?}, kept by the region \
                                     and stored ({} from earlier runs)",
                                    old.len()
                                )
                            }
                        }
                    }
                }
            },
        };
        self.emit(client, "faults-divide-nothing", note.is_empty(), t, note);

        // a terraform kept to a parcel that does not exist, then the brush on the bot's land
        let t = Instant::now();
        let quarter = Rect::new(x0, y0 + 16.0, x0 + 16.0, y0 + 32.0);
        let sent = self
            .terraform(client, action::RAISE, quarter, RAISE, 0x7fff)
            .await;
        self.emit(
            client,
            "faults-terraform-no-parcel",
            sent.is_ok(),
            t,
            sent.err().unwrap_or_default(),
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (gx, gy) = (x0 + 8.0, y0 + 24.0);
        let Some(h0) = client.ground_height(gx, gy) else {
            return;
        };
        let t = Instant::now();
        let point = Rect::new(gx, gy, gx, gy);
        let note = match self
            .terraform(client, action::RAISE, point, RAISE, -1)
            .await
        {
            Err(e) => e,
            Ok(()) => {
                let h = self
                    .ground_until(client, gx, gy, GROUND_WAIT, |h| h > h0 + RAISE / 2.0)
                    .await;
                match h {
                    Some(h) if h > h0 + RAISE / 2.0 => String::new(),
                    h => format!(
                        "the land tools stopped working: the ground stayed at {h:?} (was \
                         {h0:.2}); the region needs a restart"
                    ),
                }
            }
        };
        let works = note.is_empty();
        self.emit(client, "faults-terraform-after", works, t, note);
        if works {
            let _ = self
                .terraform(client, action::REVERT, quarter, 1.0, -1)
                .await;
        }
    }

    /// Parcels with no land: local ids the region answers for that no cell of its map has.
    async fn ghosts(&self, client: &Client) -> Result<Vec<i32>, String> {
        let p = client.region_parcels().await.map_err(|e| e.to_string())?;
        let known: Vec<i32> = p.parcels.iter().map(|q| q.local_id).collect();
        let top = known.iter().copied().max().unwrap_or(0);
        let mut ghosts = Vec::new();
        for id in 1..=top + 4 {
            if known.contains(&id) {
                continue;
            }
            if let Ok(Ok(_)) =
                tokio::time::timeout(Duration::from_secs(2), client.parcel_dwell(id)).await
            {
                ghosts.push(id);
            }
        }
        Ok(ghosts)
    }
}

/// A divide judged: one more parcel, the bot's, covering the point, the parent smaller by it.
fn judge_divide(
    p: Option<&RegionParcels>,
    parent: &ParcelInfo,
    x: f32,
    y: f32,
    me: Uuid,
    before: usize,
) -> String {
    let Some(p) = p else {
        return "the region did not answer".into();
    };
    if p.parcels.len() != before + 1 {
        return format!("{} parcels, {} expected", p.parcels.len(), before + 1);
    }
    let Some(new) = at(p, x, y) else {
        return "no parcel at the place divided".into();
    };
    if new.local_id == parent.local_id {
        return "the place divided is still the bot's parcel".into();
    }
    if new.area != 256 {
        return format!("the new parcel is {} m², 256 expected", new.area);
    }
    if new.owner_id != me {
        return format!("the new parcel is {}'s", new.owner_id);
    }
    String::new()
}
