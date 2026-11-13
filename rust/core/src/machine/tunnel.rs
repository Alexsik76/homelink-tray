//! Tunnel transitions T1-T10.
//!
//! | #   | From            | Event                                  | To          | Effects / notices                                  |
//! | --- | --------------- | -------------------------------------- | ----------- | -------------------------------------------------- |
//! | T1  | Down            | `ToggleTunnel` or `Wake(pc)`           | Starting    | `TunnelUp`; deadline; `pc` pending                 |
//! | T2  | Starting (wait) | `Wake(pc)`                             | Starting    | `pc` pending                                       |
//! | T3  | Up              | `Wake(pc)`                             | Up          | check probe of the PC (see P6, P7)                 |
//! | T4  | Starting/Up     | `ToggleTunnel`                         | (observed)  | `TunnelDown`; `expect_down`; pending cleared       |
//! | T5  | Starting        | `TunnelUpDone(Err)`                    | (observed)  | notice `TunnelFailed`; pending cleared; probe now  |
//! | T6  | Starting (wait) | reach probe answers                    | Up          | PC probes for pending and due PCs                  |
//! | T7  | Starting (wait) | reach probe silent after the deadline  | Starting    | notice `TunnelTimeout`; pending cleared; probe now |
//! | T8  | any             | full probe: service not running        | Down        | PCs Unknown; `TunnelLost` if it was Up unasked     |
//! | T9  | any             | full probe: running, host silent       | Starting    | PCs Unknown; no notice                             |
//! | T10 | any             | full probe: host answers               | Up          | PC probes for due PCs                              |
//! | T11 | Starting/Up     | `TunnelDownDone(Err)`                  | (observed)  | notice `TunnelStopFailed`; probe now; stop failure |
//!
//! Failed state (F1-F6, section 5.6 of the architecture): T1 and T4 clear a failure; T5 and
//! T7 set a start failure and T11 a stop failure; a probe that finds the host answering (T6,
//! T10) clears a start failure and one that finds the service not running (T8) clears a stop
//! failure. T8 and T9 also clear the failures of the PCs behind the tunnel (F13).

use super::{
    Effect, Failure, Machine, Observation, Output, PcAction, PcProbe, START_POLL, TunnelAction,
    TunnelFailKind, TunnelProbe,
};
use crate::domain::{PcId, PcState, TunnelId, TunnelState};
use crate::notice::Notice;
use crate::ports::Time;
use crate::snapshot::TunnelToggle;

impl Machine {
    /// `ToggleTunnel`: T1 or T4.
    pub(super) fn toggle_tunnel(&mut self, id: &TunnelId, now: Time, out: &mut Output) {
        let Some(ti) = self.tunnel_idx(id) else {
            log::warn!("toggle: unknown tunnel {id}");
            return;
        };
        let Some(t) = self.tunnels.get(ti) else {
            return;
        };
        log::info!("action: toggle tunnel {} (state {:?})", t.id, t.state);
        if t.action.is_some() {
            log::info!("skipped: another action is in progress");
            return;
        }
        match TunnelToggle::for_state(t.state) {
            TunnelToggle::Raise => self.start_tunnel(ti, now, out),
            TunnelToggle::Lower => self.stop_tunnel(ti, out),
        }
    }

    /// T1: the state becomes Starting at once, the deadline starts counting.
    pub(super) fn start_tunnel(&mut self, ti: usize, now: Time, out: &mut Output) {
        let Some(t) = self.tunnels.get_mut(ti) else {
            return;
        };
        t.failed = None;
        t.state = TunnelState::Starting;
        t.awaiting = Some(now + t.timeout);
        t.expect_down = false;
        t.action = Some(TunnelAction::Up);
        out.effects.push(Effect::TunnelUp(t.id.clone()));
    }

    /// T4.
    fn stop_tunnel(&mut self, ti: usize, out: &mut Output) {
        self.clear_pending(ti);
        let Some(t) = self.tunnels.get_mut(ti) else {
            return;
        };
        t.failed = None;
        t.expect_down = true;
        t.awaiting = None;
        t.action = Some(TunnelAction::Down);
        out.effects.push(Effect::TunnelDown(t.id.clone()));
    }

