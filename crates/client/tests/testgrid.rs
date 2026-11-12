//! The client library against the test grid, in process: the state it keeps (region, position,
//! home, active group, groups, health), names both ways, events, and requests between sessions.

use ferro_client::login::{self, LoginRequest, StartLocation};
use ferro_client::{Client, ClientError, End, Event};
use ferro_testgrid::{GridConfig, Seen, TestGrid};
use glam::Vec3;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(10);

struct Running {
    grid: TestGrid,
    client: Client,
    stop: watch::Sender<bool>,
    session: JoinHandle<End>,
}

/// Log in to a fresh test grid and run the session until arrival.
async fn connect() -> Running {
    connect_to(GridConfig::default()).await
}

async fn connect_to(cfg: GridConfig) -> Running {
    let grid = TestGrid::start(cfg).await.expect("the test grid starts");
    let http = reqwest::Client::builder()
        .redirect(login::redirect_policy())
        .build()
        .expect("http client");
    let (client, mut network) = Client::new(http.clone());
    let mut events = client.subscribe();
    let req = LoginRequest {
        login_uri: grid.login_uri(),
        username: grid.username(),
        password: grid.password().into(),
        start: StartLocation::Last,
    };
    let resp = login::login(&http, &req).await.expect("login");
    let (stop, stop_rx) = watch::channel(false);
    let session = tokio::spawn(async move { network.session(&resp, stop_rx).await });
    let arrived = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::Arrived { region, .. }) = events.recv().await {
                return region;
            }
        }
    })
    .await
    .expect("arrival");
    assert_eq!(arrived, "Test Region");
    Running {
        grid,
        client,
        stop,
        session,
    }
}

impl Running {
    async fn logout(self) -> End {
        self.stop.send(true).expect("stop");
        tokio::time::timeout(WAIT, self.session)
            .await
            .expect("the session ends")
            .expect("the session task")
    }
}

