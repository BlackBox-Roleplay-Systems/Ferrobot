//! The faults a test platform puts into the client (the `faults` feature), against the test grid.
#![cfg(feature = "faults")]

use ferro_client::login::{self, LoginRequest, StartLocation};
use ferro_client::{Client, End, Event};
use ferro_testgrid::{GridConfig, Seen, TestGrid};
use glam::Vec3;
use std::time::Duration;
use tokio::sync::{broadcast, watch};

const WAIT: Duration = Duration::from_secs(10);

async fn connect(
    neighbours: bool,
) -> (
    TestGrid,
    Client,
    watch::Sender<bool>,
    tokio::task::JoinHandle<End>,
) {
    let grid = TestGrid::start(GridConfig::default())
        .await
        .expect("the test grid starts");
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
    arrival(&mut events, WAIT).await.expect("arrival");
    (grid, client, stop, session)
}

async fn arrival(events: &mut broadcast::Receiver<Event>, wait: Duration) -> Option<String> {
    tokio::time::timeout(wait, async {
        loop {
            if let Ok(Event::Arrived { region, .. }) = events.recv().await {
                return region;
            }
        }
    })
    .await
    .ok()
}

/// A viewer that missed CrossedRegion stays where it was: the region it left made it a child
/// agent, and nothing else tells the viewer. When the region sends the crossing again, as
/// Tranquillity now does for a viewer that keeps talking to it, the crossing completes.
#[tokio::test]
async fn a_lost_crossed_region_leaves_the_bot_behind_and_a_resent_one_moves_it() {
    for neighbours in [false, true] {
        let (grid, client, stop, session) = connect(neighbours).await;
        if neighbours {
            grid.enable_neighbour();
            tokio::time::timeout(WAIT, async {
                while client.state().neighbours.is_empty() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the neighbour's circuit");
        }
        let mut events = client.subscribe();
        client.faults().drop_events("CrossedRegion", 1);
        grid.cross(Vec3::new(3.0, 128.0, 25.0));
        assert_eq!(
            arrival(&mut events, Duration::from_millis(800)).await,
            None,
            "the crossing was lost"
        );
        assert!(client.faults().pending().is_empty());
        assert_eq!(client.state().region.expect("a region").name, "Test Region");
        assert_eq!(
            grid.count(|s| matches!(s, Seen::CompleteAgentMovement { region, .. } if *region == ferro_testgrid::SECOND_REGION)),
            0
        );

        grid.cross(Vec3::new(3.0, 128.0, 25.0));
        assert_eq!(
            arrival(&mut events, WAIT).await.as_deref(),
            Some("Second Region")
        );
        stop.send(true).expect("stop");
        let end = tokio::time::timeout(WAIT, session)
            .await
            .expect("the session ends")
            .expect("the session task");
        assert!(matches!(end, End::LoggedOut), "{end:?}");
    }
}

/// A dropped reliable UDP message is not acknowledged: the region would resend it. The test
/// grid's region does not resend, so the arrival never comes, which shows the drop.
#[tokio::test]
async fn a_dropped_message_never_reaches_the_client() {
    let (grid, client, stop, session) = connect(false).await;
    let mut events = client.subscribe();
    client.faults().drop_messages("AgentMovementComplete", 1);
    let lure = grid.offer_teleport(&ferro_testgrid::Resident {
        first: "Some".into(),
        last: "One".into(),
        id: uuid::Uuid::from_u128(0x5eed_0001),
    });
    let accepted = {
        let client = client.clone();
        tokio::spawn(async move { client.accept_lure(lure).await })
    };
    assert_eq!(arrival(&mut events, Duration::from_millis(800)).await, None);
    assert!(client.faults().pending().is_empty());
    accepted.abort();
    stop.send(true).expect("stop");
    let _ = tokio::time::timeout(WAIT, session).await;
}
