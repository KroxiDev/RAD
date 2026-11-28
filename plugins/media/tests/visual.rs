mod support;
use image::{Rgba, RgbaImage};
use rad_plugin_media::Timing;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use support::*;

const BRAVE: &str = "org.mpris.MediaPlayer2.brave.instance4242";
const STRAWBERRY: &str = "org.mpris.MediaPlayer2.strawberry";
const HEARTBEAT: Duration = Duration::from_millis(500);

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

struct Art {
    dir: Scratch,
}

impl Art {
    fn new(colour: [u8; 4]) -> Self {
        let art = Self {
            dir: scratch_dir("art", false),
        };
        RgbaImage::from_pixel(200, 200, Rgba(colour))
            .save(art.path())
            .unwrap();
        art
    }

    fn path(&self) -> PathBuf {
        self.dir.join("cover.png")
    }

    fn url(&self) -> String {
        format!("file://{}", self.path().display())
    }
}

fn centre(visual: &Visual) -> [u8; 3] {
    let image = image::load_from_memory_with_format(
        &visual.jpeg().expect("a cover"),
        image::ImageFormat::Jpeg,
    )
    .unwrap()
    .to_rgb8();
    assert_eq!((image.width(), image.height()), (96, 96));
    image.get_pixel(48, 48).0
}

#[test]
fn a_cover_that_cannot_be_used_leaves_the_text() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    let notes = scratch_dir("art", false);
    let not_a_picture = notes.join("notes.txt");
    fs::write(&not_a_picture, "not a picture").unwrap();
    let good = Art::new([200, 0, 0, 255]);
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("strawberry");
    host.wait_state(subscription, "current", Some("Stopped"), secs(5))
        .unwrap();
    for url in [
        "https://example.invalid/cover.jpg".to_owned(),
        "http://example.invalid/cover.jpg".to_owned(),
        "file://example.invalid/cover.jpg".to_owned(),
        "data:image/png;base64,AAAA".to_owned(),
        "file:///nowhere/cover.png".to_owned(),
        format!("file://{}", not_a_picture.display()),
    ] {
        player
            .command(&format!("art {}", good.url()))
            .command("status Paused");
        host.wait_visual(subscription, |v| v.jpeg_hex.is_some(), HEARTBEAT)
            .unwrap();
        player.command(&format!("art {url}"));
        let (_, visual) = host
            .wait_visual(subscription, |v| v.jpeg_hex.is_none(), HEARTBEAT)
            .unwrap_or_else(|seen| panic!("{url} still shows a cover: {seen:?}"));
        assert!(visual.is("current", Some("Paused")), "{url}: {visual:?}");
    }
    host.finish().unwrap();
}

#[test]
fn changes_made_outside_reach_the_host_within_one_heartbeat() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("strawberry");
    host.wait_state(subscription, "current", Some("Stopped"), secs(5))
        .unwrap();
    for state in ["Playing", "Paused", "Stopped", "Playing"] {
        host.discard_pending();
        player.command(&format!("status {state}"));
        let took = host
            .wait_state(subscription, "current", Some(state), HEARTBEAT)
            .unwrap_or_else(|seen| panic!("never {state}: {seen:?}"));
        assert!(took <= HEARTBEAT, "{took:?}");
    }
    let (first, second) = (Art::new([220, 20, 20, 255]), Art::new([20, 220, 20, 255]));
    host.discard_pending();
    player.command(&format!("art {}", first.url()));
    let (_, red) = host
        .wait_visual(subscription, |v| v.jpeg_hex.is_some(), HEARTBEAT)
        .unwrap();
    assert!(centre(&red)[0] > 150);
    host.discard_pending();
    player.command(&format!("art {}", second.url()));
    let (_, green) = host
        .wait_visual(
            subscription,
            |v| v.jpeg_hex.is_some() && v != &red,
            HEARTBEAT,
        )
        .unwrap();
    assert!(centre(&green)[1] > 150);
    host.discard_pending();
    player.command("art");
    host.wait_state(subscription, "current", Some("Playing"), HEARTBEAT)
        .unwrap();
    host.finish().unwrap();
}

#[test]
fn a_cover_file_replaced_under_the_same_name_is_shown_again() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    let art = Art::new([220, 20, 20, 255]);
    player.command(&format!("art {}", art.url())).sync();
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("strawberry");
    let (_, red) = host
        .wait_visual(subscription, |v| v.jpeg_hex.is_some(), secs(5))
        .unwrap();
    assert!(centre(&red)[0] > 150);
    host.discard_pending();
    RgbaImage::from_pixel(300, 300, Rgba([20, 20, 220, 255]))
        .save(art.path())
        .unwrap();
    player.command("title next track");
    let (_, blue) = host
        .wait_visual(
            subscription,
            |v| v.jpeg_hex.is_some() && v != &red,
            HEARTBEAT,
        )
        .unwrap();
    assert!(centre(&blue)[2] > 150);
    host.finish().unwrap();
}