    fn clear_pending(&mut self, ti: usize) {
        for p in self.pcs.iter_mut().filter(|p| p.tunnel == Some(ti)) {
            p.connect_pending = false;
        }
    }

    /// Puts the PCs behind a tunnel back to Unknown with the counters reset.
    fn reset_pcs(&mut self, ti: usize, now: Time, clear_waking: bool) {
        let ids: Vec<usize> = self
            .pcs
            .iter()
            .enumerate()
            .filter(|(_, p)| p.tunnel == Some(ti))
            .map(|(i, _)| i)
            .collect();
        for pi in ids {
            if let Some(p) = self.pcs.get_mut(pi) {
                p.failed = None;
                p.failures = 0;
                p.next_probe = now;
                if clear_waking {
                    p.waking_until = None;
                }
            }
            self.set_pc_state(pi, PcState::Unknown);
        }
    }

    /// The result of a tunnel probe: T6-T10.
    pub(super) fn tunnel_probed(
        &mut self,
        id: &TunnelId,
        kind: TunnelProbe,
        observation: Observation,
        now: Time,
        out: &mut Output,
    ) {
        let Some(ti) = self.tunnel_idx(id) else {
            return;
        };
        let Some(t) = self.tunnels.get_mut(ti) else {
            return;
        };
        t.probing = false;
        if t.action.is_some() {
            // Like `refresh()` while busy: the result is ignored, the action decides.
            return;
        }
        t.last_check = Some(now);
        let deadline = t.awaiting;
        let mut immediate = false;
        match (kind, observation) {
            (TunnelProbe::Full, Observation::NotRunning) => self.tunnel_down_seen(ti, now, out),
            (_, Observation::Reachable) => self.tunnel_up_seen(ti, now, out),
            (TunnelProbe::Full, Observation::Silent) => self.tunnel_silent_seen(ti, now),
            (TunnelProbe::ReachOnly, _) => {
                if deadline.is_some_and(|deadline| now >= deadline) {
                    immediate = self.start_timed_out(ti, out);
                }
            }
        }
        let interval = self.tunnel_interval(ti);
        if let Some(t) = self.tunnels.get_mut(ti) {
            t.next_probe = if immediate {
                now
            } else if t.awaiting.is_some() {
                now + START_POLL
            } else {
                now + interval
            };
        }
    }

    /// T8.
    fn tunnel_down_seen(&mut self, ti: usize, now: Time, out: &mut Output) {
        let Some(t) = self.tunnels.get_mut(ti) else {
            return;
        };
        let lost = t.state == TunnelState::Up && !t.expect_down;
        t.expect_down = false;
        t.awaiting = None;
        t.state = TunnelState::Down;
        if t.failed
            .as_ref()
            .is_some_and(|f| f.kind == TunnelFailKind::Stop)
        {
            t.failed = None;
        }
        let name = t.name.clone();
        self.clear_pending(ti);
        self.reset_pcs(ti, now, true);
        if lost {
            log::warn!("tunnel {name} stopped unexpectedly");
            out.notices.push(Notice::TunnelLost { tunnel: name });
        }
    }

    /// T9.
    fn tunnel_silent_seen(&mut self, ti: usize, now: Time) {
        if let Some(t) = self.tunnels.get_mut(ti) {
            t.state = TunnelState::Starting;
        }
        self.reset_pcs(ti, now, false);
    }

    /// T6 and T10: the PCs are probed right after the tunnel, so the tunnel is always
    /// checked first.
    fn tunnel_up_seen(&mut self, ti: usize, now: Time, out: &mut Output) {
        if let Some(t) = self.tunnels.get_mut(ti) {
            t.awaiting = None;
            t.expect_down = false;
            t.state = TunnelState::Up;
            if t.failed
                .as_ref()
                .is_some_and(|f| f.kind == TunnelFailKind::Start)
            {
                t.failed = None;
            }
        }
        for p in self.pcs.iter_mut().filter(|p| p.tunnel == Some(ti)) {
            if p.action.is_some() || p.probing {
                continue;
            }
            if p.connect_pending {
                p.connect_pending = false;
                p.action = Some(PcAction::Check);
                out.effects.push(Effect::ProbePc {
                    pc: p.id.clone(),
                    kind: PcProbe::Check,
                });
            } else if p.next_probe <= now {
                p.probing = true;
                out.effects.push(Effect::ProbePc {
                    pc: p.id.clone(),
                    kind: PcProbe::Poll,
                });
            }
        }
    }

