//! A fake Second Life grid on localhost, for tests: a login server (XML-RPC), two regions (UDP
//! circuits, seed capabilities, event queues), and teleports between them.
//!
//! It answers the way the grid does, as far as the bot uses it today: the login reply of
//! `login_to_simulator`, UseCircuitCode then RegionHandshake, CompleteAgentMovement then
//! AgentMovementComplete, pings both ways, LogoutRequest then LogoutReply, the seed request and
//! the held `EventQueueGet` poll (502 when nothing came, as the grid's queue does), the agent's
//! data and groups, names, avatar searches, and teleports (TeleportStart, then TeleportFinish on
//! the event queue, or TeleportFailed).
//!
//! A test starts it, points the bot's login address at [`TestGrid::login_uri`], then watches what
//! the bot sends ([`Seen`]) and acts as the grid would: chat, IMs, teleport offers, a kick, a
//! closed circuit, a region gone silent, a failing event queue. Each feature that talks to a
//! region adds what it needs here (ROADMAP phase 1).

mod caps;
pub mod discord;
mod groups;
pub mod inventory;

pub use groups::{BUILDERS, OFFICERS, OWNERS, SECOND_GROUP, TEST_GROUP, THIRD_GROUP};
pub use region::{
    MAPPABLE_FRIEND, OBJECT, OBJECT_CHILD, OBJECT_CREATOR, OBJECT_LOCAL_ID, OBJECT_NAME,
    OBJECT_OWNER, OBJECT_POSITION, SEAT_POSITION, map_tile,
};
mod http;
mod login;
pub mod parcels;
mod region;
pub mod sound;
mod xmlrpc;

use ferro_llsd::Llsd;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

/// The login region and the region teleports go to.
pub const FIRST_REGION: usize = 0;
pub const SECOND_REGION: usize = 1;