#[test]
fn a_player_that_starts_or_quits_is_followed() {
    let bus = PrivateBus::start();
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("brave");
    let (_, absent) = host.wait_visual(subscription, |_| true, secs(5)).unwrap();
    assert_eq!(absent.availability, "unavailable");
    assert_eq!((absent.text, absent.jpeg_hex), (None, None));

    host.discard_pending();
    let mut player = FakePlayer::start(&bus, BRAVE);
    player.command("status Playing").sync();
    let took = host
        .wait_state(subscription, "current", Some("Playing"), HEARTBEAT)
        .unwrap_or_else(|seen| panic!("{seen:?}"));
    assert!(took <= HEARTBEAT);

    host.discard_pending();
    player.quit();
    host.wait_state(subscription, "unavailable", None, HEARTBEAT)
        .unwrap();

    host.discard_pending();
    let mut again = FakePlayer::start(&bus, BRAVE);
    host.wait_state(subscription, "current", Some("Stopped"), HEARTBEAT)
        .unwrap();
    again.command("status Paused");
    host.wait_state(subscription, "current", Some("Paused"), HEARTBEAT)
        .unwrap();
    host.finish().unwrap();
}

#[test]
fn only_the_chosen_application_is_shown() {
    let bus = PrivateBus::start_with_policy(
        r#"<deny send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus" send_member="GetNameOwner"/>"#,
    );
    let mut strawberry = FakePlayer::start(&bus, STRAWBERRY);
    strawberry.command("status Playing").sync();
    let mut host = Host::start(Some(bus.address()), quick());
    let brave = host.subscribe("brave");
    let berry = host.subscribe("strawberry");
    host.wait_state(berry, "current", Some("Playing"), secs(5))
        .unwrap();
    host.wait_state(brave, "unavailable", None, secs(5))
        .unwrap();

    let mut brave_player = FakePlayer::start_owning(
        &bus,
        &[
            BRAVE,
            "org.mpris.MediaPlayer2.alias",
            "org.mpris.MediaPlayer2.strawberry.instance2",
        ],
    );
    host.wait_state(brave, "current", Some("Stopped"), HEARTBEAT)
        .unwrap();
    brave_player.command("status Paused").sync();
    host.wait_state(brave, "current", Some("Paused"), HEARTBEAT)
        .unwrap();
    let brave_reads = brave_player.reads_of("PlaybackStatus");
    host.discard_pending();
    strawberry.command("status Stopped");
    host.wait_state(berry, "current", Some("Stopped"), HEARTBEAT)
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(700);
    while let Some(visual) =
        host.next_visual(brave, deadline.saturating_duration_since(Instant::now()))
    {
        assert!(visual.is("current", Some("Paused")), "{visual:?}");
    }
    assert_eq!(brave_player.reads_of("PlaybackStatus"), brave_reads);
    let alias = host.subscribe("alias");
    host.wait_state(alias, "current", Some("Paused"), HEARTBEAT)
        .unwrap();
    let berry_reads = strawberry.reads_of("PlaybackStatus");
    brave_player.command("status Playing");
    for sub in [brave, alias] {
        host.wait_state(sub, "current", Some("Playing"), HEARTBEAT)
            .unwrap();
    }
    brave_player.quit();
    for sub in [brave, alias] {
        host.wait_state(sub, "unavailable", None, HEARTBEAT)
            .unwrap();
    }
    assert_eq!(strawberry.reads_of("PlaybackStatus"), berry_reads);
    host.finish().unwrap();
}

#[test]
fn with_several_instances_the_first_one_found_is_shown() {
    let bus = PrivateBus::start();
    let mut later = FakePlayer::start(&bus, "org.mpris.MediaPlayer2.brave.instance9");
    let mut first = FakePlayer::start(&bus, "org.mpris.MediaPlayer2.brave.instance10");
    later.command("status Paused").sync();
    first.command("status Playing").sync();
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("brave");
    host.wait_state(subscription, "current", Some("Playing"), secs(5))
        .unwrap();
    host.discard_pending();
    first.quit();
    host.wait_state(subscription, "current", Some("Paused"), HEARTBEAT)
        .unwrap();
    host.finish().unwrap();
}