#[tokio::test]
async fn the_state_after_arrival() {
    let r = connect().await;
    let st = r.client.state();
    assert!(st.online);
    assert_eq!(st.name, "Test Bot");
    assert_eq!(
        st.region.as_ref().map(|r| r.name.as_str()),
        Some("Test Region")
    );
    assert_eq!(st.position, Vec3::new(128.0, 128.0, 25.0));
    assert_eq!(st.logins, 1);
    let home = st.home.expect("a home");
    assert_eq!(home.region_handle, (256256u64 << 32) | 256000);
    assert_eq!(home.position, Vec3::new(100.0, 110.0, 22.0));

    let health = r.client.health();
    assert!(health.online);
    assert_eq!(health.region.as_deref(), Some("Test Region"));
    assert!(health.since_last_packet.expect("a packet") < Duration::from_secs(5));

    // the active group (AgentDataUpdate) and the groups (AgentGroupDataUpdate, event queue)
    assert!(
        r.grid
            .wait_for(WAIT, |s| matches!(s, Seen::AgentDataUpdateRequest { .. }))
            .await
    );
    let deadline = tokio::time::Instant::now() + WAIT;
    while r.client.state().groups.len() < 2 || r.client.state().active_group.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?}",
            r.client.state()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let st = r.client.state();
    let active = st.active_group.expect("an active group");
    assert_eq!(
        (active.name.as_str(), active.title.as_str()),
        ("Test Group", "Bot")
    );
    let test_group = st
        .groups
        .iter()
        .find(|g| g.name == "Test Group")
        .expect("Test Group");
    assert_eq!(test_group.id, Uuid::from_u128(0x6000_0001));
    assert_eq!(test_group.powers, 0x0000_0002_0000_0002);
    assert!(test_group.accept_notices);

    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn names_both_ways_and_cached() {
    let r = connect().await;
    let some = Uuid::from_u128(0x5eed_0001);
    let other = Uuid::from_u128(0x5eed_0002);
    let unknown = Uuid::from_u128(0xdead);

    // name to key: searched on the grid, case does not matter; the search fills the cache for
    // the way back, so the name of that key needs no request
    assert_eq!(r.client.find_avatar("other avatar").await, Ok(Some(other)));
    assert_eq!(r.client.name(other).await, Ok(Some("Other Avatar".into())));
    assert_eq!(
        r.grid.count(|s| matches!(s, Seen::UuidNameRequest { .. })),
        0
    );

    // key to name: asked once, then from the cache
    let names = r.client.names(&[some]).await.expect("names");
    assert_eq!(names[&some], "Some Resident");
    assert_eq!(r.client.name(some).await, Ok(Some("Some Resident".into())));
    assert_eq!(
        r.grid.count(|s| matches!(s, Seen::UuidNameRequest { .. })),
        1
    );
    // a username stands for "<name> Resident" (SecondBot's Name2Key): found in the cache
    assert_eq!(r.client.find_avatar("some").await, Ok(Some(some)));
    assert_eq!(r.client.find_avatar("SOME RESIDENT").await, Ok(Some(some)));
    assert_eq!(
        r.grid
            .count(|s| matches!(s, Seen::AvatarPickerRequest { .. })),
        1
    );
    // the bot knows its own name without asking
    let me = r.client.state().agent_id;
    assert_eq!(r.client.name(me).await, Ok(Some("Test Bot".into())));

    assert_eq!(r.client.find_avatar("Nobody Here").await, Ok(None));
    // a key the grid does not know: no answer, the request times out
    assert_eq!(r.client.name(unknown).await, Err(ClientError::Timeout));

    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn chat_reaches_subscribers() {
    let r = connect().await;
    let mut events = r.client.subscribe();
    r.grid.say("Some Resident", "hello");
    let chat = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::Chat(c)) = events.recv().await {
                return c;
            }
        }
    })
    .await
    .expect("chat");
    assert_eq!(
        (chat.from_name.as_str(), chat.message.as_str()),
        ("Some Resident", "hello")
    );
    assert_eq!(chat.chat_type, ferro_client::events::chat_type::SAY);
    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn after_the_session_requests_fail_at_once_and_the_state_says_offline() {
    let r = connect().await;
    let client = r.client.clone();
    let mut events = client.subscribe();
    r.grid.kick("Kicked for a test.");
    let end = tokio::time::timeout(WAIT, r.session)
        .await
        .expect("the session ends")
        .expect("the session task");
    assert!(matches!(end, End::Kicked(ref why) if why == "Kicked for a test."));
    let reason = loop {
        if let Ok(Event::Disconnected { reason }) = events.recv().await {
            break reason;
        }
    };
    assert_eq!(reason, "disconnected by the grid: Kicked for a test.");
    assert!(!client.health().online);
    assert_eq!(client.state().region, None);
    assert_eq!(
        client.name(Uuid::from_u128(0x5eed_0002)).await,
        Err(ClientError::NotConnected)
    );
}

