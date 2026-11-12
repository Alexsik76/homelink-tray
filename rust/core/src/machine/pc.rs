//! PC transitions P1-P13.
//!
//! | #   | State                 | Event                          | New state | Notices / counters                                      |
//! | --- | --------------------- | ------------------------------ | --------- | ------------------------------------------------------- |
//! | P1  | any                   | poll: ready                    | Online    | failures = 0; `PcOnline` if it was Waking               |
//! | P2  | Online                | poll: not ready, failures < N  | Online    | failures += 1                                           |
//! | P3  | Waking                | poll: not ready, before limit  | Waking    | failures += 1                                           |
//! | P4  | other                 | poll: not ready                | Offline   | `PcLost` (was Online, no power command), `PcTimeout`    |
//! | P5  | any                   | tunnel Down or silent          | Unknown   | failures = 0; no notice (see `tunnel.rs`)               |
//! | P6  | any                   | check: ready                   | Online    | failures = 0; no `PcOnline`                             |
//! | P7  | any                   | check: not ready               | same      | effect `Wake`                                           |
//! | P8  | any                   | `WakeDone(Ok)`                 | Waking    | `waking_until = now + wake_timeout`                     |
//! | P9  | any                   | `WakeDone(Err)`                | same      | `WakeFailed`                                            |
//! | P10 | Online                | `Power(a)`                     | same      | effect `Power`                                          |
//! | P11 | other                 | `Power(a)`                     | same      | `PowerUnavailable`                                      |
//! | P12 | any                   | `PowerDone(Ok)`                | same      | `power_sent = true`; `SleepSent` / `ShutdownSent`       |
//! | P13 | any                   | `PowerDone(Err)`               | same      | `PowerFailed`                                           |
//!
//! `N` is `offline_after_failures`; P2 compares the counter after the increment.

use super::{Effect, Machine, Output, PcAction, PcProbe};
use crate::domain::{PcId, PcState, PowerAction, TunnelState};
use crate::notice::Notice;
use crate::ports::Time;

impl Machine {
    /// `Wake(pc)`: connect-and-wake for a PC behind a tunnel (T1-T3), a plain wake otherwise.
    pub(super) fn wake_pc(&mut self, id: &PcId, now: Time, out: &mut Output) {
        let Some(pi) = self.pc_idx(id) else {
            log::warn!("wake: unknown PC {id}");
            return;
        };
        let Some(p) = self.pcs.get(pi) else { return };
        log::info!("action: wake {} (state {:?})", p.id, p.state);
        if !p.can_wake {
            log::info!("skipped: {} has no wake method", p.id);
            return;
        }
        if matches!(p.state, PcState::Online | PcState::Waking) {
            log::info!("skipped: {} is {:?}", p.id, p.state);
            return;
        }
        if p.action.is_some() || p.connect_pending {
            log::info!("skipped: another action is in progress");
            return;
        }
        match p.tunnel {
            Some(ti) => self.wake_via_tunnel(pi, ti, now, out),
            None => {
                if let Some(p) = self.pcs.get_mut(pi) {
                    p.action = Some(PcAction::Wake);
                    out.effects.push(Effect::Wake(p.id.clone()));
                }
            }
        }
    }

    /// `Power(pc, action)`: P10 or P11.
    pub(super) fn power_pc(&mut self, id: &PcId, action: PowerAction, out: &mut Output) {
        let Some(p) = self.pc_idx(id).and_then(|i| self.pcs.get_mut(i)) else {
            log::warn!("power: unknown PC {id}");
            return;
        };
        log::info!("action: {action:?} {}", p.id);
        if p.action.is_some() {
            log::info!("skipped: another action is in progress");
            return;
        }
        if p.state != PcState::Online {
            log::info!("skipped: {} is {:?}", p.id, p.state);
            out.notices
                .push(Notice::PowerUnavailable { pc: p.name.clone() });
            return;
        }
        let supported = match action {
            PowerAction::Sleep => p.can_sleep,
            PowerAction::Shutdown => p.can_shutdown,
        };
        if !supported {
            log::info!("skipped: {} has no {action:?} command", p.id);
            return;
        }
        p.action = Some(PcAction::Power(action));
        out.effects.push(Effect::Power {
            pc: p.id.clone(),
            action,
        });
    }

