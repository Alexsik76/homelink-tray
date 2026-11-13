//! Behaviour that only exists with several PCs: a shared tunnel, PCs without a tunnel,
//! connect-and-wake for an inactive PC, independent failure counters, and a randomised
//! check of invariants.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use homelink_core::domain::{PcState, PowerAction, TunnelState, WakeKind};
use homelink_core::machine::Command;
use homelink_core::notice::Notice;
use homelink_core::snapshot::{DisplayState, TunnelCorner};
use homelink_core::testing::{Harness, multi_config, pc_id, tunnel_id};

const HOME_PROBE: &str = "192.168.88.1";
const OFFICE_PROBE: &str = "10.8.0.1";
const DESKTOP: &str = "192.168.88.10";
const LAPTOP: &str = "192.168.88.11";
const OFFICE_PC: &str = "10.8.0.20";
const NAS: &str = "10.0.0.7";

fn env() -> Harness {
    Harness::new(multi_config())
}

/// Runs held jobs until none is left.
fn drain(h: &mut Harness) {
    while h.jobs.as_ref().unwrap().pending() > 0 {
        h.run_jobs();
    }
}

// ---------- shared tunnel ----------

#[test]
fn two_pcs_waking_together_start_the_shared_tunnel_once() {
    let mut h = Harness::manual(multi_config());
    h.reachable(HOME_PROBE, true);
    h.wake_pc("desktop");
    h.wake_pc("laptop"); // arrives while the tunnel is starting
    drain(&mut h);
    assert_eq!(
        h.tunnel.ups(),
        [tunnel_id("home")],
        "one start for both PCs"
    );
    assert_eq!(h.wake.woken(), [pc_id("desktop"), pc_id("laptop")]);
    assert_eq!(
        (h.pc("desktop"), h.pc("laptop")),
        (PcState::Waking, PcState::Waking)
    );
    assert_eq!(h.tunnel_state("home"), TunnelState::Up);
}

#[test]
fn a_second_pc_woken_after_the_tunnel_is_up_does_not_touch_the_tunnel() {
    let mut h = env();
    h.reachable(HOME_PROBE, true);
    h.wake_pc("desktop");
    h.wake_pc("laptop");
    assert_eq!(h.tunnel.ups().len(), 1);
    assert_eq!(
        h.wake.calls().last(),
        Some(&(pc_id("laptop"), WakeKind::MagicPacket))
    );
    assert!(h.tunnel.downs().is_empty());
}

#[test]
fn waking_an_offline_pc_behind_an_up_tunnel_only_sends_the_wake_request() {
    let mut h = env();
    h.tunnel.set_running(&tunnel_id("home"), true);
    h.reachable(HOME_PROBE, true);
    h.refresh();
    assert_eq!(
        (h.tunnel_state("home"), h.pc("desktop")),
        (TunnelState::Up, PcState::Offline)
    );
    h.wake_pc("desktop");
    assert!(h.tunnel.ups().is_empty(), "the tunnel is already up");
    assert_eq!(h.wake.woken(), [pc_id("desktop")]);
    assert_eq!(h.pc("desktop"), PcState::Waking);
}

#[test]
fn a_failed_tunnel_start_clears_every_waiting_pc_with_one_notice() {
    let mut h = Harness::manual(multi_config());
    h.tunnel.fail_up_with(Some("no service"));
    h.wake_pc("desktop");
    h.wake_pc("laptop");
    drain(&mut h);
    assert_eq!(
        h.notices(),
        [Notice::TunnelFailed {
            tunnel: "Home".into(),
            detail: "no service".into()
        }]
    );
    assert!(h.wake.woken().is_empty());
    // Both can be tried again.
    h.tunnel.fail_up_with(None);
    h.reachable(HOME_PROBE, true);
    h.wake_pc("desktop");
    drain(&mut h);
    assert_eq!(h.wake.woken(), [pc_id("desktop")]);
}