/// The account the grid knows, and how it behaves.
#[derive(Debug, Clone)]
pub struct GridConfig {
    pub first: String,
    pub last: String,
    pub password: String,
    /// The login region, at (1000, 1000) on the grid.
    pub region_name: String,
    /// Its neighbour to the east, at (1001, 1000), where teleports go.
    pub second_region_name: String,
    /// How long the event queue holds a poll with nothing to send before its 502.
    pub eq_hold: Duration,
    /// How often the region pings the bot.
    pub ping_interval: Duration,
    /// Other avatars the grid knows, for names and searches.
    pub residents: Vec<Resident>,
    /// The groups: the bot is in some, can be invited to the others.
    pub groups: Vec<GridGroup>,
    /// The regions give the GroupMemberData capability (else members come by UDP).
    pub member_data_cap: bool,
    pub active_group: Option<Uuid>,
    /// The login reply's `home`, as the grid writes it (LLSD notation in a string).
    pub home: String,
    /// Display names some residents chose; the others have the default one.
    pub display_names: Vec<(Uuid, String)>,
    /// The regions give the GetDisplayNames capability (OpenSim may not).
    pub display_names_cap: bool,
    /// The regions give the AgentProfile capability, as Second Life's do (OpenSim's do not:
    /// profiles and picks then come by UDP).
    pub agent_profile_cap: bool,
    /// What uploading a sound costs, in the login reply's `account_level_benefits` (Second
    /// Life's); `None`: no benefits in the reply, as OpenSim's.
    pub sound_upload_cost: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct Resident {
    pub id: Uuid,
    pub first: String,
    pub last: String,
}

/// A group: its roles, its members, and the bot's place in it.
#[derive(Debug, Clone)]
pub struct GridGroup {
    pub id: Uuid,
    pub name: String,
    /// The bot's title and powers in it.
    pub title: String,
    pub powers: u64,
    /// The bot is in it (else it can be invited, [`TestGrid::invite_bot`]).
    pub bot_member: bool,
    /// The bot's roles, besides "everyone".
    pub bot_roles: Vec<Uuid>,
    /// The roles; "everyone" (the nil id) first.
    pub roles: Vec<GridRole>,
    /// The members other than the bot.
    pub members: Vec<GridMember>,
}

#[derive(Debug, Clone)]
pub struct GridRole {
    pub id: Uuid,
    pub name: String,
    pub title: String,
    pub powers: u64,
}

#[derive(Debug, Clone)]
pub struct GridMember {
    pub id: Uuid,
    /// Roles besides "everyone".
    pub roles: Vec<Uuid>,
    pub owner: bool,
    /// "Online", or the last login as MM/DD/YYYY.
    pub last_login: String,
}

impl Default for GridConfig {
    fn default() -> Self {
        GridConfig {
            first: "Test".into(),
            last: "Bot".into(),
            password: "a test password".into(),
            region_name: "Test Region".into(),
            second_region_name: "Second Region".into(),
            eq_hold: Duration::from_secs(15),
            ping_interval: Duration::from_secs(2),
            residents: vec![
                Resident {
                    id: Uuid::from_u128(0x5eed_0001),
                    first: "Some".into(),
                    last: "Resident".into(),
                },
                Resident {
                    id: Uuid::from_u128(0x5eed_0002),
                    first: "Other".into(),
                    last: "Avatar".into(),
                },
            ],
            groups: groups::default_groups(),
            active_group: Some(Uuid::from_u128(0x6000_0001)),
            // in the second region
            home: "{'region_handle':[r256256, r256000], 'position':[r100, r110, r22], 'look_at':[r1, r0, r0]}".into(),
            display_names: vec![(Uuid::from_u128(0x5eed_0001), "Sunny".into())],
            display_names_cap: true,
            member_data_cap: true,
            agent_profile_cap: false,
            sound_upload_cost: Some(10),
        }
    }
}

/// What the grid saw the bot do, in order. `session` counts the logins, from 1; `region` is
/// [`FIRST_REGION`] or [`SECOND_REGION`].
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    Login {
        session: u32,
        first: String,
        last: String,
        start: String,
        channel: String,
    },
    LoginRefused {
        reason: String,
    },
    UseCircuitCode {
        session: u32,
        region: usize,
    },
    CompleteAgentMovement {
        session: u32,
        region: usize,
    },
    AgentThrottle {
        session: u32,
    },
    RegionHandshakeReply {
        session: u32,
        flags: u32,
    },
    AgentUpdate {
        session: u32,
    },
    CompletePingCheck {
        session: u32,
    },
    LogoutRequest {
        session: u32,
    },
    AgentDataUpdateRequest {
        session: u32,
    },
    UuidNameRequest {
        session: u32,
        ids: Vec<Uuid>,
    },
    AvatarPickerRequest {
        session: u32,
        name: String,
    },
    DisplayNamesRequest {
        session: u32,
        ids: Vec<Uuid>,
    },
    GroupRolesRequest {
        group: Uuid,
    },
    /// By UDP (`cap` false) or the GroupMemberData capability.
    GroupMembersRequest {
        group: Uuid,
        cap: bool,
    },
    GroupRoleMembersRequest {
        group: Uuid,
    },
    GroupTitlesRequest {
        group: Uuid,
    },
    GroupInvite {
        group: Uuid,
        invitee: Uuid,
        role: Uuid,
    },
    GroupEject {
        group: Uuid,
        member: Uuid,
        done: bool,
    },
    GroupRoleChange {
        group: Uuid,
        role: Uuid,
        member: Uuid,
        add: bool,
    },
    GroupInviteAnswer {
        group: Uuid,
        accept: bool,
    },
    GroupChatStart {
        group: Uuid,
    },
    /// A group notice: "subject|message", and whether an item came with it.
    GroupNotice {
        group: Uuid,
        message: String,
        attached: bool,
    },
    GroupTitleUpdate {
        group: Uuid,
        role: Uuid,
    },
    ActivateGroup {
        group: Uuid,
    },
    JoinGroup {
        group: Uuid,
    },
    LeaveGroup {
        group: Uuid,
    },
    /// GroupAPIv1: a ban (`ban`) or an unban.
    GroupBanChange {
        group: Uuid,
        ids: Vec<Uuid>,
        ban: bool,
    },
    /// ChatSessionRequest "accept invitation".
    GroupChatAccept {
        group: Uuid,
    },
    GroupChatSend {
        group: Uuid,
        message: String,
    },
    GroupChatLeave {
        group: Uuid,
    },
    /// The bot spoke in local chat.
    Chat {
        message: String,
        channel: i32,
        chat_type: u8,
    },
    /// The bot sent a plain IM.
    Im {
        to: Uuid,
        message: String,
    },
    /// The bot answered an offer by IM (inventory accepted or declined, a teleport request
    /// declined): the dialog and the offer's id.
    ImAnswer {
        to: Uuid,
        dialog: u8,
        id: Uuid,
    },
    FriendshipAccepted {
        transaction: Uuid,
    },
    /// The bot offered a teleport (StartLure).
    StartLure {
        target: Uuid,
    },
    /// The bot asked for its outfit to be baked (UpdateAvatarAppearance).
    AppearanceRequest {
        cof_version: i32,
    },
    /// The bot offered an inventory item or folder (IM_INVENTORY_OFFERED): the offer's name and
    /// the bucket's (asset type, key) entries.
    InventoryOffer {
        to: Uuid,
        name: String,
        entries: Vec<(u8, Uuid)>,
    },
    CreateInventoryFolder {
        id: Uuid,
        parent: Uuid,
        name: String,
    },
    CreateInventoryItem {
        item: Uuid,
        folder: Uuid,
        name: String,
        asset_type: i8,
    },
    /// A notecard's asset uploaded (UpdateNotecardAgentInventory's uploader).
    NotecardSaved {
        item: Uuid,
        asset: String,
    },
    MoveInventoryItem {
        item: Uuid,
        folder: Uuid,
        name: String,
    },
    MoveInventoryFolder {
        folder: Uuid,
        parent: Uuid,
    },
    /// The control flags of the agent updates changed (fly, stand up, sit on the ground, stop).
    Controls {
        flags: u32,
    },
    /// The way the bot faces changed: a heading in degrees clockwise from north, rounded.
    Facing {
        heading: i32,
    },
    AgentRequestSit {
        target: Uuid,
    },
    AgentSit,
    AlwaysRun {
        on: bool,
    },
    SoundTrigger {
        asset: Uuid,
    },
    /// AvatarPropertiesUpdate: the bot's profile as it now is, and whether the parts not
    /// changed (the first life text, publishing) came back as they were.
    ProfileUpdate {
        about: String,
        image: Uuid,
        rest_kept: bool,
    },
    /// An avatar's picks asked for: by the GenericMessage "avatarpicksrequest", or the
    /// AgentProfile capability (`cap`).
    PicksRequest {
        avatar: Uuid,
        cap: bool,
    },
    /// CreateGroupRequest.
    CreateGroup {
        name: String,
        charter: String,
        show_in_list: bool,
        membership_fee: i32,
        open_enrollment: bool,
        allow_publish: bool,
    },
    /// An asset uploaded through NewFileAgentInventory and its uploader: what the request said
    /// of it, and the bytes received ([`checksum`]).
    AssetUploaded {
        asset_type: String,
        inventory_type: String,
        folder: Uuid,
        name: String,
        expected_cost: i32,
        next_owner_mask: i32,
        len: usize,
        checksum: u64,
    },
    /// A ViewerAsset download.
    ViewerAsset {
        asset: Uuid,
    },
    /// ViewerEffect: its type (15 point at, 7 beam) and its target.
    Effect {
        kind: u8,
        target: Uuid,
    },
    ScriptAnswer {
        task: Uuid,
        questions: i32,
    },
    /// Something done to an object, in words: "rez <item> at x,y,z bypass 1",
    /// "derez <local ids> to <destination> in <folder>", "grab <local id>", "degrab …",
    /// "select <local ids>", "deselect <local ids>",
    /// "contents <local id> <item>", "script <local id> <item> running <bool>".
    Object {
        what: String,
    },
    /// AgentAnimation: an animation started or stopped.
    Animation {
        id: Uuid,
        start: bool,
    },
    FindAgent {
        prey: Uuid,
    },
    MuteListRequest,
    /// ConfirmXferPacket: a packet of a file the grid sent.
    XferConfirmed {
        packet: u32,
    },
    ParcelPropertiesRequest {
        sequence: i32,
    },
    /// DirLandQuery: land for sale asked for.
    LandQuery {
        flags: u32,
        kind: u32,
        price: i32,
        area: i32,
        start: i32,
    },
    /// Something done to a parcel or on it, in words: "eject <key> <flags>", "freeze …",
    /// "return <type> <owners>", "deed <local id> <group>", "release", "buy", "reclaim".
    Parcel {
        what: String,
    },
    /// EstateOwnerMessage: its method and parameters.
    EstateMessage {
        method: String,
        params: Vec<String>,
    },
    /// A terrain file the bot uploaded by xfer, whole: its name, the size its first packet
    /// announced, the bytes received and their [`checksum`].
    TerrainUploaded {
        name: String,
        announced: usize,
        len: usize,
        checksum: u64,
    },
    /// A POST to an estate capability (regioninfo, estatechangeinfo), its body as XML.
    EstatePost {
        cap: String,
        body: String,
    },
    /// ParcelAccessListUpdate: a list (1 access, 2 bans) as it now is.
    AccessListUpdate {
        flags: u32,
        ids: Vec<Uuid>,
    },
    /// ParcelPropertiesUpdate: the parcel as it is now.
    ParcelUpdate {
        parcel: Box<parcels::GridParcel>,
    },
    /// An inventory asset asked for, by its item.
    TransferRequest {
        item: Uuid,
    },
    /// GenericMessage "autopilot": its parameters.
    AutoPilot {
        params: Vec<String>,
    },
    HoverHeight {
        height: String,
    },
    /// The bot paid: an avatar (5001) or an object (5008).
    Pay {
        to: Uuid,
        amount: i32,
        transaction_type: i32,
    },
    EndFriendship {
        other: Uuid,
    },
    GrantRights {
        to: Uuid,
        rights: i32,
    },
    /// The bot looked a region up by name (MapNameRequest).
    MapNameRequest {
        name: String,
    },
    /// The bot pressed a dialog's button (ScriptDialogReply).
    DialogReply {
        object_id: Uuid,
        channel: i32,
        index: i32,
        label: String,
    },
    TeleportLureRequest {
        session: u32,
        lure: Uuid,
    },
    TeleportLandmarkRequest {
        session: u32,
        landmark: Uuid,
    },
    TeleportLocationRequest {
        session: u32,
        region_handle: u64,
    },
    SeedRequest {
        session: u32,
        region: usize,
        caps: Vec<String>,
    },
    EventQueuePoll {
        session: u32,
        region: usize,
    },
}

