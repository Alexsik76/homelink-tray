//! Notifications the program shows to the user.
//!
//! A [`Notice`] is typed, so the compiler checks that each one has a key. The locale files
//! hold `<key>.title` and `<key>.body`; the parameters returned by [`Notice::params`] fill
//! the `{placeholders}` in them.

/// Longest `detail` shown in a notification, in characters. The full text goes to the log.
pub const DETAIL_LIMIT: usize = 200;

/// Something the user should be told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The tunnel service could not be started.
    TunnelFailed {
        /// Tunnel name.
        tunnel: String,
        /// Reason reported by the system.
        detail: String,
    },
    /// A tunnel that was up stopped without being asked to.
    TunnelLost {
        /// Tunnel name.
        tunnel: String,
    },
    /// The tunnel started but its probe host never answered.
    TunnelTimeout {
        /// Tunnel name.
        tunnel: String,
    },
    /// The wake request could not be sent.
    WakeFailed {
        /// PC name.
        pc: String,
        /// Reason.
        detail: String,
    },
    /// A PC answered after a wake request.
    PcOnline {
        /// PC name.
        pc: String,
        /// PC address.
        host: String,
    },
    /// A PC did not answer within the wake timeout.
    PcTimeout {
        /// PC name.
        pc: String,
    },
    /// An online PC stopped answering without a power command.
    PcLost {
        /// PC name.
        pc: String,
    },
    /// The sleep command was accepted.
    SleepSent {
        /// PC name.
        pc: String,
        /// PC address.
        host: String,
    },
    /// The shutdown command was accepted.
    ShutdownSent {
        /// PC name.
        pc: String,
        /// PC address.
        host: String,
    },
    /// A power command failed.
    PowerFailed {
        /// PC name.
        pc: String,
        /// Reason.
        detail: String,
    },
    /// A power command was requested for a PC that is not online.
    PowerUnavailable {
        /// PC name.
        pc: String,
    },
}

impl Notice {
    /// Every key, one per variant, for tests and tooling.
    pub const KEYS: [&'static str; 11] = [
        "notify.tunnel_failed",
        "notify.tunnel_lost",
        "notify.tunnel_timeout",
        "notify.wake_failed",
        "notify.pc_online",
        "notify.pc_timeout",
        "notify.pc_lost",
        "notify.sleep_sent",
        "notify.shutdown_sent",
        "notify.power_failed",
        "notify.power_unavailable",
    ];

    /// Prefix of the pair of locale keys `<key>.title` and `<key>.body`.
    pub fn key(&self) -> &'static str {
        match self {
            Self::TunnelFailed { .. } => "notify.tunnel_failed",
            Self::TunnelLost { .. } => "notify.tunnel_lost",
            Self::TunnelTimeout { .. } => "notify.tunnel_timeout",
            Self::WakeFailed { .. } => "notify.wake_failed",
            Self::PcOnline { .. } => "notify.pc_online",
            Self::PcTimeout { .. } => "notify.pc_timeout",
            Self::PcLost { .. } => "notify.pc_lost",
            Self::SleepSent { .. } => "notify.sleep_sent",
            Self::ShutdownSent { .. } => "notify.shutdown_sent",
            Self::PowerFailed { .. } => "notify.power_failed",
            Self::PowerUnavailable { .. } => "notify.power_unavailable",
        }
    }

    /// Values for the placeholders of the texts. `detail` is cut to [`DETAIL_LIMIT`].
    pub fn params(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::TunnelFailed { tunnel, detail } => {
                vec![("tunnel", tunnel.clone()), ("detail", limit_detail(detail))]
            }
            Self::TunnelLost { tunnel } | Self::TunnelTimeout { tunnel } => {
                vec![("tunnel", tunnel.clone())]
            }
            Self::WakeFailed { pc, detail } | Self::PowerFailed { pc, detail } => {
                vec![("pc", pc.clone()), ("detail", limit_detail(detail))]
            }
            Self::PcOnline { pc, host }
            | Self::SleepSent { pc, host }
            | Self::ShutdownSent { pc, host } => {
                vec![("pc", pc.clone()), ("host", host.clone())]
            }
            Self::PcTimeout { pc } | Self::PcLost { pc } | Self::PowerUnavailable { pc } => {
                vec![("pc", pc.clone())]
            }
        }
    }
}

/// Cuts `text` to [`DETAIL_LIMIT`] characters (never inside a character).
pub fn limit_detail(text: &str) -> String {
    text.chars().take(DETAIL_LIMIT).collect()
}

/// One notice of every variant, for tests.
#[cfg(test)]
pub(crate) fn samples() -> Vec<Notice> {
    let s = |x: &str| x.to_owned();
    vec![
        Notice::TunnelFailed {
            tunnel: s("t"),
            detail: s("d"),
        },
        Notice::TunnelLost { tunnel: s("t") },
        Notice::TunnelTimeout { tunnel: s("t") },
        Notice::WakeFailed {
            pc: s("p"),
            detail: s("d"),
        },
        Notice::PcOnline {
            pc: s("p"),
            host: s("h"),
        },
        Notice::PcTimeout { pc: s("p") },
        Notice::PcLost { pc: s("p") },
        Notice::SleepSent {
            pc: s("p"),
            host: s("h"),
        },
        Notice::ShutdownSent {
            pc: s("p"),
            host: s("h"),
        },
        Notice::PowerFailed {
            pc: s("p"),
            detail: s("d"),
        },
        Notice::PowerUnavailable { pc: s("p") },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_list_matches_variants() {
        let keys: Vec<&str> = samples().iter().map(Notice::key).collect();
        assert_eq!(keys, Notice::KEYS);
    }

    #[test]
    fn detail_is_cut_on_a_character_boundary() {
        let long = "є".repeat(DETAIL_LIMIT + 50);
        let cut = limit_detail(&long);
        assert_eq!(cut.chars().count(), DETAIL_LIMIT);
        let notice = Notice::PowerFailed {
            pc: "p".into(),
            detail: long,
        };
        let detail = notice
            .params()
            .into_iter()
            .find(|(k, _)| *k == "detail")
            .unwrap()
            .1;
        assert_eq!(detail.chars().count(), DETAIL_LIMIT);
    }

    #[test]
    fn short_detail_is_kept() {
        assert_eq!(limit_detail("boom"), "boom");
    }
}