    /// T7. Returns true: a full probe follows at once, as `_refresh` did.
    fn start_timed_out(&mut self, ti: usize, out: &mut Output) -> bool {
        let Some(t) = self.tunnels.get_mut(ti) else {
            return false;
        };
        t.awaiting = None;
        log::warn!("tunnel {} did not answer within {:?}", t.name, t.timeout);
        let notice = Notice::TunnelTimeout {
            tunnel: t.name.clone(),
        };
        t.failed = Some(Failure {
            kind: TunnelFailKind::Start,
            notice: notice.clone(),
        });
        out.notices.push(notice);
        self.clear_pending(ti);
        true
    }

    /// The result of starting a tunnel: T5, or the start of the wait for T6.
    pub(super) fn tunnel_up_done(
        &mut self,
        id: &TunnelId,
        result: Result<(), String>,
        now: Time,
        out: &mut Output,
    ) {
        let Some(ti) = self.tunnel_idx(id) else {
            return;
        };
        if let Some(t) = self.tunnels.get_mut(ti) {
            t.action = None;
            t.next_probe = now;
            if let Err(detail) = result {
                log::error!("tunnel start failed: {detail}");
                t.awaiting = None;
                let notice = Notice::TunnelFailed {
                    tunnel: t.name.clone(),
                    detail,
                };
                t.failed = Some(Failure {
                    kind: TunnelFailKind::Start,
                    notice: notice.clone(),
                });
                out.notices.push(notice);
                self.clear_pending(ti);
            }
        }
    }

    /// The result of stopping a tunnel: T11 on an error, then observe the real state at once.
    pub(super) fn tunnel_down_done(
        &mut self,
        id: &TunnelId,
        result: Result<(), String>,
        now: Time,
        out: &mut Output,
    ) {
        if let Some(t) = self.tunnel_idx(id).and_then(|i| self.tunnels.get_mut(i)) {
            t.action = None;
            t.next_probe = now;
            if let Err(detail) = result {
                log::error!("tunnel stop failed: {detail}");
                let notice = Notice::TunnelStopFailed {
                    tunnel: t.name.clone(),
                    detail,
                };
                t.failed = Some(Failure {
                    kind: TunnelFailKind::Stop,
                    notice: notice.clone(),
                });
                out.notices.push(notice);
            }
        }
    }

    /// `Wake` for a PC behind a tunnel: T1, T2 or T3.
    pub(super) fn wake_via_tunnel(&mut self, pi: usize, ti: usize, now: Time, out: &mut Output) {
        let Some(t) = self.tunnels.get(ti) else {
            return;
        };
        match (t.action, t.state) {
            (Some(TunnelAction::Down), _) => {
                log::info!("skipped: tunnel {} is being stopped", t.id);
            }
            (Some(TunnelAction::Up), _) => self.set_pending(pi),
            (None, TunnelState::Up) => {
                // T3
                if let Some(p) = self.pcs.get_mut(pi) {
                    p.action = Some(PcAction::Check);
                    out.effects.push(Effect::ProbePc {
                        pc: p.id.clone(),
                        kind: PcProbe::Check,
                    });
                }
            }
            (None, _) if t.awaiting.is_some() => self.set_pending(pi), // T2
            (None, _) => {
                // T1: Down, or Starting without anyone waiting.
                self.set_pending(pi);
                self.start_tunnel(ti, now, out);
            }
        }
    }

    fn set_pending(&mut self, pi: usize) {
        if let Some(p) = self.pcs.get_mut(pi) {
            p.connect_pending = true;
        }
    }
}

/// Helper for tests and the engine: the PC ids waiting for a tunnel.
impl Machine {
    /// PCs that wait for their tunnel to come up before being woken.
    pub fn pending_wakes(&self) -> Vec<PcId> {
        self.pcs
            .iter()
            .filter(|p| p.connect_pending)
            .map(|p| p.id.clone())
            .collect()
    }
}