/// One login: its ids, where its agent is, and its event queue.
struct Session {
    n: u32,
    session_id: Uuid,
    circuit_code: u32,
    /// Ended by a logout, a kick or a closed circuit: its capabilities answer 404.
    ended: bool,
    /// The region the agent is in, and the one a teleport sends it to.
    region: usize,
    expected: Option<usize>,
    /// Where the agent arrives in the next region (AgentMovementComplete).
    arrival: glam::Vec3,
    /// Events waiting for the next poll of the current region's queue.
    events: VecDeque<(String, Llsd)>,
    /// The next poll answers 404, as a region does when it closes the queue.
    close_queue: bool,
    /// Every poll fails at once (503), as a region whose queue is broken.
    fail_queue: bool,
    /// The region neither answers nor sends anything on the session's circuit, as a region that
    /// hangs or a network that drops everything.
    silent: bool,
    eq_id: i32,
}

struct State {
    sessions: HashMap<u32, Session>,
    seen: Vec<Seen>,
    /// Teleport offers sent to the bot, by lure id.
    lures: HashSet<Uuid>,
    /// Items offered to the bot, by the offer's transaction: their type and name.
    inventory_offers: HashMap<Uuid, (i8, String)>,
    /// The next teleport fails, with this reason.
    fail_teleport: Option<String>,
    /// The groups as they are now: ejections, role changes and accepted invitations change them.
    groups: Vec<GridGroup>,
    /// The test region's parcel, as updates change it.
    parcel: parcels::GridParcel,
    /// Its ban list.
    parcel_bans: Vec<Uuid>,
    /// Profiles as set (AvatarPropertiesUpdate): about text and picture, by avatar.
    profiles: HashMap<Uuid, (String, Uuid)>,
    /// Group invitations sent to the bot: transaction to group.
    group_invites: HashMap<Uuid, Uuid>,
    /// The group chats the bot is in.
    group_chats: HashSet<Uuid>,
    /// The bot's active group, as it changes it.
    active_group: Option<Uuid>,
    /// The banned avatars, by group.
    bans: HashMap<Uuid, Vec<Uuid>>,
    /// The bot's inventory.
    inventory: inventory::Inventory,
    /// The bot's L$.
    balance: i32,
    /// Files on the grid's web server (`/files/<name>`), for the commands that download one.
    files: HashMap<String, Vec<u8>>,
    /// Uploads offered by NewFileAgentInventory and not yet sent: their request, by the id in
    /// their uploader's address.
    asset_uploads: HashMap<Uuid, Llsd>,
}

