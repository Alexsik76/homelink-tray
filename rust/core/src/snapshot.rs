//! What the engine publishes to the UI: plain, comparable data with the availability of
//! every action and the display state already decided, so the UI holds no rules.

use std::net::Ipv4Addr;
use std::time::Duration;

use crate::domain::{PcId, PcState, RelayKind, TunnelId, TunnelState};
use crate::notice::Notice;
use crate::ports::Time;

/// Check times in a snapshot are rounded down to this step, so that idle polling does not
/// change the snapshot (and so is not published) every few seconds. Showing "last check"
/// with this precision is enough.
pub const CHECK_STEP: Duration = Duration::from_secs(30);

/// The whole state at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// One group per tunnel in configuration order, then one for PCs without a tunnel
    /// (when there are any).
    pub groups: Vec<GroupView>,
    /// The state summarised for the tray icon.
    pub tray: TrayState,
}

/// A tunnel and the PCs reached through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    /// The tunnel; `None` for the group of PCs without one.
    pub tunnel: Option<TunnelView>,
    /// The PCs, in configuration order.
    pub pcs: Vec<PcView>,
}

/// How a tunnel or a PC is shown: the five states of the state marker.
///
/// Decided in the core (section 5.6 of `docs/rust-architecture.md`); the UI only maps it to
/// a colour and a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayState {
    /// Tunnel up, PC online.
    Ok,
    /// Tunnel connecting, PC waking or an action on it in progress.
    Busy,
    /// Tunnel stopped, PC off.
    Off,
    /// Not known yet.
    Unknown,
    /// The last requested action failed (section 5.6).
    Fail,
}

impl DisplayState {
    /// Suffix of the locale key `state.<name>`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Busy => "busy",
            Self::Off => "off",
            Self::Unknown => "unknown",
            Self::Fail => "fail",
        }
    }
}

/// What the toggle button of a tunnel would do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelToggle {
    /// Bring the tunnel up (it is down).
    Raise,
    /// Bring the tunnel down (it is starting or up).
    Lower,
}

impl TunnelToggle {
    /// The action `ToggleTunnel` takes in `state`.
    pub fn for_state(state: TunnelState) -> Self {
        if state == TunnelState::Down {
            Self::Raise
        } else {
            Self::Lower
        }
    }
}

/// A tunnel as shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelView {
    /// Identifier.
    pub id: TunnelId,
    /// Display name.
    pub name: String,
    /// State.
    pub state: TunnelState,
    /// Whether the toggle button is enabled (no start or stop is running).
    pub can_toggle: bool,
    /// The state for the marker, with failure and "never checked" taken into account.
    pub display: DisplayState,
    /// When the last check that counted finished (see [`CHECK_STEP`]); `None` before the
    /// first one. A time of the engine's clock.
    pub last_checked: Option<Time>,
    /// The error of the failed action while `display` is [`DisplayState::Fail`]; render it
    /// with `Translator::notice`.
    pub last_error: Option<Notice>,
    /// The kind of tunnel, e.g. `WireGuard`.
    pub kind: &'static str,
    /// Names of the PCs behind the tunnel, in configuration order.
    pub pcs: Vec<String>,
    /// What the toggle would do now.
    pub toggle: TunnelToggle,
}

/// How a PC is woken, without secrets or paths; render it with `Translator::wake_summary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeSummary {
    /// Through a relay host over SSH.
    Relay {
        /// Id of the relay.
        relay: String,
        /// Its preset.
        kind: RelayKind,
    },
    /// A magic packet sent from this computer.
    MagicPacket {
        /// Destination address.
        address: Ipv4Addr,
        /// UDP port.
        port: u16,
    },
    /// An arbitrary command.
    Command {
        /// Its program (first element).
        program: String,
    },
}