    /// The result of a PC probe: P1-P4 for polling, P6 and P7 for the wake check.
    pub(super) fn pc_probed(
        &mut self,
        id: &PcId,
        kind: PcProbe,
        ready: bool,
        now: Time,
        out: &mut Output,
    ) {
        let Some(pi) = self.pc_idx(id) else { return };
        let Some(p) = self.pcs.get_mut(pi) else {
            return;
        };
        let tunnel_up = p
            .tunnel
            .and_then(|ti| self.tunnels.get(ti))
            .is_none_or(|t| t.state == TunnelState::Up);
        match kind {
            PcProbe::Poll => {
                p.probing = false;
                // Ignored while an action runs (`refresh()` skipped while busy) or when
                // the tunnel is not up any more.
                if p.action.is_some() || !tunnel_up {
                    return;
                }
                self.poll_result(pi, ready, now, out);
            }
            PcProbe::Check => {
                if p.action != Some(PcAction::Check) {
                    return;
                }
                if !tunnel_up {
                    p.action = None;
                    return;
                }
                if ready {
                    // P6
                    p.action = None;
                    p.failures = 0;
                    p.next_probe = now + p.timing.poll;
                    self.set_pc_state(pi, PcState::Online);
                } else {
                    // P7
                    p.action = Some(PcAction::Wake);
                    out.effects.push(Effect::Wake(p.id.clone()));
                }
            }
        }
    }

    /// P1-P4, a port of the PC half of `_refresh`.
    fn poll_result(&mut self, pi: usize, ready: bool, now: Time, out: &mut Output) {
        let Some(p) = self.pcs.get_mut(pi) else {
            return;
        };
        let was = p.state;
        let was_waking = was == PcState::Waking;
        if ready {
            // P1
            p.waking_until = None;
            p.failures = 0;
            let notice = was_waking.then(|| Notice::PcOnline {
                pc: p.name.clone(),
                host: p.host.to_string(),
            });
            self.set_pc_state(pi, PcState::Online);
            out.notices.extend(notice);
        } else {
            // a single failed check must not flip an online PC to offline
            p.failures += 1;
            let limit = p.timing.offline_after_failures;
            if was == PcState::Online && p.failures < limit {
                // P2
                log::info!("pc {} check failed ({} of {limit})", p.id, p.failures);
            } else if was_waking && p.waking_until.is_some_and(|until| now < until) {
                // P3
            } else {
                // P4
                let power_sent = p.power_sent;
                let name = p.name.clone();
                let host = p.host.to_string();
                self.set_pc_state(pi, PcState::Offline);
                if was == PcState::Online && !power_sent {
                    log::warn!("pc {host} stopped responding");
                    out.notices.push(Notice::PcLost { pc: name.clone() });
                }
                if was_waking {
                    log::warn!("pc {host} did not answer within the wake timeout");
                    out.notices.push(Notice::PcTimeout { pc: name });
                }
            }
        }
        if let Some(p) = self.pcs.get_mut(pi) {
            p.next_probe = now + p.interval();
        }
    }

    /// P8 and P9.
    pub(super) fn wake_done(
        &mut self,
        id: &PcId,
        result: Result<(), String>,
        now: Time,
        out: &mut Output,
    ) {
        let Some(pi) = self.pc_idx(id) else { return };
        let Some(p) = self.pcs.get_mut(pi) else {
            return;
        };
        p.action = None;
        match result {
            Ok(()) => {
                log::info!("wake request sent to {}", p.id);
                let wake_poll = p.timing.wake_poll;
                p.waking_until = Some(now + p.timing.wake_timeout);
                p.next_probe = now + wake_poll;
                let tunnel = p.tunnel;
                self.set_pc_state(pi, PcState::Waking);
                // The tunnel cycle drives the PC's probes: bring it forward to the shorter
                // interval (`poll_interval`).
                if let Some(t) = tunnel.and_then(|ti| self.tunnels.get_mut(ti)) {
                    t.next_probe = t.next_probe.min(now + wake_poll);
                }
            }
            Err(detail) => {
                log::error!("wake of {} failed: {detail}", p.id);
                out.notices.push(Notice::WakeFailed {
                    pc: p.name.clone(),
                    detail,
                });
            }
        }
    }

    /// P12 and P13. The state is left to the polling.
    pub(super) fn power_done(
        &mut self,
        id: &PcId,
        action: PowerAction,
        result: Result<(), String>,
        out: &mut Output,
    ) {
        let Some(p) = self.pc_idx(id).and_then(|i| self.pcs.get_mut(i)) else {
            return;
        };
        p.action = None;
        match result {
            Ok(()) => {
                log::info!("{action:?} command accepted by {}", p.id);
                p.power_sent = true;
                let (pc, host) = (p.name.clone(), p.host.to_string());
                out.notices.push(match action {
                    PowerAction::Sleep => Notice::SleepSent { pc, host },
                    PowerAction::Shutdown => Notice::ShutdownSent { pc, host },
                });
            }
            Err(detail) => {
                log::error!("{action:?} command for {} failed: {detail}", p.id);
                out.notices.push(Notice::PowerFailed {
                    pc: p.name.clone(),
                    detail,
                });
            }
        }
    }
}