/// The picks of the avatars the grid knows: Some Resident has two, the others none. `None` for an
/// avatar the grid does not know.
pub fn picks(avatar: Uuid) -> Option<Vec<(Uuid, String)>> {
    match avatar.as_u128() {
        0x5eed_0001 => Some(vec![
            (Uuid::from_u128(0x91c4_0001), "Sunny Beach".into()),
            (
                Uuid::from_u128(0x91c4_0002),
                "Club <Night> & \"Day\"".into(),
            ),
        ]),
        0x5eed_0002 => Some(Vec::new()),
        _ => None,
    }
}

/// FNV-1a: what [`Seen::TerrainUploaded`] says of the bytes, rather than all of them.
pub fn checksum(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A region as the others see it.
#[derive(Debug, Clone)]
struct RegionInfo {
    name: String,
    addr: SocketAddr,
    /// Corner on the grid, in metres.
    x: u32,
    y: u32,
}

impl RegionInfo {
    fn handle(&self) -> u64 {
        (u64::from(self.x) << 32) | u64::from(self.y)
    }
}

struct Shared {
    cfg: GridConfig,
    agent_id: Uuid,
    http_addr: SocketAddr,
    regions: Vec<RegionInfo>,
    state: Mutex<State>,
    /// Woken when an event is queued or a session ends, for the held polls.
    eq_wake: Notify,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        // a test that panicked while holding the lock leaves the state as it was: keep going
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn saw(&self, seen: Seen) {
        log::debug!("testgrid saw {seen:?}");
        self.state().seen.push(seen);
    }

    fn queue_event(&self, n: u32, message: &str, body: Llsd) {
        if let Some(s) = self.state().sessions.get_mut(&n) {
            s.events.push_back((message.into(), body));
        }
        self.eq_wake.notify_waiters();
    }

    /// The region says nothing on this session's circuit ([`TestGrid::silence_region`]).
    fn silent(&self, n: u32) -> bool {
        self.state().sessions.get(&n).is_some_and(|s| s.silent)
    }

    fn end_session(&self, n: u32) {
        if let Some(s) = self.state().sessions.get_mut(&n) {
            s.ended = true;
        }
        self.eq_wake.notify_waiters();
    }

    fn seed_url(&self, n: u32, region: usize) -> String {
        format!("http://{}/cap/{n}/{region}/seed", self.http_addr)
    }
}

/// What a test asks a region to send to the bot.
#[derive(Debug)]
enum RegionCommand {
    Say {
        from: String,
        text: String,
    },
    Im {
        from: String,
        text: String,
    },
    /// An IM with a sender key: a resident's (IM_NOTHING_SPECIAL), or an object's owner's
    /// (IM_FROM_TASK).
    ImFrom {
        id: Uuid,
        name: String,
        dialog: u8,
        text: String,
        session: Uuid,
        bucket: Vec<u8>,
    },
    OfferTeleport {
        from: Resident,
        lure: Uuid,
    },
    Dialog {
        object_id: Uuid,
        object_name: String,
        owner: Resident,
        message: String,
        buttons: Vec<String>,
        channel: i32,
    },
    Kick {
        reason: String,
    },
    DisableSimulator,
    /// A script asks the bot for permissions.
    ScriptQuestion {
        task: Uuid,
        owner: String,
        questions: i32,
    },
    RestartNotice {
        seconds: i32,
    },
    GroupInvitation {
        from: Resident,
        group: Uuid,
        transaction: Uuid,
        fee: i32,
    },
    GroupChat {
        from: Resident,
        group: Uuid,
        text: String,
    },
    /// A resident walks to a place, or leaves (`None`).
    MoveResident {
        index: usize,
        to: Option<glam::Vec3>,
    },
}

/// A running fake grid. Dropping it stops it.
pub struct TestGrid {
    shared: Arc<Shared>,
    regions: Vec<mpsc::UnboundedSender<RegionCommand>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for TestGrid {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl TestGrid {
    /// Start the login server and the regions on localhost, on free ports.
    pub async fn start(cfg: GridConfig) -> std::io::Result<TestGrid> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let sockets = [
            UdpSocket::bind("127.0.0.1:0").await?,
            UdpSocket::bind("127.0.0.1:0").await?,
        ];
        let regions = vec![
            RegionInfo {
                name: cfg.region_name.clone(),
                addr: sockets[0].local_addr()?,
                x: 1000 * 256,
                y: 1000 * 256,
            },
            RegionInfo {
                name: cfg.second_region_name.clone(),
                addr: sockets[1].local_addr()?,
                x: 1001 * 256,
                y: 1000 * 256,
            },
        ];
        let groups = cfg.groups.clone();
        let active_group = cfg.active_group;
        let shared = Arc::new(Shared {
            cfg,
            agent_id: Uuid::new_v4(),
            http_addr: listener.local_addr()?,
            regions,
            state: Mutex::new(State {
                sessions: HashMap::new(),
                seen: Vec::new(),
                lures: HashSet::new(),
                inventory_offers: HashMap::new(),
                fail_teleport: None,
                groups,
                parcel: parcels::GridParcel::default(),
                parcel_bans: Vec::new(),
                profiles: HashMap::from([(
                    Uuid::from_u128(0x5eed_0001),
                    ("I like tests".to_owned(), Uuid::from_u128(0x1a6e_0001)),
                )]),
                group_invites: HashMap::new(),
                group_chats: HashSet::new(),
                active_group,
                bans: HashMap::new(),
                inventory: inventory::Inventory::default(),
                balance: 500,
                files: HashMap::new(),
                asset_uploads: HashMap::new(),
            }),
            eq_wake: Notify::new(),
        });
        let mut tasks = vec![tokio::spawn(http::serve(listener, shared.clone()))];
        let mut senders = Vec::new();
        for (index, sock) in sockets.into_iter().enumerate() {
            let (tx, rx) = mpsc::unbounded_channel();
            tasks.push(tokio::spawn(region::run(index, sock, shared.clone(), rx)));
            senders.push(tx);
        }
        Ok(TestGrid {
            shared,
            regions: senders,
            tasks,
        })
    }

    /// The address to give the bot as its login server.
    pub fn login_uri(&self) -> String {
        format!("http://{}/login.cgi", self.shared.http_addr)
    }

    /// The account's name, "First Last".
    pub fn username(&self) -> String {
        format!("{} {}", self.shared.cfg.first, self.shared.cfg.last)
    }

    pub fn password(&self) -> &str {
        &self.shared.cfg.password
    }

    pub fn region_name(&self) -> &str {
        &self.shared.cfg.region_name
    }

    /// The handle of a region ([`FIRST_REGION`], [`SECOND_REGION`]).
    pub fn region_handle(&self, region: usize) -> u64 {
        self.shared.regions[region].handle()
    }

    /// Everything seen so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.shared.state().seen.clone()
    }

    /// How many of what was seen match.
    pub fn count(&self, matches: impl Fn(&Seen) -> bool) -> usize {
        self.shared
            .state()
            .seen
            .iter()
            .filter(|s| matches(s))
            .count()
    }

    /// Wait until `done` holds for what was seen; false when `timeout` passes first.
    pub async fn wait_until(&self, timeout: Duration, done: impl Fn(&[Seen]) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // a copy: `done` may ask the grid again, and its lock is not reentrant
            let seen = self.shared.state().seen.clone();
            if done(&seen) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait for the first thing seen that matches.
    pub async fn wait_for(&self, timeout: Duration, matches: impl Fn(&Seen) -> bool) -> bool {
        self.wait_until(timeout, |seen| seen.iter().any(&matches))
            .await
    }

    /// Someone says `text` in local chat, next to the bot.
    pub fn say(&self, from: &str, text: &str) {
        self.command(RegionCommand::Say {
            from: from.into(),
            text: text.into(),
        });
    }

    /// Someone sends the bot an IM.
    pub fn im(&self, from: &str, text: &str) {
        self.command(RegionCommand::Im {
            from: from.into(),
            text: text.into(),
        });
    }

    /// An object of `owner`'s shows the bot a dialog (llDialog). Returns the object's key.
    pub fn dialog(
        &self,
        object_name: &str,
        owner: &Resident,
        message: &str,
        buttons: &[&str],
        channel: i32,
    ) -> Uuid {
        let object_id = Uuid::new_v4();
        self.command(RegionCommand::Dialog {
            object_id,
            object_name: object_name.into(),
            owner: owner.clone(),
            message: message.into(),
            buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
            channel,
        });
        object_id
    }

    /// A resident sends the bot an IM, with their key as the grid gives it.
    pub fn im_from(&self, from: &Resident, text: &str) {
        self.command(RegionCommand::ImFrom {
            id: from.id,
            name: format!("{} {}", from.first, from.last),
            dialog: 0,
            text: text.into(),
            session: Uuid::new_v4(),
            bucket: Vec::new(),
        });
    }

    /// An object owned by `owner` sends the bot an IM (llInstantMessage: IM_FROM_TASK, the
    /// owner's key, the object's name).
    pub fn object_im(&self, owner: &Resident, object: &str, text: &str) {
        self.command(RegionCommand::ImFrom {
            id: owner.id,
            name: object.into(),
            dialog: 19,
            text: text.into(),
            session: Uuid::new_v4(),
            bucket: Vec::new(),
        });
    }

    fn offer(
        &self,
        from: &Resident,
        name: Option<&str>,
        dialog: u8,
        text: &str,
        bucket: Vec<u8>,
    ) -> Uuid {
        let session = Uuid::new_v4();
        self.command(RegionCommand::ImFrom {
            id: from.id,
            name: name.map_or_else(|| format!("{} {}", from.first, from.last), str::to_owned),
            dialog,
            text: text.into(),
            session,
            bucket,
        });
        session
    }

    /// A resident offers the bot friendship (IM_FRIENDSHIP_OFFERED). Returns its transaction.
    pub fn offer_friendship(&self, from: &Resident) -> Uuid {
        self.offer(from, None, 38, "Would you be my friend?", Vec::new())
    }

    /// A resident, or an object of theirs (`object`: its name), offers the bot an item
    /// (IM_INVENTORY_OFFERED, IM_TASK_INVENTORY_OFFERED). Returns its transaction.
    pub fn offer_inventory(&self, from: &Resident, object: Option<&str>) -> Uuid {
        // the bucket: the asset type, then the item's id
        let mut bucket = vec![7u8];
        bucket.extend_from_slice(Uuid::new_v4().as_bytes());
        let dialog = if object.is_some() { 9 } else { 4 };
        let session = self.offer(from, object, dialog, "A notecard", bucket);
        self.shared
            .state()
            .inventory_offers
            .insert(session, (7, "A notecard".into()));
        session
    }

    /// A resident asks the bot for a teleport to them (IM_TELEPORT_REQUEST).
    pub fn request_teleport(&self, from: &Resident) -> Uuid {
        self.offer(from, None, 26, "Please send me a teleport", Vec::new())
    }

    /// A resident offers the bot a teleport to them (IM_LURE_USER); accepted, it goes to the
    /// second region. Returns the lure's id.
    pub fn offer_teleport(&self, from: &Resident) -> Uuid {
        let lure = Uuid::new_v4();
        self.shared.state().lures.insert(lure);
        self.command(RegionCommand::OfferTeleport {
            from: from.clone(),
            lure,
        });
        lure
    }

    /// A resident invites the bot into a group it is not in (IM_GROUP_INVITATION), with this
    /// membership fee. Returns the invitation's transaction id.
    pub fn invite_bot(&self, from: &Resident, group: Uuid, fee: i32) -> Uuid {
        let transaction = Uuid::new_v4();
        self.shared.state().group_invites.insert(transaction, group);
        self.command(RegionCommand::GroupInvitation {
            from: from.clone(),
            group,
            transaction,
            fee,
        });
        transaction
    }

    /// A resident says something in a group's chat. The bot's first line of a chat it is not in
    /// comes as a ChatterBoxInvitation on the event queue, the others as IM_SESSION_SEND.
    pub fn group_chat(&self, from: &Resident, group: Uuid, text: &str) {
        let joined = self.shared.state().group_chats.contains(&group);
        if joined {
            self.command(RegionCommand::GroupChat {
                from: from.clone(),
                group,
                text: text.into(),
            });
            return;
        }
        let name = self
            .shared
            .state()
            .groups
            .iter()
            .find(|g| g.id == group)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let from_name = format!("{} {}", from.first, from.last);
        let mut bucket = name.clone().into_bytes();
        bucket.push(0);
        let mut data = ferro_llsd::Map::new();
        data.insert("binary_bucket".into(), Llsd::Binary(bucket));
        let mut params = ferro_llsd::Map::new();
        params.insert("from_id".into(), from.id.into());
        params.insert("from_name".into(), from_name.clone().into());
        params.insert("to_id".into(), self.shared.agent_id.into());
        params.insert("id".into(), group.into());
        params.insert("message".into(), text.into());
        params.insert("type".into(), 17.into());
        params.insert("from_group".into(), true.into());
        params.insert("offline".into(), 0.into());
        params.insert("timestamp".into(), 0.into());
        params.insert("data".into(), Llsd::Map(data));
        let mut im = ferro_llsd::Map::new();
        im.insert("message_params".into(), Llsd::Map(params));
        let mut body = ferro_llsd::Map::new();
        body.insert("session_id".into(), group.into());
        body.insert("session_name".into(), name.into());
        body.insert("from_name".into(), from_name.into());
        body.insert("instantmessage".into(), Llsd::Map(im));
        self.queue_event("ChatterBoxInvitation", Llsd::Map(body));
    }

    /// The members of a group as they are now (the bot included when it is in it).
    pub fn group_members(&self, group: Uuid) -> Vec<GridMember> {
        let st = self.shared.state();
        st.groups
            .iter()
            .find(|g| g.id == group)
            .map(|g| groups::all_members(g, self.shared.agent_id))
            .unwrap_or_default()
    }

    /// The bot's inventory as it is now.
    /// Put a file on the grid's web server; its URL.
    pub fn serve_file(&self, name: &str, data: Vec<u8>) -> String {
        self.shared.state().files.insert(name.to_owned(), data);
        format!("http://{}/files/{name}", self.shared.http_addr)
    }

    pub fn parcel(&self) -> parcels::GridParcel {
        self.shared.state().parcel.clone()
    }

    pub fn inventory(&self) -> inventory::Inventory {
        self.shared.state().inventory.clone()
    }

    /// The bot's agent id.
    pub fn agent_id(&self) -> Uuid {
        self.shared.agent_id
    }

    /// The next teleport fails (TeleportFailed on the event queue) with this reason.
    pub fn fail_next_teleport(&self, reason: &str) {
        self.shared.state().fail_teleport = Some(reason.into());
    }

    /// The region kicks the bot (KickUser), as the grid does when the account logs in elsewhere.
    pub fn kick(&self, reason: &str) {
        self.command(RegionCommand::Kick {
            reason: reason.into(),
        });
    }

    /// The region announces a restart in `seconds`, as Second Life does (AlertMessage with a
    /// RegionRestartSeconds or RegionRestartMinutes notification).
    pub fn announce_restart(&self, seconds: i32) {
        self.command(RegionCommand::RestartNotice { seconds });
    }

    /// A script of an object owned by `owner` (a name) asks the bot for permissions.
    pub fn script_question(&self, task: Uuid, owner: &str, questions: i32) {
        self.command(RegionCommand::ScriptQuestion {
            task,
            owner: owner.to_owned(),
            questions,
        });
    }

    /// A resident of the settings (by its index) walks to this place of the bot's region: an
    /// object update, and the coarse updates after it. They stand in a row to the west at first,
    /// from 98, 128, 25.
    pub fn move_resident(&self, index: usize, to: glam::Vec3) {
        self.command(RegionCommand::MoveResident {
            index,
            to: Some(to),
        });
    }

    /// A resident leaves the region: its object is killed, and the coarse updates leave it out.
    pub fn remove_resident(&self, index: usize) {
        self.command(RegionCommand::MoveResident { index, to: None });
    }

    /// The region closes the circuit (DisableSimulator), as when it shuts down.
    pub fn disable_simulator(&self) {
        self.command(RegionCommand::DisableSimulator);
    }

    /// Queue an event on the latest session's event queue.
    pub fn queue_event(&self, message: &str, body: Llsd) {
        let n = latest(&mut self.shared.state()).map(|s| s.n);
        if let Some(n) = n {
            self.shared.queue_event(n, message, body);
        }
    }

    /// The latest session's event queue answers its next poll with 404, as a closed queue does.
    pub fn close_event_queue(&self) {
        let mut st = self.shared.state();
        if let Some(s) = latest(&mut st) {
            s.close_queue = true;
        }
        drop(st);
        self.shared.eq_wake.notify_waiters();
    }

    /// The latest session's event queue fails every poll at once (503) from now on, so that the
    /// bot backs off, then gives the queue up.
    pub fn fail_event_queue(&self) {
        let mut st = self.shared.state();
        if let Some(s) = latest(&mut st) {
            s.fail_queue = true;
        }
        drop(st);
        self.shared.eq_wake.notify_waiters();
    }

    /// The region goes silent on the latest session's circuit: no acks, no pings, no answers,
    /// nothing it was asked to send. Its event queue goes on as before.
    pub fn silence_region(&self) {
        if let Some(s) = latest(&mut self.shared.state()) {
            s.silent = true;
        }
    }

    /// A command for the region the latest session's agent is in.
    fn command(&self, c: RegionCommand) {
        let region = latest(&mut self.shared.state()).map_or(FIRST_REGION, |s| s.region);
        // the region tasks only end when the grid is dropped
        let _ = self.regions[region].send(c);
    }
}

fn latest(st: &mut State) -> Option<&mut Session> {
    st.sessions.values_mut().max_by_key(|s| s.n)
}