/// A PC as shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcView {
    /// Identifier.
    pub id: PcId,
    /// Display name.
    pub name: String,
    /// State.
    pub state: PcState,
    /// An action (connect, wake or power command) is running.
    pub busy: bool,
    /// Which actions are enabled now.
    pub actions: Actions,
    /// The state for the marker, with failure and in-progress actions taken into account.
    pub display: DisplayState,
    /// When the last check that counted finished (see [`CHECK_STEP`]); `None` before the
    /// first one. A time of the engine's clock.
    pub last_checked: Option<Time>,
    /// The error of the failed action while `display` is [`DisplayState::Fail`]; render it
    /// with `Translator::notice`.
    pub last_error: Option<Notice>,
    /// Address (IP or host name).
    pub address: String,
    /// Name of the tunnel the PC is reached through, if any.
    pub tunnel_name: Option<String>,
    /// How it is woken; `None` when it has no wake method.
    pub wake: Option<WakeSummary>,
    /// TCP ports that must be open for the PC to count as online.
    pub ports: Vec<u16>,
    /// Which actions exist in the configuration at all, whatever the state: an action that
    /// is configured but not enabled in `actions` is "not available now"; one that is not
    /// configured is "not configured".
    pub configured: Actions,
}

/// Actions of a PC: the enabled ones, or the configured ones.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Actions {
    /// Wake (with the tunnel brought up first when needed).
    pub wake: bool,
    /// Sleep.
    pub sleep: bool,
    /// Shut down.
    pub shutdown: bool,
    /// Open Remote Desktop.
    pub rdp: bool,
}

/// The corner square of the tray icon: the aggregate state of the tunnels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelCorner {
    /// No square: every tunnel is stopped or not known, or there are none.
    None,
    /// A tunnel is up.
    Ok,
    /// A tunnel is connecting.
    Busy,
    /// A tunnel failed.
    Fail,
}

/// The summary shown by the tray icon. The monitor glyph reads as "the PC", so the screen
/// shows the PCs and the corner square the tunnels. This deliberately differs from section 9
/// of `docs/design/ui-handoff.md` (owner's decision of 13.11.2026).
///
/// `pcs` is the first that applies of: any PC failed, any busy, any online, all off, else
/// unknown (also with no PCs). `tunnels` is `Fail` if any tunnel failed, else `Busy` if any is
/// connecting, else `Ok` if any is up, else `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayState {
    /// Fill of the screen: the aggregate of the PCs.
    pub pcs: DisplayState,
    /// The corner square: the aggregate of the tunnels.
    pub tunnels: TunnelCorner,
    /// Tunnels that are up.
    pub tunnels_up: usize,
    /// All tunnels.
    pub tunnels_total: usize,
    /// PCs that are online.
    pub pcs_online: usize,
    /// All PCs.
    pub pcs_total: usize,
    /// PCs whose state is not known.
    pub pcs_unknown: usize,
}

