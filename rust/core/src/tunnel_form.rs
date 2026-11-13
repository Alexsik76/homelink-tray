//! The form behind "Додати тунель" and "Змінити" on a tunnel (section 7 of the handoff).
//!
//! The UI binds the text fields of [`TunnelForm`], calls [`TunnelForm::validate`] and shows
//! each [`Problem`] under the field named in [`field`]. The result is a [`Tunnel`]; adding or
//! replacing it in the configuration is `settings::add_tunnel` / `settings::edit_tunnel`.

use crate::config::Config;
use crate::domain::{InvalidValue, Tunnel, TunnelId, TunnelKind};
use crate::forms;
use crate::ids;
use crate::problem::{Code, Problem};

/// Names of the fields in [`Problem::field`].
pub mod field {
    /// Display name.
    pub const NAME: &str = "name";
    /// Path of the WireGuard configuration (`.conf` or `.conf.dpapi`).
    pub const CONF: &str = "conf";
    /// Address that answers ping only while the tunnel works.
    pub const PROBE_HOST: &str = "probe_host";
    /// Path of `wireguard.exe`.
    pub const WIREGUARD_EXE: &str = "wireguard_exe";
    /// Seconds to wait for the probe host after the start.
    pub const TIMEOUT: &str = "timeout";
    /// Milliseconds to wait for one ping.
    pub const PING_TIMEOUT: &str = "ping_timeout";
}

/// Prefill for the path of `wireguard.exe` when the configuration has no tunnel to copy it from.
pub const DEFAULT_WIREGUARD_EXE: &str = r"C:\Program Files\WireGuard\wireguard.exe";
/// Prefill for the start timeout (seconds) when there is no tunnel to copy it from.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
/// Prefill for the ping timeout (milliseconds) when there is no tunnel to copy it from.
pub const DEFAULT_PING_TIMEOUT_MS: u64 = 1000;

/// The tunnel being edited: its id and the name it had when the form was opened.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Original {
    id: TunnelId,
    name: String,
}

/// The fields of the tunnel form, as typed.
///
/// The three advanced fields (`wireguard_exe`, `timeout_seconds`, `ping_timeout_ms`) are
/// prefilled; the handoff shows only the first three, so the window may keep the rest
/// hidden and the prefilled values are used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelForm {
    /// Display name.
    pub name: String,
    /// Path of the WireGuard configuration.
    pub conf: String,
    /// Address that answers ping only while the tunnel works.
    pub probe_host: String,
    /// Path of `wireguard.exe`.
    pub wireguard_exe: String,
    /// Seconds to wait for the probe host after the start.
    pub timeout_seconds: String,
    /// Milliseconds to wait for one ping.
    pub ping_timeout_ms: String,
    original: Option<Original>,
}

impl TunnelForm {
    /// An empty form for a new tunnel; the advanced fields are copied from the first existing
    /// tunnel, or take the `DEFAULT_*` values.
    pub fn new(config: &Config) -> Self {
        let (exe, timeout, ping) = match config.tunnels.first() {
            Some(t) => {
                let TunnelKind::WireGuardService { exe, .. } = &t.kind;
                (
                    exe.clone(),
                    t.timeout.as_secs().to_string(),
                    t.ping_timeout.as_millis().to_string(),
                )
            }
            None => (
                DEFAULT_WIREGUARD_EXE.to_owned(),
                DEFAULT_TIMEOUT_SECONDS.to_string(),
                DEFAULT_PING_TIMEOUT_MS.to_string(),
            ),
        };
        Self {
            name: String::new(),
            conf: String::new(),
            probe_host: String::new(),
            wireguard_exe: exe,
            timeout_seconds: timeout,
            ping_timeout_ms: ping,
            original: None,
        }
    }

    /// A form filled from an existing tunnel ("Змінити").
    pub fn from_tunnel(tunnel: &Tunnel) -> Self {
        let TunnelKind::WireGuardService { exe, conf } = &tunnel.kind;
        Self {
            name: tunnel.name.clone(),
            conf: conf.clone(),
            probe_host: tunnel.probe_host.to_string(),
            wireguard_exe: exe.clone(),
            timeout_seconds: tunnel.timeout.as_secs().to_string(),
            ping_timeout_ms: tunnel.ping_timeout.as_millis().to_string(),
            original: Some(Original {
                id: tunnel.id.clone(),
                name: tunnel.name.clone(),
            }),
        }
    }

    /// The id of the tunnel being edited; `None` for a new one.
    pub fn editing(&self) -> Option<&TunnelId> {
        self.original.as_ref().map(|o| &o.id)
    }

