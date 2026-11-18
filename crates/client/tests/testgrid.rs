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
    connect_with(cfg, false).await
}

/// Log in with the circuits to neighbour regions kept, or not.
async fn connect_with(cfg: GridConfig, neighbours: bool) -> Running {
    let grid = TestGrid::start(cfg).await.expect("the test grid starts");
    let http = reqwest::Client::builder()
        .redirect(login::redirect_policy())
        .build()
        .expect("http client");
    let (client, mut network) = Client::new(http.clone());
    network.set_neighbours(neighbours);
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
    // invite, eject, assign and remove members, ban
    assert_eq!(test_group.powers, 0x306 | (1 << 51));
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

mod groups {
    use super::*;
    use ferro_client::groups::LastSeen;
    use ferro_testgrid::{BUILDERS, OFFICERS, OWNERS, TEST_GROUP, THIRD_GROUP};

    const SOME: Uuid = Uuid::from_u128(0x5eed_0001);
    const OTHER: Uuid = Uuid::from_u128(0x5eed_0002);

    /// Connected, with the group list in.
    async fn ready(cfg: GridConfig) -> Running {
        let r = connect_to(cfg).await;
        let deadline = tokio::time::Instant::now() + WAIT;
        while r.client.state().groups.is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "no group list");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // and the capabilities
        assert!(
            r.grid
                .wait_for(WAIT, |s| matches!(s, Seen::SeedRequest { .. }))
                .await
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        r
    }

    #[tokio::test]
    async fn roles_titles_and_role_members_over_several_packets() {
        let r = ready(GridConfig::default()).await;
        let roles = r.client.group_roles(TEST_GROUP).await.expect("roles");
        assert_eq!(roles.len(), 4, "{roles:?}");
        let everyone = roles.iter().find(|x| x.id.is_nil()).expect("everyone");
        assert_eq!((everyone.name.as_str(), everyone.members), ("Everyone", 3));
        assert!(
            roles
                .iter()
                .any(|x| x.id == BUILDERS && x.name == "Builders")
        );

        let mut pairs = r
            .client
            .group_role_members(TEST_GROUP)
            .await
            .expect("pairs");
        pairs.sort();
        let me = r.client.state().agent_id;
        let mut want = vec![(OFFICERS, me), (OWNERS, SOME), (BUILDERS, SOME)];
        want.sort();
        assert_eq!(pairs, want);

        let titles = r.client.group_titles(TEST_GROUP).await.expect("titles");
        assert_eq!(titles.len(), 2);
        assert!(
            titles
                .iter()
                .any(|t| t.role == OFFICERS && t.title == "Bot" && t.selected)
        );
        assert!(matches!(r.logout().await, End::LoggedOut));
    }

    async fn check_members(r: &Running) {
        let members = r.client.group_members(TEST_GROUP).await.expect("members");
        assert_eq!(members.len(), 3, "{members:?}");
        let some = members
            .iter()
            .find(|m| m.id == SOME)
            .expect("Some Resident");
        assert!(some.is_owner);
        assert_eq!(some.last_seen, LastSeen::Online);
        let other = members
            .iter()
            .find(|m| m.id == OTHER)
            .expect("Other Avatar");
        assert_eq!(other.last_seen, LastSeen::Date(2026, 9, 12));
        assert_eq!(other.title, "Member");
        assert_eq!(r.client.is_group_member(TEST_GROUP, OTHER).await, Ok(true));
        assert_eq!(
            r.client
                .is_group_member(TEST_GROUP, Uuid::from_u128(0xdead))
                .await,
            Ok(false)
        );
        // one request: the cache answers the rest
        assert_eq!(
            r.grid
                .count(|s| matches!(s, Seen::GroupMembersRequest { .. })),
            1
        );
    }

    #[tokio::test]
    async fn members_from_the_capability_cached() {
        let r = ready(GridConfig::default()).await;
        check_members(&r).await;
        assert!(
            r.grid.count(|s| *s
                == Seen::GroupMembersRequest {
                    group: TEST_GROUP,
                    cap: true
                })
                == 1
        );
        assert!(matches!(r.logout().await, End::LoggedOut));
    }

    #[tokio::test]
    async fn members_by_udp_without_the_capability() {
        let r = ready(GridConfig {
            member_data_cap: false,
            ..GridConfig::default()
        })
        .await;
        check_members(&r).await;
        assert!(
            r.grid.count(|s| *s
                == Seen::GroupMembersRequest {
                    group: TEST_GROUP,
                    cap: false
                })
                == 1
        );
        assert!(matches!(r.logout().await, End::LoggedOut));
    }

    #[tokio::test]
    async fn a_group_the_bot_is_not_in_is_unknown() {
        let r = ready(GridConfig::default()).await;
        assert_eq!(
            r.client.group_roles(THIRD_GROUP).await,
            Err(ClientError::UnknownGroup)
        );
        assert_eq!(
            r.client.is_group_member(THIRD_GROUP, SOME).await,
            Err(ClientError::UnknownGroup)
        );
        assert!(matches!(r.logout().await, End::LoggedOut));
    }

    #[tokio::test]
    async fn invite_eject_and_role_changes() {
        let r = ready(GridConfig::default()).await;
        let newcomer = Uuid::from_u128(0x5eed_0009);
        r.client
            .group_invite(TEST_GROUP, newcomer, &[BUILDERS])
            .await
            .expect("invite");
        assert!(
            r.grid
                .wait_for(WAIT, |s| *s
                    == Seen::GroupInvite {
                        group: TEST_GROUP,
                        invitee: newcomer,
                        role: BUILDERS
                    })
                .await
        );

        // a role given, then taken
        r.client
            .group_add_role(TEST_GROUP, OTHER, BUILDERS)
            .await
            .expect("add");
        let has_builders = |r: &Running| {
            r.grid
                .group_members(TEST_GROUP)
                .iter()
                .any(|m| m.id == OTHER && m.roles.contains(&BUILDERS))
        };
        assert!(r.grid.wait_until(WAIT, |_| has_builders(&r)).await);
        r.client
            .group_remove_role(TEST_GROUP, OTHER, BUILDERS)
            .await
            .expect("remove");
        assert!(r.grid.wait_until(WAIT, |_| !has_builders(&r)).await);

        // ejected: gone from the grid and from the cache
        assert_eq!(r.client.is_group_member(TEST_GROUP, OTHER).await, Ok(true));
        r.client
            .group_eject(TEST_GROUP, OTHER)
            .await
            .expect("eject");
        assert!(
            !r.grid
                .group_members(TEST_GROUP)
                .iter()
                .any(|m| m.id == OTHER)
        );
        assert_eq!(r.client.is_group_member(TEST_GROUP, OTHER).await, Ok(false));
        // an owner cannot be ejected
        assert!(matches!(
            r.client.group_eject(TEST_GROUP, SOME).await,
            Err(ClientError::Refused(_))
        ));
        assert!(matches!(r.logout().await, End::LoggedOut));
    }

    #[tokio::test]
    async fn a_group_invitation_accepted() {
        let r = ready(GridConfig::default()).await;
        let mut events = r.client.subscribe();
        let tx = r.grid.invite_bot(&some_resident(), THIRD_GROUP, 0);
        let im = tokio::time::timeout(WAIT, async {
            loop {
                if let Ok(Event::InstantMessage(im)) = events.recv().await {
                    return im;
                }
            }
        })
        .await
        .expect("the invitation");
        assert_eq!(im.dialog, ferro_client::events::dialog::GROUP_INVITATION);
        assert_eq!((im.from_agent_id, im.id), (THIRD_GROUP, tx));
        r.client
            .answer_group_invite(im.from_agent_id, im.id, true)
            .await
            .expect("answer");
        assert!(
            r.grid
                .wait_for(WAIT, |s| *s
                    == Seen::GroupInviteAnswer {
                        group: THIRD_GROUP,
                        accept: true
                    })
                .await
        );
        let deadline = tokio::time::Instant::now() + WAIT;
        while !r.client.state().groups.iter().any(|g| g.id == THIRD_GROUP) {
            assert!(tokio::time::Instant::now() < deadline, "not in the group");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(matches!(r.logout().await, End::LoggedOut));
    }

    #[tokio::test]
    async fn group_chat_both_ways() {
        let r = ready(GridConfig::default()).await;
        let mut events = r.client.subscribe();
        // the bot speaks: the session starts, then the line goes
        r.client
            .group_chat_send(TEST_GROUP, "hello group")
            .await
            .expect("send");
        assert!(
            r.grid
                .wait_for(WAIT, |s| *s
                    == Seen::GroupChatSend {
                        group: TEST_GROUP,
                        message: "hello group".into()
                    })
                .await
        );
        assert_eq!(
            r.grid
                .count(|s| *s == Seen::GroupChatStart { group: TEST_GROUP }),
            1
        );
        assert!(r.client.state().group_chats.contains(&TEST_GROUP));
        assert_eq!(
            r.client.group_chat_send(THIRD_GROUP, "no").await,
            Err(ClientError::UnknownGroup)
        );

        // someone else speaks first in another group: an invitation the bot accepts
        let second = ferro_testgrid::SECOND_GROUP;
        let next_line = |events: &mut tokio::sync::broadcast::Receiver<Event>| {
            let mut events = events.resubscribe();
            async move {
                tokio::time::timeout(WAIT, async {
                    loop {
                        if let Ok(Event::GroupChat {
                            group,
                            from_name,
                            message,
                            ..
                        }) = events.recv().await
                        {
                            return (group, from_name, message);
                        }
                    }
                })
                .await
                .expect("a group chat line")
            }
        };
        let first = next_line(&mut events);
        r.grid.group_chat(&some_resident(), second, "first line");
        assert_eq!(
            first.await,
            (second, "Some Resident".into(), "first line".into())
        );
        assert!(
            r.grid
                .wait_for(WAIT, |s| *s == Seen::GroupChatAccept { group: second })
                .await
        );
        // the next lines come as session messages
        let next = next_line(&mut events);
        r.grid.group_chat(&some_resident(), second, "second line");
        assert_eq!(next.await.2, "second line");

        r.client.group_chat_leave(TEST_GROUP).await.expect("leave");
        assert!(
            r.grid
                .wait_for(WAIT, |s| *s == Seen::GroupChatLeave { group: TEST_GROUP })
                .await
        );
        assert!(!r.client.state().group_chats.contains(&TEST_GROUP));
        assert!(matches!(r.logout().await, End::LoggedOut));
    }
}

/// Waits for the client's state to satisfy `done`.
async fn state_until(client: &Client, done: impl Fn(&ferro_client::State) -> bool) -> bool {
    tokio::time::timeout(WAIT, async {
        while !done(&client.state()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

async fn arrival_in(events: &mut tokio::sync::broadcast::Receiver<Event>) -> (String, Vec3) {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::Arrived { region, position }) = events.recv().await {
                return (region, position);
            }
        }
    })
    .await
    .expect("an arrival")
}

fn circuits_to(grid: &TestGrid, region: usize) -> usize {
    grid.count(|s| matches!(s, Seen::UseCircuitCode { region: r, .. } if *r == region))
}

#[tokio::test]
async fn without_neighbours_enable_simulator_opens_nothing() {
    let r = connect().await;
    r.grid.enable_neighbour();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(circuits_to(&r.grid, ferro_testgrid::SECOND_REGION), 0);
    assert!(r.client.state().neighbours.is_empty());
    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn without_neighbours_a_crossing_opens_a_circuit_to_the_new_region() {
    let r = connect().await;
    let mut events = r.client.subscribe();
    r.grid.cross(Vec3::new(3.0, 128.0, 25.0));
    assert_eq!(
        arrival_in(&mut events).await,
        ("Second Region".into(), Vec3::new(3.0, 128.0, 25.0))
    );
    assert_eq!(circuits_to(&r.grid, ferro_testgrid::SECOND_REGION), 1);
    assert!(r.client.state().neighbours.is_empty());
    assert!(matches!(r.logout().await, End::LoggedOut));
}

/// As a viewer: EnableSimulator opens a circuit to the neighbour (UseCircuitCode, the handshake
/// answered, no CompleteAgentMovement), a crossing moves in over it with CompleteAgentMovement
/// alone, and the region left is kept as a neighbour.
#[tokio::test]
async fn a_crossing_moves_in_over_the_neighbours_circuit() {
    let r = connect_with(GridConfig::default(), true).await;
    let second = r.grid.region_handle(ferro_testgrid::SECOND_REGION);
    let first = r.grid.region_handle(ferro_testgrid::FIRST_REGION);
    r.grid.enable_neighbour();
    assert!(
        state_until(&r.client, |st| st.neighbours
            == vec![(second, "Second Region".into())])
        .await,
        "the neighbour's circuit and handshake: {:?}",
        r.client.state().neighbours
    );
    assert_eq!(circuits_to(&r.grid, ferro_testgrid::SECOND_REGION), 1);
    assert_eq!(
        r.grid.count(|s| matches!(s, Seen::CompleteAgentMovement { region, .. } if *region == ferro_testgrid::SECOND_REGION)),
        0,
        "a child agent is not moved into"
    );

    let mut events = r.client.subscribe();
    r.grid.cross(Vec3::new(3.0, 128.0, 25.0));
    assert_eq!(
        arrival_in(&mut events).await,
        ("Second Region".into(), Vec3::new(3.0, 128.0, 25.0))
    );
    // no second UseCircuitCode: the circuit was open
    assert_eq!(circuits_to(&r.grid, ferro_testgrid::SECOND_REGION), 1);
    let st = r.client.state();
    let region = st.region.expect("a region");
    assert_eq!(
        (region.name.as_str(), region.handle),
        ("Second Region", second)
    );
    assert_eq!(st.neighbours, vec![(first, "Test Region".into())]);

    // and it keeps working there
    r.grid.say("Someone", "hello across the border");
    assert!(
        tokio::time::timeout(WAIT, async {
            loop {
                if let Ok(Event::Chat(c)) = events.recv().await
                    && c.message == "hello across the border"
                {
                    return;
                }
            }
        })
        .await
        .is_ok()
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}

#[tokio::test]
async fn a_neighbour_that_closes_its_circuit_is_dropped_and_the_session_goes_on() {
    let r = connect_with(GridConfig::default(), true).await;
    r.grid.enable_neighbour();
    assert!(state_until(&r.client, |st| st.neighbours.len() == 1).await);
    r.grid.close_neighbour();
    assert!(state_until(&r.client, |st| st.neighbours.is_empty()).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let st = r.client.state();
    assert!(st.online);
    assert_eq!(st.region.expect("a region").name, "Test Region");
    assert!(matches!(r.logout().await, End::LoggedOut));
}

/// A region that never answers, as a region whose address the bot cannot reach: UseCircuitCode
/// goes out to it and is never acked.
async fn nowhere() -> tokio::net::UdpSocket {
    tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("a socket")
}

async fn teleport_failed_in(events: &mut tokio::sync::broadcast::Receiver<Event>) -> String {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::TeleportFailed { reason }) = events.recv().await {
                return reason;
            }
        }
    })
    .await
    .expect("TeleportFailed")
}

/// After TeleportFinish or CrossedRegion to a region the bot cannot reach, the region left takes
/// the agent back (OpenSim: TeleportFailed over its circuit) and the bot goes back to it: the
/// session goes on where it was, with or without neighbours.
#[tokio::test]
async fn a_move_to_a_region_that_never_answers_goes_back_when_the_region_left_says_so() {
    for neighbours in [false, true] {
        let r = connect_with(GridConfig::default(), neighbours).await;
        let first = r.grid.region_handle(ferro_testgrid::FIRST_REGION);
        let dead = nowhere().await;
        let mut events = r.client.subscribe();
        r.grid.cross_to(dead.local_addr().expect("addr"));
        // the bot tries the new region
        let mut buf = [0u8; 1500];
        tokio::time::timeout(WAIT, dead.recv_from(&mut buf))
            .await
            .expect("UseCircuitCode to the new region")
            .expect("recv");
        assert!(state_until(&r.client, |st| st.region.is_none()).await);
        r.grid
            .take_back("Destination region did not signal teleport completion.");
        let reason = teleport_failed_in(&mut events).await;
        assert!(reason.contains("did not signal"), "{reason}");
        let st = r.client.state();
        assert!(st.online);
        let region = st.region.expect("back in a region");
        assert_eq!(
            (region.name.as_str(), region.handle),
            ("Test Region", first)
        );
        assert_eq!(st.position, Vec3::new(128.0, 128.0, 25.0));

        // and it works there: chat comes in, a teleport goes through, the logout is answered
        r.grid.say("Someone", "welcome back");
        assert!(
            tokio::time::timeout(WAIT, async {
                loop {
                    if let Ok(Event::Chat(c)) = events.recv().await
                        && c.message == "welcome back"
                    {
                        return;
                    }
                }
            })
            .await
            .is_ok(),
            "neighbours {neighbours}"
        );
        r.client
            .teleport_to(first, Vec3::new(100.0, 100.0, 25.0))
            .await
            .expect("a local teleport after going back");
        assert!(matches!(r.logout().await, End::LoggedOut));
    }
}

/// A region left that never says: the bot goes back once the wait for the arrival expires.
#[tokio::test]
async fn a_move_never_confirmed_goes_back_when_the_wait_expires() {
    let grid = TestGrid::start(GridConfig::default())
        .await
        .expect("the test grid starts");
    let http = reqwest::Client::new();
    let (client, mut network) = Client::new(http.clone());
    network.set_timeouts(ferro_client::Timeouts {
        transit: Duration::from_secs(2),
        ..Default::default()
    });
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
    arrival_in(&mut events).await;
    let r = Running {
        grid,
        client,
        stop,
        session,
    };
    let dead = nowhere().await;
    r.grid.cross_to(dead.local_addr().expect("addr"));
    let reason = teleport_failed_in(&mut events).await;
    assert!(reason.contains("did not confirm"), "{reason}");
    assert_eq!(
        r.client.state().region.map(|r| r.name),
        Some("Test Region".into())
    );
    // the region's late TeleportFailed is taken quietly: no second event
    r.grid
        .take_back("Destination region did not signal teleport completion.");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|e| matches!(e, Event::TeleportFailed { .. })),
        "one TeleportFailed only"
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}

