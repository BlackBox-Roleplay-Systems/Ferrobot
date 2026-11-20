//! Bakes on Mesh and the bake channels an outfit adds and takes away, with the probe's own test
//! outfit: a Universal layer painting the left arm, the left leg and the three aux channels, and a
//! skirt, each texture a solid colour of its own (fixtures/textures, made by make-textures.py).
//! Worn, the region must bake those channels, the appearance service must serve them, and they
//! must stay through a teleport; taken off, the channels must be cleared, while the old bakes stay
//! in the asset service for whatever still names them (an NPC saved from the outfit).

use super::Probe;
use ferro_client::appearance::{Appearance, DEFAULT_AVATAR_TEXTURE};
use ferro_client::wearable::{self, slot};
use ferro_client::{Client, Event};
use glam::Vec3;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

/// The channels the test outfit paints: the skirt (19), then left arm, left leg, aux 1 to 3.
const OUTFIT_FACES: [usize; 6] = [19, 40, 41, 42, 43, 44];
/// How long a bake may take.
const BAKE_WAIT: Duration = Duration::from_secs(60);
/// How long without a new appearance before the latest is the one a viewer keeps.
const SETTLE: Duration = Duration::from_secs(10);

const TEXTURES: [(&str, &[u8]); 6] = [
    ("aux1", include_bytes!("../../fixtures/textures/aux1.j2k")),
    ("aux2", include_bytes!("../../fixtures/textures/aux2.j2k")),
    ("aux3", include_bytes!("../../fixtures/textures/aux3.j2k")),
    (
        "leftarm",
        include_bytes!("../../fixtures/textures/leftarm.j2k"),
    ),
    (
        "leftleg",
        include_bytes!("../../fixtures/textures/leftleg.j2k"),
    ),
    ("skirt", include_bytes!("../../fixtures/textures/skirt.j2k")),
];

impl Probe {
    pub(super) async fn bom(&mut self, client: &Client) {
        let me = client.state().agent_id;

        // a test outfit an earlier run left on would stay worn through this one's taking off
        let t = Instant::now();
        let leftover: Vec<Uuid> = client
            .outfit_items()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.name.starts_with("Probe "))
            .map(|i| i.id)
            .collect();
        if !leftover.is_empty() {
            let mut events = client.subscribe();
            let ok = client.remove_from_outfit(&leftover).await.is_ok();
            settled(&mut events, client, me).await;
            self.emit(
                client,
                "bom-leftover-taken-off",
                ok,
                t,
                format!("{} items of an earlier run", leftover.len()),
            );
        }

        let t = Instant::now();
        let outfit = match make_outfit(client, self.round).await {
            Ok(o) => {
                self.emit(client, "bom-outfit-made", true, t, String::new());
                o
            }
            Err(e) => {
                self.emit(client, "bom-outfit-made", false, t, e);
                return;
            }
        };

        // worn: every channel of the outfit baked
        let mut events = client.subscribe();
        let t = Instant::now();
        let worn = match client.add_to_outfit(outfit.items.clone(), false).await {
            Ok(()) => settled(&mut events, client, me).await,
            Err(e) => {
                self.emit(client, "bom-worn", false, t, format!("not worn: {e}"));
                return;
            }
        };
        let Some(worn) = worn else {
            self.emit(client, "bom-worn", false, t, "no appearance".into());
            return;
        };
        let missing = unbaked(&worn);
        self.emit(
            client,
            "bom-worn",
            missing.is_empty(),
            t,
            if missing.is_empty() {
                String::new()
            } else {
                format!("faces not baked: {missing:?}")
            },
        );
        self.fetch_from_service(client, "bom-fetch-from-appearance-service", me, &worn)
            .await;

        // a teleport: the destination's arrival bake must keep every channel
        if self.opts.neighbours {
            let (home, near) = (self.opts.home.clone(), self.opts.near.clone());
            let middle = Vec3::new(128.0, 128.0, 30.0);
            for region in [near, home] {
                let mut events = client.subscribe();
                if !self
                    .teleport(client, &format!("bom-tp-to-{region}"), &region, middle)
                    .await
                {
                    return;
                }
                let t = Instant::now();
                let after = settled_from(&mut events, client, me, current(client, me)).await;
                let (ok, note) = match &after {
                    None => (false, "no appearance".to_owned()),
                    Some(a) => {
                        let lost = unbaked(a);
                        (
                            lost.is_empty(),
                            if lost.is_empty() {
                                String::new()
                            } else {
                                format!("faces lost after the teleport: {lost:?}")
                            },
                        )
                    }
                };
                self.emit(client, &format!("bom-kept-in-{region}"), ok, t, note);
            }
        }