#[test]
fn a_tunnel_start_timeout_is_one_notice_and_wakes_nobody() {
    let mut h = Harness::manual(multi_config());
    h.wake_pc("desktop");
    h.wake_pc("laptop");
    // The probe host never answers: run jobs and let a second pass at a time.
    for _ in 0..60 {
        h.run_jobs();
        h.advance(1);
    }
    drain(&mut h);
    assert!(h.wake.woken().is_empty());
    assert_eq!(
        h.notices(),
        [Notice::TunnelTimeout {
            tunnel: "Home".into()
        }]
    );
}

#[test]
fn losing_a_shared_tunnel_is_one_notice_and_no_pc_lost() {
    let mut h = env();
    h.reachable(HOME_PROBE, true);
    h.reachable(DESKTOP, true);
    h.reachable(LAPTOP, true);
    h.tunnel.set_running(&tunnel_id("home"), true);
    h.refresh();
    assert_eq!(
        (h.pc("desktop"), h.pc("laptop")),
        (PcState::Online, PcState::Online)
    );
    h.clear_notices();

    h.tunnel.set_running(&tunnel_id("home"), false);
    h.reachable(DESKTOP, false);
    h.reachable(LAPTOP, false);
    h.refresh();
    h.refresh();
    assert_eq!(
        (h.pc("desktop"), h.pc("laptop")),
        (PcState::Unknown, PcState::Unknown)
    );
    assert_eq!(
        h.notices(),
        [Notice::TunnelLost {
            tunnel: "Home".into()
        }]
    );
}

#[test]
fn tunnels_are_independent() {
    let mut h = env();
    h.reachable(HOME_PROBE, true);
    h.reachable(OFFICE_PROBE, true);
    h.reachable(DESKTOP, true);
    h.reachable(OFFICE_PC, true);
    h.toggle_tunnel("home");
    h.toggle_tunnel("office");
    assert_eq!(
        (h.tunnel_state("home"), h.tunnel_state("office")),
        (TunnelState::Up, TunnelState::Up)
    );
    assert_eq!(
        (h.pc("desktop"), h.pc("office-pc")),
        (PcState::Online, PcState::Online)
    );

    h.toggle_tunnel("office");
    assert_eq!(h.tunnel_state("office"), TunnelState::Down);
    assert_eq!(h.pc("office-pc"), PcState::Unknown);
    assert_eq!(
        (h.tunnel_state("home"), h.pc("desktop")),
        (TunnelState::Up, PcState::Online)
    );
    assert_eq!(h.tunnel.downs(), [tunnel_id("office")]);
    assert!(h.notices().is_empty(), "a user disconnect is silent");
}

// ---------- PC without a tunnel ----------

#[test]
fn a_pc_without_a_tunnel_is_watched_and_woken_without_any_tunnel() {
    let mut h = env();
    h.refresh();
    assert_eq!(h.pc("nas"), PcState::Offline);
    assert_eq!(h.tunnel_state("home"), TunnelState::Down);

    h.wake_pc("nas");
    assert!(h.tunnel.ups().is_empty(), "no tunnel is involved");
    assert_eq!(h.wake.calls(), [(pc_id("nas"), WakeKind::MagicPacket)]);
    assert_eq!(h.pc("nas"), PcState::Waking);

    h.reachable(NAS, true);
    h.refresh();
    assert_eq!(h.pc("nas"), PcState::Online);
    assert_eq!(
        h.notices(),
        [Notice::PcOnline {
            pc: "NAS".into(),
            host: NAS.into()
        }]
    );
}

#[test]
fn a_pc_without_a_tunnel_runs_power_commands_when_online() {
    let mut h = env();
    h.reachable(NAS, true);
    h.refresh();
    h.power_pc("nas", PowerAction::Shutdown);
    assert_eq!(h.power.calls(), [(pc_id("nas"), PowerAction::Shutdown)]);
}