impl TrayState {
    pub(crate) fn summarise(
        tunnels: impl Iterator<Item = (DisplayState, TunnelState)>,
        pcs: impl Iterator<Item = (DisplayState, PcState)>,
    ) -> Self {
        let tunnels: Vec<(DisplayState, TunnelState)> = tunnels.collect();
        let pcs: Vec<(DisplayState, PcState)> = pcs.collect();
        let tunnel_any = |d: DisplayState| tunnels.iter().any(|(x, _)| *x == d);
        let corner = if tunnel_any(DisplayState::Fail) {
            TunnelCorner::Fail
        } else if tunnel_any(DisplayState::Busy) {
            TunnelCorner::Busy
        } else if tunnel_any(DisplayState::Ok) {
            TunnelCorner::Ok
        } else {
            TunnelCorner::None
        };
        let any = |d: DisplayState| pcs.iter().any(|(x, _)| *x == d);
        let screen = if any(DisplayState::Fail) {
            DisplayState::Fail
        } else if any(DisplayState::Busy) {
            DisplayState::Busy
        } else if any(DisplayState::Ok) {
            DisplayState::Ok
        } else if !pcs.is_empty() && pcs.iter().all(|(x, _)| *x == DisplayState::Off) {
            DisplayState::Off
        } else {
            DisplayState::Unknown
        };
        Self {
            pcs: screen,
            tunnels: corner,
            tunnels_up: tunnels
                .iter()
                .filter(|(_, s)| *s == TunnelState::Up)
                .count(),
            tunnels_total: tunnels.len(),
            pcs_online: pcs.iter().filter(|(_, s)| *s == PcState::Online).count(),
            pcs_total: pcs.len(),
            pcs_unknown: pcs.iter().filter(|(_, s)| *s == PcState::Unknown).count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DisplayState as D;

    fn tray(t: &[(D, TunnelState)], p: &[(D, PcState)]) -> TrayState {
        TrayState::summarise(t.iter().copied(), p.iter().copied())
    }

    #[test]
    fn tray_screen_is_the_pc_aggregate_first_match_wins() {
        use PcState as P;
        let screen = |p: &[(D, P)]| tray(&[], p).pcs;
        assert_eq!(screen(&[]), D::Unknown, "no PCs: nothing to fill");
        assert_eq!(screen(&[(D::Unknown, P::Unknown)]), D::Unknown);
        assert_eq!(
            screen(&[(D::Off, P::Offline), (D::Unknown, P::Unknown)]),
            D::Unknown
        );
        assert_eq!(
            screen(&[(D::Off, P::Offline), (D::Off, P::Offline)]),
            D::Off
        );
        assert_eq!(
            screen(&[
                (D::Off, P::Offline),
                (D::Unknown, P::Unknown),
                (D::Ok, P::Online)
            ]),
            D::Ok
        );
        assert_eq!(
            screen(&[
                (D::Ok, P::Online),
                (D::Busy, P::Waking),
                (D::Off, P::Offline)
            ]),
            D::Busy
        );
        assert_eq!(
            screen(&[
                (D::Busy, P::Waking),
                (D::Fail, P::Offline),
                (D::Ok, P::Online)
            ]),
            D::Fail
        );
    }

    #[test]
    fn tray_corner_is_the_tunnel_aggregate() {
        use TunnelState as T;
        let corner = |t: &[(D, T)]| tray(t, &[]).tunnels;
        assert_eq!(corner(&[]), TunnelCorner::None);
        assert_eq!(corner(&[(D::Unknown, T::Down)]), TunnelCorner::None);
        assert_eq!(
            corner(&[(D::Off, T::Down), (D::Unknown, T::Down)]),
            TunnelCorner::None
        );
        assert_eq!(
            corner(&[(D::Off, T::Down), (D::Ok, T::Up)]),
            TunnelCorner::Ok
        );
        assert_eq!(
            corner(&[(D::Ok, T::Up), (D::Busy, T::Starting), (D::Off, T::Down)]),
            TunnelCorner::Busy
        );
        assert_eq!(
            corner(&[(D::Busy, T::Starting), (D::Fail, T::Down), (D::Ok, T::Up)]),
            TunnelCorner::Fail
        );
    }

    #[test]
    fn tray_counts() {
        let t = tray(
            &[
                (D::Ok, TunnelState::Up),
                (D::Fail, TunnelState::Up),
                (D::Off, TunnelState::Down),
            ],
            &[
                (D::Ok, PcState::Online),
                (D::Busy, PcState::Online),
                (D::Off, PcState::Offline),
                (D::Busy, PcState::Waking),
                (D::Unknown, PcState::Unknown),
            ],
        );
        assert_eq!(
            (
                t.tunnels_up,
                t.tunnels_total,
                t.pcs_online,
                t.pcs_total,
                t.pcs_unknown
            ),
            (2, 3, 2, 5, 1)
        );
    }

    #[test]
    fn toggle_follows_the_state() {
        assert_eq!(
            TunnelToggle::for_state(TunnelState::Down),
            TunnelToggle::Raise
        );
        assert_eq!(
            TunnelToggle::for_state(TunnelState::Starting),
            TunnelToggle::Lower
        );
        assert_eq!(
            TunnelToggle::for_state(TunnelState::Up),
            TunnelToggle::Lower
        );
    }
}