    /// Checks every field with the rules of the configuration file and builds the tunnel.
    ///
    /// The id of a new tunnel is derived from its name (unique among tunnels). An edited
    /// tunnel keeps its id unless its name changed; then the id follows the new name.
    pub fn validate(&self, config: &Config) -> Result<Tunnel, Vec<Problem>> {
        let mut out = Vec::new();
        let own = self.editing();
        let name = forms::required(field::NAME, &self.name, &mut out);
        if let Some(name) = name
            && config
                .tunnels
                .iter()
                .any(|t| Some(&t.id) != own && t.name.trim().to_lowercase() == name.to_lowercase())
        {
            out.push(Problem::new(field::NAME, Code::NameTaken).with("name", name));
        }
        let conf = forms::wireguard_conf(field::CONF, &self.conf, &mut out);
        let probe_host = forms::host(field::PROBE_HOST, &self.probe_host, &mut out);
        let exe = forms::required(field::WIREGUARD_EXE, &self.wireguard_exe, &mut out);
        let timeout = forms::seconds(field::TIMEOUT, &self.timeout_seconds, &mut out);
        let ping_timeout = forms::millis(field::PING_TIMEOUT, &self.ping_timeout_ms, &mut out);
        let (Some(name), Some(conf), Some(probe_host), Some(exe), Some(timeout), Some(ping)) =
            (name, conf, probe_host, exe, timeout, ping_timeout)
        else {
            return Err(out);
        };
        if !out.is_empty() {
            return Err(out);
        }
        let id = match &self.original {
            Some(o) if o.name.trim() == name => o.id.clone(),
            _ => match derive_id(config, name, own) {
                Ok(id) => id,
                Err(_) => return Err(vec![Problem::new(field::NAME, Code::Required)]),
            },
        };
        Ok(Tunnel {
            id,
            name: name.to_owned(),
            kind: TunnelKind::WireGuardService {
                exe: exe.to_owned(),
                conf,
            },
            probe_host,
            timeout,
            ping_timeout: ping,
        })
    }
}

/// The id for a tunnel called `name`, free among the tunnels other than `own`.
fn derive_id(
    config: &Config,
    name: &str,
    own: Option<&TunnelId>,
) -> Result<TunnelId, InvalidValue> {
    // A slug is made of valid id characters, so the parse cannot fail in practice.
    TunnelId::parse(&ids::unique(name, "tunnel", |candidate| {
        config
            .tunnels
            .iter()
            .any(|t| t.id.as_str() == candidate && Some(&t.id) != own)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{multi_config, single_config, tunnel_id};
    use std::time::Duration;

    fn filled(config: &Config) -> TunnelForm {
        let mut form = TunnelForm::new(config);
        form.name = "Lab".into();
        form.conf = r"C:\WG\lab.conf.dpapi".into();
        form.probe_host = "10.9.0.1".into();
        form
    }

    fn codes(problems: &[Problem]) -> Vec<(&'static str, Code)> {
        problems.iter().map(|p| (p.field, p.code)).collect()
    }

    #[test]
    fn a_new_form_copies_the_advanced_fields_from_an_existing_tunnel() {
        let form = TunnelForm::new(&single_config());
        assert_eq!(form.wireguard_exe, r"C:\WG\wireguard.exe");
        assert_eq!(form.timeout_seconds, "20");
        assert_eq!(form.ping_timeout_ms, "1000");
        let empty = Config {
            tunnels: vec![],
            pcs: vec![],
            relays: vec![],
            ..single_config()
        };
        assert_eq!(TunnelForm::new(&empty).wireguard_exe, DEFAULT_WIREGUARD_EXE);
    }

    #[test]
    fn a_valid_form_builds_a_tunnel_with_a_derived_id() {
        let config = single_config();
        let tunnel = filled(&config).validate(&config).unwrap();
        assert_eq!(tunnel.id.as_str(), "lab");
        assert_eq!(tunnel.name, "Lab");
        assert_eq!(tunnel.probe_host.as_str(), "10.9.0.1");
        assert_eq!(tunnel.timeout, Duration::from_secs(20));
    }

    #[test]
    fn the_id_is_made_unique() {
        let config = single_config();
        let mut form = filled(&config);
        form.name = "home".into(); // slug equals the id of "Home", but the name differs by case
        let problems = form.validate(&config).unwrap_err();
        assert_eq!(codes(&problems), [(field::NAME, Code::NameTaken)]);
        form.name = "Home 2".into();
        assert_eq!(form.validate(&config).unwrap().id.as_str(), "home-2");
        form.name = "Дім".into();
        assert_eq!(form.validate(&config).unwrap().id.as_str(), "dim");
    }

    #[test]
    fn every_field_reports_its_own_problem() {
        let config = single_config();
        let mut form = TunnelForm::new(&config);
        form.conf = "x.txt".into();
        form.probe_host = "a b".into();
        form.wireguard_exe = " ".into();
        form.timeout_seconds = "0".into();
        form.ping_timeout_ms = "abc".into();
        let problems = form.validate(&config).unwrap_err();
        assert_eq!(
            codes(&problems),
            [
                (field::NAME, Code::Required),
                (field::CONF, Code::ConfExtension),
                (field::PROBE_HOST, Code::HostInvalid),
                (field::WIREGUARD_EXE, Code::Required),
                (field::TIMEOUT, Code::SecondsRange),
                (field::PING_TIMEOUT, Code::NumberInvalid),
            ]
        );
    }

    #[test]
    fn an_edit_keeps_the_id_unless_the_name_changes() {
        let config = multi_config();
        let home = config.tunnel(&tunnel_id("home")).unwrap();
        let mut form = TunnelForm::from_tunnel(home);
        assert_eq!(form.editing(), Some(&tunnel_id("home")));
        assert_eq!(form.validate(&config).unwrap(), *home);
        form.probe_host = "192.168.88.2".into();
        assert_eq!(form.validate(&config).unwrap().id.as_str(), "home");
        form.name = "Дім".into();
        assert_eq!(form.validate(&config).unwrap().id.as_str(), "dim");
        // Renaming to a name whose slug is the tunnel's own id keeps that id.
        form.name = "HOME!".into();
        assert_eq!(form.validate(&config).unwrap().id.as_str(), "home");
        // Another tunnel's name is taken.
        form.name = "office".into();
        assert!(form.validate(&config).is_err());
    }
}