#[test]
fn a_tunnel_down_does_not_affect_a_pc_without_a_tunnel() {
    let mut h = env();
    h.reachable(NAS, true);
    h.refresh();
    h.refresh();
    assert_eq!(h.pc("nas"), PcState::Online);
    assert_eq!(h.tunnel_state("home"), TunnelState::Down);
    assert!(h.notices().is_empty());
}

// ---------- connect-and-wake ----------

#[test]
fn connect_and_wake_for_an_inactive_pc_is_one_command() {
    let mut h = env();
    h.reachable(OFFICE_PROBE, true);
    h.wake_pc("office-pc"); // tunnel down, PC off: one command does both
    assert_eq!(h.tunnel.ups(), [tunnel_id("office")]);
    assert_eq!(h.wake.calls(), [(pc_id("office-pc"), WakeKind::Command)]);
    assert_eq!(
        (h.tunnel_state("office"), h.pc("office-pc")),
        (TunnelState::Up, PcState::Waking)
    );
    // The other tunnel and its PCs are untouched.
    assert_eq!(
        (h.tunnel_state("home"), h.pc("desktop")),
        (TunnelState::Down, PcState::Unknown)
    );
}

#[test]
fn connect_and_wake_for_a_pc_that_already_answers_sends_nothing() {
    let mut h = env();
    h.reachable(OFFICE_PROBE, true);
    h.reachable(OFFICE_PC, true);
    h.wake_pc("office-pc");
    assert_eq!(h.pc("office-pc"), PcState::Online);
    assert!(h.wake.woken().is_empty());
}

// ---------- failure counters ----------

#[test]
fn failure_counters_are_independent_per_pc() {
    let mut h = env();
    for host in [HOME_PROBE, DESKTOP, LAPTOP] {
        h.reachable(host, true);
    }
    h.tunnel.set_running(&tunnel_id("home"), true);
    h.refresh();
    assert_eq!(
        (h.pc("desktop"), h.pc("laptop")),
        (PcState::Online, PcState::Online)
    );
    h.clear_notices();

    // Round 1: both fail once.
    h.reachable(DESKTOP, false);
    h.reachable(LAPTOP, false);
    h.refresh();
    // Round 2: the desktop answers again (its counter restarts), the laptop fails again.
    h.reachable(DESKTOP, true);
    h.refresh();
    assert_eq!(
        (h.pc("desktop"), h.pc("laptop")),
        (PcState::Online, PcState::Online)
    );
    // Round 3: the desktop fails once more; the laptop reaches its third failure.
    h.reachable(DESKTOP, false);
    h.refresh();
    assert_eq!(h.pc("laptop"), PcState::Offline);
    assert_eq!(
        h.pc("desktop"),
        PcState::Online,
        "the desktop is only at one failure"
    );
    assert_eq!(
        h.notices(),
        [Notice::PcLost {
            pc: "Laptop".into()
        }]
    );
}

#[test]
fn a_power_command_silences_only_its_own_pc() {
    let mut h = env();
    for host in [HOME_PROBE, DESKTOP, LAPTOP] {
        h.reachable(host, true);
    }
    h.tunnel.set_running(&tunnel_id("home"), true);
    h.refresh();
    h.power_pc("desktop", PowerAction::Shutdown);
    h.clear_notices();
    h.reachable(DESKTOP, false);
    h.reachable(LAPTOP, false);
    for _ in 0..3 {
        h.refresh();
    }
    assert_eq!(
        h.notices(),
        [Notice::PcLost {
            pc: "Laptop".into()
        }]
    );
}

#[test]
fn a_failing_wake_of_one_pc_does_not_block_another() {
    let mut h = env();
    h.reachable(HOME_PROBE, true);
    h.reachable(NAS, false);
    h.wake.fail_with(Some("boom"));
    h.wake_pc("nas");
    assert_ne!(h.pc("nas"), PcState::Waking);
    assert_eq!(
        h.notices(),
        [Notice::WakeFailed {
            pc: "NAS".into(),
            detail: "boom".into()
        }]
    );
    h.wake.fail_with(None);
    h.wake_pc("desktop");
    assert_eq!(h.pc("desktop"), PcState::Waking);
}