#[test]
fn two_buttons_for_the_same_application_show_the_same_thing() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    let timing = Timing {
        linger: HEARTBEAT * 2,
        ..quick()
    };
    let mut host = Host::start(Some(bus.address()), timing);
    let one = host.subscribe("strawberry");
    host.wait_state(one, "current", Some("Stopped"), secs(5))
        .unwrap();
    let two = host.subscribe("strawberry");
    host.wait_state(two, "current", Some("Stopped"), secs(5))
        .unwrap();
    host.discard_pending();
    player.command("status Playing");
    host.wait_state(one, "current", Some("Playing"), HEARTBEAT)
        .unwrap();
    host.wait_state(two, "current", Some("Playing"), HEARTBEAT)
        .unwrap();
    host.unsubscribe(one);
    host.discard_pending();
    player.command("status Paused");
    host.wait_state(two, "current", Some("Paused"), HEARTBEAT)
        .unwrap();
    host.unsubscribe(two);
    host.discard_pending();
    let reads = player.reads_of("PlaybackStatus");
    player.command("status Stopped");
    assert!(host.next_visual(two, HEARTBEAT).is_none());
    assert_eq!(player.reads_of("PlaybackStatus"), reads);
    host.finish().unwrap();
}

#[test]
fn between_observations_no_cached_state_is_blindly_renewed() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("strawberry");
    host.wait_state(subscription, "current", Some("Stopped"), secs(5))
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let reads = player.reads_of("PlaybackStatus");
    host.discard_pending();
    assert!(
        host.take(
            |f| matches!(
                f["message"]["kind"].as_str(),
                Some("visual" | "visual_renew")
            ),
            Duration::from_millis(1400)
        )
        .is_none()
    );
    assert_eq!(
        player.reads_of("PlaybackStatus"),
        reads,
        "the player was polled"
    );
    host.finish().unwrap();
}

#[test]
fn a_player_that_stops_answering_is_marked_stale_and_the_others_carry_on() {
    let bus = PrivateBus::start();
    let mut stuck = FakePlayer::start(&bus, BRAVE);
    let mut fine = FakePlayer::start(&bus, STRAWBERRY);
    stuck.command("status Playing").sync();
    let timing = Timing {
        recheck: Duration::from_millis(500),
        ..quick()
    };
    let mut host = Host::start(Some(bus.address()), timing);
    let hung = host.subscribe("brave");
    let healthy = host.subscribe("strawberry");
    host.wait_state(hung, "current", Some("Playing"), secs(5))
        .unwrap();
    host.wait_state(healthy, "current", Some("Stopped"), secs(5))
        .unwrap();

    stuck.freeze();
    host.discard_pending();
    fine.command("status Playing");
    host.wait_state(healthy, "current", Some("Playing"), HEARTBEAT)
        .expect("the healthy one is not held up");
    host.wait_state(hung, "stale", Some("Playing"), secs(3))
        .unwrap();

    host.discard_pending();
    stuck.thaw();
    fine.command("status Paused");
    host.wait_state(healthy, "current", Some("Paused"), HEARTBEAT)
        .unwrap();
    host.wait_state(hung, "current", Some("Playing"), secs(3))
        .unwrap();
    host.finish().unwrap();
}

#[test]
fn a_player_that_froze_without_a_word_is_marked_stale_by_the_slow_check() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    player.command("status Playing").sync();
    let timing = Timing {
        recheck: Duration::from_millis(400),
        ..quick()
    };
    let mut host = Host::start(Some(bus.address()), timing.clone());
    let subscription = host.subscribe("strawberry");
    host.wait_state(subscription, "current", Some("Playing"), secs(5))
        .unwrap();
    std::thread::sleep(Duration::from_millis(250));

    player.freeze();
    let within = timing.recheck + timing.refresh_deadline + Duration::from_millis(1500);
    host.wait_state(subscription, "stale", Some("Playing"), within)
        .unwrap_or_else(|seen| panic!("never stale: {seen:?}"));

    player.thaw();
    host.wait_state(subscription, "current", Some("Playing"), within)
        .unwrap_or_else(|seen| panic!("never current again: {seen:?}"));
    host.finish().unwrap();
}

#[test]
fn the_slow_check_leaves_a_healthy_player_as_it_is() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    let timing = Timing {
        recheck: Duration::from_millis(150),
        ..quick()
    };
    let mut host = Host::start(Some(bus.address()), timing);
    let subscription = host.subscribe("strawberry");
    host.wait_state(subscription, "current", Some("Stopped"), secs(5))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let reads = player.reads_of("PlaybackStatus");
    host.discard_pending();
    let end = Instant::now() + Duration::from_millis(1200);
    while let Some(visual) =
        host.next_visual(subscription, end.saturating_duration_since(Instant::now()))
    {
        assert!(visual.is("current", Some("Stopped")), "{visual:?}");
    }
    let checks = player.reads_of("PlaybackStatus") - reads;
    assert!((3..=12).contains(&checks), "{checks} checks in 1.2 s");
    host.finish().unwrap();
}

