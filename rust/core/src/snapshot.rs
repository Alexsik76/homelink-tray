//! What the engine publishes to the UI: plain, comparable data with the availability of
//! every action already decided, so the UI holds no rules.

use crate::domain::{PcId, PcState, TunnelId, TunnelState};

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
    /// Which actions are enabled.
    pub actions: Actions,
}

/// Enabled actions of a PC.
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

/// The summary shown by the tray icon.
///
/// The tunnel badge is `Starting` if any tunnel is starting, else `Up` if any is up, else
/// `Down`. The PC screen is the first that applies of waking, online, offline, unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayState {
    /// Tunnel badge.
    pub tunnel: TunnelState,
    /// PC screen.
    pub pc: PcState,
}

impl TrayState {
    pub(crate) fn summarise(
        tunnels: impl Iterator<Item = TunnelState>,
        pcs: impl Iterator<Item = PcState>,
    ) -> Self {
        let tunnels: Vec<TunnelState> = tunnels.collect();
        let pcs: Vec<PcState> = pcs.collect();
        let tunnel = [TunnelState::Starting, TunnelState::Up]
            .into_iter()
            .find(|s| tunnels.contains(s))
            .unwrap_or(TunnelState::Down);
        let pc = [PcState::Waking, PcState::Online, PcState::Offline]
            .into_iter()
            .find(|s| pcs.contains(s))
            .unwrap_or(PcState::Unknown);
        Self { tunnel, pc }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_summary_priorities() {
        use PcState as P;
        use TunnelState as T;
        let s = |t: &[T], p: &[P]| TrayState::summarise(t.iter().copied(), p.iter().copied());
        assert_eq!(
            s(&[], &[]),
            TrayState {
                tunnel: T::Down,
                pc: P::Unknown
            }
        );
        assert_eq!(
            s(&[T::Up, T::Starting], &[P::Online, P::Waking]),
            TrayState {
                tunnel: T::Starting,
                pc: P::Waking
            }
        );
        assert_eq!(
            s(&[T::Down, T::Up], &[P::Unknown, P::Offline, P::Online]),
            TrayState {
                tunnel: T::Up,
                pc: P::Online
            }
        );
        assert_eq!(
            s(&[T::Down], &[P::Unknown, P::Offline]),
            TrayState {
                tunnel: T::Down,
                pc: P::Offline
            }
        );
    }
}