        // taken off: the channels cleared, the old bakes kept
        let mut events = client.subscribe();
        let t = Instant::now();
        let ids: Vec<Uuid> = outfit.items.iter().map(|i| i.id).collect();
        let off = match client.remove_from_outfit(&ids).await {
            Ok(()) => settled(&mut events, client, me).await,
            Err(e) => {
                self.emit(
                    client,
                    "bom-taken-off",
                    false,
                    t,
                    format!("not taken off: {e}"),
                );
                return;
            }
        };
        let (ok, note) = match &off {
            None => (false, "no appearance".to_owned()),
            Some(a) => {
                let still: Vec<usize> = OUTFIT_FACES
                    .iter()
                    .copied()
                    .filter(|f| baked(a, *f))
                    .collect();
                (
                    still.is_empty(),
                    if still.is_empty() {
                        String::new()
                    } else {
                        format!("faces still baked: {still:?}")
                    },
                )
            }
        };
        self.emit(client, "bom-taken-off", ok, t, note);

        let t = Instant::now();
        let mut gone = Vec::new();
        for f in OUTFIT_FACES {
            let id = worn.faces[f];
            if client.viewer_asset("texture_id", id).await.is_err() {
                gone.push(f);
            }
        }
        self.emit(
            client,
            "bom-old-bakes-kept",
            gone.is_empty(),
            t,
            if gone.is_empty() {
                String::new()
            } else {
                format!("the bakes of faces {gone:?} are gone from the asset service")
            },
        );
    }
}

struct Outfit {
    items: Vec<ferro_client::inventory::InvItem>,
}

/// The textures uploaded and the two wearables made from them, in a folder of their own.
async fn make_outfit(client: &Client, round: u32) -> Result<Outfit, String> {
    let root = client.state().inventory_root;
    let folder = client
        .create_folder(root, &format!("Probe outfit {round}"))
        .await
        .map_err(|e| format!("folder: {e}"))?;
    let mut tex = std::collections::HashMap::new();
    for (name, bytes) in TEXTURES {
        let up = client
            .upload_texture(folder, &format!("Probe {name}"), bytes.to_vec())
            .await
            .map_err(|e| format!("texture {name}: {e}"))?;
        tex.insert(name, up.asset);
    }
    let layer = wearable::asset(
        "Probe BoM layer",
        wearable::UNIVERSAL,
        &[],
        &[
            (slot::LEFT_ARM_TATTOO, tex["leftarm"]),
            (slot::LEFT_LEG_TATTOO, tex["leftleg"]),
            (slot::AUX1_TATTOO, tex["aux1"]),
            (slot::AUX2_TATTOO, tex["aux2"]),
            (slot::AUX3_TATTOO, tex["aux3"]),
        ],
    );
    let skirt = wearable::asset(
        "Probe skirt",
        wearable::SKIRT,
        &[],
        &[(slot::SKIRT, tex["skirt"])],
    );
    let mut items = Vec::new();
    for (name, kind, data) in [
        ("Probe BoM layer", wearable::UNIVERSAL, layer),
        ("Probe skirt", wearable::SKIRT, skirt),
    ] {
        let id = client
            .create_wearable(folder, name, wearable::ASSET_CLOTHING, kind, data)
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        let item = client
            .inventory_item(id)
            .await
            .map_err(|e| format!("{name}: {e}"))?
            .ok_or_else(|| format!("{name}: the new item is not in the inventory"))?;
        items.push(item);
    }
    Ok(Outfit { items })
}

fn baked(a: &Appearance, face: usize) -> bool {
    a.faces
        .get(face)
        .is_some_and(|id| !id.is_nil() && *id != DEFAULT_AVATAR_TEXTURE)
}

/// The outfit's faces without a bake.
fn unbaked(a: &Appearance) -> Vec<usize> {
    OUTFIT_FACES
        .iter()
        .copied()
        .filter(|f| !baked(a, *f))
        .collect()
}

fn current(client: &Client, agent: Uuid) -> Option<Appearance> {
    client.state().appearances.get(&agent).cloned()
}

/// The latest appearance once the region has sent none for `SETTLE`, the first waited for as long
/// as a bake takes (`BAKE_WAIT` in all), or None when none came at all.
async fn settled(events: &mut Receiver<Event>, client: &Client, agent: Uuid) -> Option<Appearance> {
    settled_from(events, client, agent, None).await
}

async fn settled_from(
    events: &mut Receiver<Event>,
    client: &Client,
    agent: Uuid,
    mut latest: Option<Appearance>,
) -> Option<Appearance> {
    let t = Instant::now();
    while t.elapsed() < BAKE_WAIT {
        // the first may take a whole bake; after it, a quiet SETTLE means the appearance stays
        let wait = if latest.is_none() { BAKE_WAIT } else { SETTLE };
        let next = tokio::time::timeout(wait, async {
            loop {
                match events.recv().await {
                    Ok(Event::Appearance { agent: a, .. }) if a == agent => return true,
                    Err(RecvError::Closed) => return false,
                    _ => {}
                }
            }
        })
        .await;
        match next {
            Ok(true) => latest = current(client, agent),
            _ => break,
        }
    }
    latest
}