#[test]
fn a_player_that_dies_while_it_is_being_read_is_unavailable_and_never_stale() {
    let bus = PrivateBus::start();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    player.command("status Playing").sync();
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("strawberry");
    host.wait_state(subscription, "current", Some("Playing"), secs(5))
        .unwrap();
    std::thread::sleep(Duration::from_millis(250));

    player
        .command("die PlaybackStatus")
        .command("status Paused");
    let mut seen = Vec::new();
    let end = Instant::now() + secs(3);
    while let Some(visual) =
        host.next_visual(subscription, end.saturating_duration_since(Instant::now()))
    {
        let gone = visual.availability == "unavailable";
        seen.push(visual);
        if gone {
            break;
        }
    }
    assert!(
        seen.last().is_some_and(|v| v.availability == "unavailable"),
        "{seen:?}"
    );
    assert!(seen.iter().all(|v| v.availability != "stale"), "{seen:?}");
    host.finish().unwrap();
}

#[test]
fn a_player_that_never_answered_is_unavailable() {
    let bus = PrivateBus::start();
    let mut frozen = FakePlayer::start(&bus, BRAVE);
    frozen.freeze();
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("brave");
    host.wait_state(subscription, "unavailable", None, secs(3))
        .unwrap();
    frozen.thaw();
    frozen.command("status Playing");
    host.wait_state(subscription, "current", Some("Playing"), HEARTBEAT)
        .unwrap();
    host.finish().unwrap();
}

#[test]
fn losing_the_bus_is_unavailable_and_a_bus_that_returns_is_followed_again() {
    let mut bus = PrivateBus::start_at_fixed_address();
    let before = FakePlayer::start(&bus, STRAWBERRY);
    let before_brave = FakePlayer::start(&bus, BRAVE);
    let mut host = Host::start(Some(bus.address()), quick());
    let subscription = host.subscribe("strawberry");
    let brave = host.subscribe("brave");
    for sub in [subscription, brave] {
        host.wait_state(sub, "current", Some("Stopped"), secs(5))
            .unwrap();
    }
    host.discard_pending();
    bus.kill();
    for sub in [subscription, brave] {
        host.wait_state(sub, "unavailable", None, secs(3)).unwrap();
    }
    drop((before, before_brave));

    bus.restart();
    let mut new_brave = FakePlayer::start(&bus, BRAVE);
    new_brave.command("status Paused").sync();
    let mut player = FakePlayer::start(&bus, STRAWBERRY);
    player.command("status Playing").sync();
    host.wait_state(subscription, "current", Some("Playing"), secs(3))
        .unwrap();
    host.wait_state(brave, "current", Some("Paused"), secs(3))
        .unwrap();
    host.discard_pending();
    player.command("status Paused");
    host.wait_state(subscription, "current", Some("Paused"), HEARTBEAT)
        .unwrap();
    host.finish().unwrap();
}

#[test]
fn without_a_bus_the_visual_is_unavailable() {
    let mut dead = PrivateBus::start();
    let address = dead.address().to_owned();
    dead.kill();
    for address in [Some(address.as_str()), None] {
        let mut host = Host::start(address, quick());
        let subscription = host.subscribe("brave");
        host.wait_state(subscription, "unavailable", None, secs(3))
            .unwrap();
        host.finish().unwrap();
    }
}

#[test]
fn a_subscription_without_a_usable_player_is_unavailable() {
    let bus = PrivateBus::start();
    let _player = FakePlayer::start(&bus, STRAWBERRY);
    let mut host = Host::start(Some(bus.address()), quick());
    for values in [
        json!({}),
        json!({"player":""}),
        json!({"player":7}),
        json!({"player":"a\nb"}),
    ] {
        let subscription = host.subscribe_values("now_playing", values.clone());
        let frame = host
            .take(
                |f| f["correlation"]["request"] == subscription && f["message"]["kind"] == "visual",
                secs(3),
            )
            .expect("a visual");
        assert_eq!(
            frame["message"]["state"],
            json!({"availability":"unavailable"}),
            "{values}"
        );
        host.unsubscribe(subscription);
    }
    let subscription = host.subscribe_values("other", json!({"player":"strawberry"}));
    let frame = host
        .take(
            |f| f["correlation"]["request"] == subscription && f["message"]["kind"] == "visual",
            secs(3),
        )
        .expect("a visual");
    assert_eq!(
        frame["message"]["state"],
        json!({"availability":"unavailable"})
    );
    host.finish().unwrap();
}