// ---------- snapshot ----------

#[test]
fn the_snapshot_groups_pcs_by_tunnel_and_summarises_for_the_tray() {
    let mut h = env();
    h.reachable(NAS, true);
    h.refresh();
    let snapshot = h.sink.snapshots().pop().unwrap();
    assert_eq!(snapshot.groups.len(), 3);
    let names: Vec<Vec<&str>> = snapshot
        .groups
        .iter()
        .map(|g| g.pcs.iter().map(|p| p.name.as_str()).collect())
        .collect();
    assert_eq!(
        names,
        [vec!["Desktop", "Laptop"], vec!["Office PC"], vec!["NAS"]]
    );
    assert!(snapshot.groups[2].tunnel.is_none());
    assert_eq!(snapshot.tray.pcs, DisplayState::Ok, "the NAS is online");
    assert_eq!(snapshot.tray.tunnels, TunnelCorner::None);
    assert_eq!(
        (snapshot.tray.tunnels_up, snapshot.tray.tunnels_total),
        (0, 2)
    );
    assert_eq!(
        (snapshot.tray.pcs_online, snapshot.tray.pcs_total),
        (1, 4),
        "the NAS is online"
    );
}

#[test]
fn publishing_happens_only_when_something_changed() {
    let mut h = env();
    h.refresh();
    let count = h.sink.snapshots().len();
    h.refresh();
    h.refresh();
    assert_eq!(
        h.sink.snapshots().len(),
        count,
        "idle polling publishes nothing"
    );
}

// ---------- randomised invariants ----------

/// A tiny deterministic generator (xorshift), so the test needs no extra crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn random_operations_keep_the_invariants() {
    let hosts = [HOME_PROBE, OFFICE_PROBE, DESKTOP, LAPTOP, OFFICE_PC, NAS];
    let pcs = ["desktop", "laptop", "office-pc", "nas"];
    let behind: [(&str, Option<&str>); 4] = [
        ("desktop", Some("home")),
        ("laptop", Some("home")),
        ("office-pc", Some("office")),
        ("nas", None),
    ];
    for seed in 1..=60u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut h = env();
        h.engine.pump();
        for _ in 0..80 {
            match rng.below(7) {
                0 => h.reachable(hosts[rng.below(hosts.len())], rng.below(2) == 0),
                1 => h.tunnel.set_running(
                    &tunnel_id(["home", "office"][rng.below(2)]),
                    rng.below(2) == 0,
                ),
                2 => h.wake_pc(pcs[rng.below(pcs.len())]),
                3 => h.toggle_tunnel(["home", "office"][rng.below(2)]),
                4 => h.power_pc(
                    pcs[rng.below(pcs.len())],
                    if rng.below(2) == 0 {
                        PowerAction::Sleep
                    } else {
                        PowerAction::Shutdown
                    },
                ),
                5 => h.advance(rng.below(200) as u64),
                _ => h.refresh(),
            }
            // A PC behind a tunnel is never Online or Waking while its tunnel is Down.
            for (pc, tunnel) in behind {
                if let Some(t) = tunnel
                    && h.tunnel_state(t) == TunnelState::Down
                {
                    assert!(
                        !matches!(h.pc(pc), PcState::Online | PcState::Waking),
                        "seed {seed}: {pc} is {:?} behind a down tunnel",
                        h.pc(pc)
                    );
                }
            }
            // Every published snapshot is consistent with the machine.
            let last = h.sink.snapshots().pop().unwrap();
            assert_eq!(
                last,
                h.engine.machine().snapshot(),
                "seed {seed}: stale snapshot"
            );
        }
    }
}

#[test]
fn commands_for_unknown_ids_are_harmless() {
    let mut h = env();
    h.send(Command::Wake(pc_id("ghost")));
    h.send(Command::ToggleTunnel(tunnel_id("ghost")));
    h.send(Command::Power(pc_id("ghost"), PowerAction::Sleep));
    assert!(h.notices().is_empty());
}