fn some_resident() -> ferro_testgrid::Resident {
    ferro_testgrid::Resident {
        id: Uuid::from_u128(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    }
}

#[tokio::test]
async fn a_teleport_offer_accepted_moves_the_bot_and_it_keeps_working() {
    let r = connect().await;
    let mut events = r.client.subscribe();
    let lure = r.grid.offer_teleport(&some_resident());
    // the offer reaches the client as an IM_LURE_USER with the lure's id
    let offer = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::InstantMessage(im)) = events.recv().await {
                return im;
            }
        }
    })
    .await
    .expect("the offer");
    assert_eq!(
        (offer.dialog, offer.id),
        (ferro_client::events::dialog::LURE_USER, lure)
    );

    assert_eq!(
        r.client.accept_lure(offer.id).await,
        Ok("Second Region".into())
    );
    let st = r.client.state();
    assert!(st.online);
    let region = st.region.expect("a region");
    assert_eq!(region.name, "Second Region");
    assert_eq!(
        region.handle,
        r.grid.region_handle(ferro_testgrid::SECOND_REGION)
    );
    assert_eq!(st.logins, 1, "a teleport is not a login");
    assert!(
        r.grid.count(|s| *s
            == Seen::UseCircuitCode {
                session: 1,
                region: 1
            })
            == 1
    );
    assert!(
        r.grid
            .wait_for(WAIT, |s| *s
                == Seen::EventQueuePoll {
                    session: 1,
                    region: 1
                })
            .await
    );

    // the new region answers requests and its chat reaches the client
    assert_eq!(
        r.client.name(Uuid::from_u128(0x5eed_0002)).await,
        Ok(Some("Other Avatar".into()))
    );
    r.grid.say("Other Avatar", "welcome");
    let chat = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::Chat(c)) = events.recv().await {
                return c;
            }
        }
    })
    .await
    .expect("chat in the new region");
    assert_eq!(chat.message, "welcome");

    // and the logout goes to the new region
    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn a_failed_teleport_leaves_the_bot_where_it_was() {
    let r = connect().await;
    r.grid.fail_next_teleport("The region is full.");
    let lure = r.grid.offer_teleport(&some_resident());
    assert_eq!(
        r.client.accept_lure(lure).await,
        Err(ClientError::Refused("The region is full.".into()))
    );
    assert_eq!(
        r.client.accept_lure(Uuid::new_v4()).await,
        Err(ClientError::Refused(
            "That teleport offer is no longer valid.".into()
        ))
    );
    let st = r.client.state();
    assert!(st.online);
    assert_eq!(st.region.expect("a region").name, "Test Region");
    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn teleports_home_and_to_a_location() {
    let r = connect().await;
    // home is in the second region
    assert_eq!(r.client.teleport_home().await, Ok("Second Region".into()));
    assert_eq!(r.client.state().position, Vec3::new(100.0, 110.0, 22.0));

    // back to the first region, by its handle
    let first = r.grid.region_handle(ferro_testgrid::FIRST_REGION);
    assert_eq!(
        r.client
            .teleport_to(first, Vec3::new(10.0, 20.0, 30.0))
            .await,
        Ok("Test Region".into())
    );
    // inside the region: TeleportLocal
    assert_eq!(
        r.client
            .teleport_to(first, Vec3::new(50.0, 60.0, 30.0))
            .await,
        Ok("Test Region".into())
    );
    assert_eq!(r.client.state().position, Vec3::new(50.0, 60.0, 30.0));
    // nowhere
    assert_eq!(
        r.client.teleport_to(42, Vec3::ZERO).await,
        Err(ClientError::Refused("Could not find that region.".into()))
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn display_names_from_the_people_api_cached() {
    let r = connect().await;
    let some = Uuid::from_u128(0x5eed_0001);
    let other = Uuid::from_u128(0x5eed_0002);
    let unknown = Uuid::from_u128(0xdead);
    let names = r
        .client
        .display_names(&[some, other, unknown])
        .await
        .expect("display names");
    assert_eq!(names.len(), 2, "{names:?}");
    let sunny = &names[&some];
    assert_eq!(
        (sunny.display_name.as_str(), sunny.username.as_str()),
        ("Sunny", "some")
    );
    assert!(!sunny.is_default);
    let o = &names[&other];
    assert_eq!(
        (o.display_name.as_str(), o.username.as_str()),
        ("Other Avatar", "other.avatar")
    );
    assert!(o.is_default);

    // cached, and the legacy names came with them
    assert_eq!(
        r.client
            .display_name(some)
            .await
            .expect("cached")
            .map(|d| d.display_name),
        Some("Sunny".into())
    );
    assert_eq!(r.client.name(other).await, Ok(Some("Other Avatar".into())));
    assert_eq!(
        r.grid
            .count(|s| matches!(s, Seen::DisplayNamesRequest { .. })),
        1
    );
    assert_eq!(
        r.grid.count(|s| matches!(s, Seen::UuidNameRequest { .. })),
        0
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn without_the_people_api_display_names_are_the_legacy_names() {
    let r = connect_to(GridConfig {
        display_names_cap: false,
        ..GridConfig::default()
    })
    .await;
    // the capabilities come with the seed, a moment after the arrival
    assert!(
        r.grid
            .wait_for(WAIT, |s| matches!(s, Seen::SeedRequest { .. }))
            .await
    );
    let some = Uuid::from_u128(0x5eed_0001);
    let d = r
        .client
        .display_name(some)
        .await
        .expect("a reply")
        .expect("known");
    assert_eq!(
        (d.display_name.as_str(), d.username.as_str()),
        ("Some Resident", "some")
    );
    assert!(d.is_default);
    assert_eq!(
        r.grid
            .count(|s| matches!(s, Seen::DisplayNamesRequest { .. })),
        0
    );
    assert_eq!(
        r.grid.count(|s| matches!(s, Seen::UuidNameRequest { .. })),
        1
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}