fn polls_of(grid: &TestGrid, region: usize) -> usize {
    grid.count(|s| matches!(s, Seen::EventQueuePoll { region: r, .. } if *r == region))
}

/// As a viewer: a neighbour's event queue is polled once the region gives its seed
/// (EstablishAgentCommunication), and the region left by a crossing goes on being polled.
#[tokio::test]
async fn neighbours_event_queues_are_polled() {
    let r = connect_with(GridConfig::default(), true).await;
    let (first, second) = (ferro_testgrid::FIRST_REGION, ferro_testgrid::SECOND_REGION);
    r.grid.enable_neighbour();
    assert!(
        r.grid
            .wait_for(
                WAIT,
                |s| matches!(s, Seen::EventQueuePoll { region, .. } if *region == second)
            )
            .await,
        "the neighbour's queue is polled"
    );
    let mut events = r.client.subscribe();
    r.grid.cross(Vec3::new(3.0, 128.0, 25.0));
    arrival_in(&mut events).await;
    let before = polls_of(&r.grid, first);
    assert!(
        r.grid
            .wait_until(WAIT, |_| polls_of(&r.grid, first) > before)
            .await,
        "the region left is polled as a neighbour"
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}

/// Without neighbours, EstablishAgentCommunication opens nothing.
#[tokio::test]
async fn without_neighbours_no_neighbour_queue_is_polled() {
    let r = connect().await;
    r.grid.enable_neighbour();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(polls_of(&r.grid, ferro_testgrid::SECOND_REGION), 0);
    assert!(matches!(r.logout().await, End::LoggedOut));
}

/// A box as the build tool makes one, moved with its linkset, deleted to the trash: the
/// messages the region gets (Aurora's build_cmds.rs).
#[tokio::test]
async fn a_box_is_made_moved_and_deleted() {
    let r = connect().await;
    let did = |what: String| move |s: &Seen| matches!(s, Seen::Object { what: w } if *w == what);
    r.client
        .add_box(Vec3::new(10.0, 20.0, 30.0), Vec3::new(1.0, 1.0, 0.5))
        .await
        .expect("add");
    assert!(
        r.grid
            .wait_for(
                WAIT,
                did("add pcode 9 path 0x10 profile 0x1 scale 100,100 size 1.0,1.0,0.5 at 10,20,30 bypass 1".into())
            )
            .await,
        "{:?}",
        r.grid.seen()
    );
    r.client
        .move_object(77, Vec3::new(260.0, 20.0, 30.0))
        .await
        .expect("move");
    assert!(
        r.grid
            .wait_for(WAIT, did("update 77 type 0x9 to 260,20,30".into()))
            .await
    );
    r.client.delete_object(77).await.expect("delete");
    assert!(
        r.grid
            .wait_for(
                WAIT,
                |s| matches!(s, Seen::Object { what } if what.starts_with("derez 77 to 6 "))
            )
            .await
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}

/// The land tools as the viewer sends them: Subdivide and Join of a selection, a brush at a
/// point (the old brush index and the real size), a flood over a selection.
#[tokio::test]
async fn land_is_divided_joined_and_terraformed_as_the_viewer_does() {
    use ferro_client::ParcelAction;
    use ferro_client::land::action;
    let r = connect().await;
    let parcel = |what: &str| {
        let what = what.to_owned();
        move |s: &Seen| matches!(s, Seen::Parcel { what: w } if *w == what)
    };
    let rect = |west, south, east, north| (west, south, east, north);
    let (w, s, e, n) = rect(16.0, 16.0, 32.0, 32.0);
    r.client
        .parcel_act(ParcelAction::Divide {
            west: w,
            south: s,
            east: e,
            north: n,
        })
        .await
        .expect("divide");
    assert!(r.grid.wait_for(WAIT, parcel("divide 16 16 32 32")).await);
    r.client
        .parcel_act(ParcelAction::Join {
            west: 12.0,
            south: 16.0,
            east: 36.0,
            north: 32.0,
        })
        .await
        .expect("join");
    assert!(r.grid.wait_for(WAIT, parcel("join 12 16 36 32")).await);
    r.client
        .parcel_act(ParcelAction::ModifyLand {
            action: action::RAISE,
            brush_size: 4.0,
            seconds: 0.25,
            height: 21.5,
            local_id: -1,
            west: 40.0,
            south: 50.0,
            east: 40.0,
            north: 50.0,
        })
        .await
        .expect("raise");
    assert!(
        r.grid
            .wait_for(
                WAIT,
                parcel("modify-land 1 brush 1 Some(4.0) seconds 0.25 height 21.5 parcel -1 at 40 50 40 50")
            )
            .await
    );
    r.client
        .parcel_act(ParcelAction::ModifyLand {
            action: action::REVERT,
            brush_size: 8.0,
            seconds: 1.0,
            height: 0.0,
            local_id: 3,
            west: 0.0,
            south: 0.0,
            east: 16.0,
            north: 16.0,
        })
        .await
        .expect("revert");
    assert!(
        r.grid
            .wait_for(
                WAIT,
                parcel("modify-land 5 brush 2 Some(8.0) seconds 1 height 0 parcel 3 at 0 0 16 16")
            )
            .await
    );
    assert!(matches!(r.logout().await, End::LoggedOut));
}

/// The ground as the region's LayerData patches have it, and the parcel overlay once all its
/// parts have come; both forgotten in another region.
#[tokio::test]
async fn the_ground_and_the_parcel_overlay_are_known_from_the_region() {
    use ferro_client::land::{self, CellOwner};
    let r = connect().await;
    assert_eq!(r.client.ground_height(40.0, 50.0), None);
    r.grid
        .send_ground(land::encode_flat_layer(&[(2, 3, 21.0), (15, 0, 7.5)]));
    assert!(
        state_until(&r.client, |st| st.terrain.len() == 2).await,
        "the patches came"
    );
    let h = r.client.ground_height(40.0, 50.0).expect("a height");
    assert!((h - 21.0).abs() < 1e-3, "{h}");
    let h = r.client.ground_height(250.0, 1.0).expect("a height");
    assert!((h - 7.5).abs() < 1e-3, "{h}");
    assert_eq!(r.client.ground_height(100.0, 100.0), None);

    // the bot's own cells, a line west of the cell (4, 4): a parcel of someone else's
    let mut first = vec![3u8; land::CELLS_PER_PART];
    first[4 * 64 + 4] = 1 | 0x40;
    for (i, part) in [first, vec![3; 1024], vec![3; 1024], vec![3; 1024]]
        .into_iter()
        .enumerate()
    {
        r.grid.send_overlay(i as i32, part);
    }
    assert!(
        state_until(&r.client, |st| st.overlay.width() == Some(64)).await,
        "the whole overlay came"
    );
    let o = r.client.state().overlay;
    assert_eq!(o.owner(0, 0), Some(CellOwner::Mine));
    assert_eq!(o.owner(4, 4), Some(CellOwner::Other));
    assert_eq!(o.west_line(4, 4), Some(true));
    assert_eq!(o.updates, 4);
    assert!(matches!(r.logout().await, End::LoggedOut));
}
