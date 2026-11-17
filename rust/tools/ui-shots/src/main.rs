//! Renders windows of HomeLink without a display, to PNG files (probe for layout checks).
use std::cell::RefCell;
use std::rc::Rc;

use homelink_core::domain::{Os, RelayKind};
use homelink_core::i18n::{Lang, Translator};
use homelink_core::{pc_setup, relay_setup};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

slint::include_modules!();

// Copied from rust/app/src/manage.rs by run.sh.
include!("words.rs");

struct Headless(RefCell<Vec<Rc<MinimalSoftwareWindow>>>);
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(w.clone());
        Ok(w)
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn png(path: &str, w: u32, h: u32, rgb: &[u8]) {
    let mut raw = Vec::new();
    for y in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgb[y * w as usize * 3..(y + 1) * w as usize * 3]);
    }
    let mut z = vec![0x78, 0x01];
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, c) in chunks.iter().enumerate() {
        z.push(u8::from(i + 1 == chunks.len()));
        let len = c.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, out).unwrap();
}

fn shot(win: &Rc<MinimalSoftwareWindow>, w: u32, h: u32, path: &str) {
    win.set_size(slint::PhysicalSize::new(w, h));
    let mut buf = vec![slint::Rgb8Pixel::new(0, 0, 0); (w * h) as usize];
    win.request_redraw();
    win.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let rgb: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    png(path, w, h, &rgb);
}

fn model(v: Vec<SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(v))
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let windows = Rc::new(Headless(RefCell::new(Vec::new())));
    let list = windows.clone();
    struct P(Rc<Headless>);
    impl Platform for P {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            self.0.create_window_adapter()
        }
    }
    slint::platform::set_platform(Box::new(P(windows))).unwrap();
    let tr = Translator::new(Lang::En);
    let wiz = WizardWindow::new().unwrap();
    wiz.global::<Theme>().set_dark(true);
    fill_words(&wiz.global::<Words>(), &tr);
    wiz.set_heading("Edit Desktop".into());
    wiz.show().unwrap();
    let win = list.0.borrow().last().unwrap().clone();
    let key =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJSldq3cwkmqFSjBv5FC58EviGCezzGKShe8j//eD0CM homelink";

    // Step 3: existing RouterOS relay.
    wiz.set_step(3);
    wiz.set_step_text("Step 3 of 5 · Wake".into());
    wiz.set_wake_index(1);
    wiz.set_relay_labels(model(vec![
        "10.10.1.1 (RouterOS)".into(),
        "A new relay host".into(),
    ]));
    wiz.set_relay_kinds(ModelRc::new(VecModel::from(vec![0])));
    wiz.set_relay_index(0);
    wiz.set_mac("24:4B:FE:5C:E2:4B".into());
    wiz.set_interface("TrustVlan10".into());
    wiz.set_relay_setup_intro(tr.t("wizard.relay.setup.routeros", &[]).into());
    wiz.set_relay_setup(
        relay_setup::text(&relay_setup::RelaySetup {
            kind: RelayKind::RouterOs,
            user: "wol-bot",
            public_key: Some(key),
            client_address: None,
            sudo: Some(false),
        })
        .into(),
    );
    wiz.set_errors(model(vec![SharedString::default(); 30]));
    for (name, scroll) in [("step3-top", false), ("step3-bottom", true)] {
        shot(&win, 420, 430, "/dev/null");
        let (c, v) = (wiz.get_dbg_content(), wiz.get_dbg_view());
        if scroll {
            wiz.set_dbg_scroll((c - v).max(0.0));
        } else {
            wiz.set_dbg_scroll(0.0);
        }
        shot(&win, 420, 430, &format!("{out}/{name}.png"));
        println!("{name}: content={c} view={v}");
    }
    // The same at a very tall window: everything is visible, the real height of the content.
    wiz.set_dbg_scroll(0.0);
    shot(&win, 420, 1400, &format!("{out}/step3-tall.png"));

    // Step 4: SSH power for Linux.
    wiz.set_step(4);
    wiz.set_step_text("Step 4 of 5 · Sleep and shutdown".into());
    wiz.set_allow_power(true);
    wiz.set_allow_rdp(true);
    wiz.set_power_index(0);
    wiz.set_os_index(1);
    wiz.set_ssh_user("alex".into());
    wiz.set_ssh_port("22".into());
    wiz.set_ssh_identity(r"C:\Program Files\HomeLink\keys\homelink_ed25519_2".into());
    wiz.set_power_public_key(key.into());
    wiz.set_pc_setup_intro(tr.t("wizard.pc.setup.linux", &[]).into());
    wiz.set_pc_setup(
        pc_setup::text(&pc_setup::PcSetup {
            os: Os::Linux,
            user: "alex",
            public_key: Some(key),
        })
        .into(),
    );
    wiz.set_rdp_user("alex".into());
    for (name, scroll) in [("step4-top", false), ("step4-bottom", true)] {
        wiz.set_dbg_scroll(0.0);
        shot(&win, 420, 430, "/dev/null");
        let (c, v) = (wiz.get_dbg_content(), wiz.get_dbg_view());
        if scroll {
            wiz.set_dbg_scroll((c - v).max(0.0));
        }
        shot(&win, 420, 430, &format!("{out}/{name}.png"));
        println!("{name}: content={c} view={v}");
    }
    wiz.set_dbg_scroll(0.0);
    shot(&win, 420, 2000, &format!("{out}/step4-tall.png"));

    // Step 5: checks, a host key offer, the neighbor row with "use these values".
    wiz.set_step(5);
    wiz.set_step_text("Step 5 of 5 · Check".into());
    let line = |name: &str, result: &str, mark: Mark, trust: bool, fix: bool| CheckLine {
        name: name.into(),
        result: result.into(),
        mark,
        trust,
        fix,
    };
    wiz.set_checks(ModelRc::new(VecModel::from(vec![
        line(
            "Readiness ports",
            "closed: 22, 3389",
            Mark::Fail,
            false,
            false,
        ),
        line(
            "SSH to the relay host",
            "host key not known yet",
            Mark::Fail,
            true,
            false,
        ),
        line("SSH to the PC", "works", Mark::Ok, false, false),
        line(
            "PC in the router's network",
            &tr.t(
                "check.result.neighbor_differs",
                &[("interface", "TrustVlan10"), ("mac", "24:4B:FE:5C:E2:4B")],
            ),
            Mark::Fail,
            false,
            true,
        ),
    ])));
    wiz.set_trust_prompt(
        tr.t("wizard.ssh.host_key_unknown", &[("host", "10.10.1.1")])
            .into(),
    );
    wiz.set_trust_fingerprints(model(vec![
        tr.t(
            "wizard.ssh.fingerprint",
            &[(
                "fingerprint",
                "SHA256:Epe5jl1Pg9mhFUf7f+1oduOXwFVzIMyCfS4i4WgTV9c (RSA)",
            )],
        )
        .into(),
    ]));
    for (name, scroll) in [("step5-top", false), ("step5-bottom", true)] {
        wiz.set_dbg_scroll(0.0);
        shot(&win, 420, 430, "/dev/null");
        let (c, v) = (wiz.get_dbg_content(), wiz.get_dbg_view());
        if scroll {
            wiz.set_dbg_scroll((c - v).max(0.0));
        }
        shot(&win, 420, 430, &format!("{out}/{name}.png"));
        println!("{name}: content={c} view={v}");
    }

    // The settings window.
    for dark in [true, false] {
        let set = SettingsWindow::new().unwrap();
        set.global::<Theme>().set_dark(dark);
        fill_words(&set.global::<Words>(), &tr);
        set.show().unwrap();
        let sw = list.0.borrow().last().unwrap().clone();
        let row = |id: &str, name: &str, sec: &str, armed: bool| SettingsRow {
            id: id.into(),
            name: name.into(),
            secondary: sec.into(),
            armed,
        };
        set.set_tunnels(ModelRc::new(VecModel::from(vec![
            row("home", "Home", "1 PC", false),
            row("vic-wg", "vic-wg", "2 PCs", false),
        ])));
        set.set_pcs(ModelRc::new(VecModel::from(vec![
            row("desktop", "Desktop", "Home", false),
            row("nas", "NAS", "no tunnel", true),
        ])));
        let suffix = if dark { "dark" } else { "light" };
        let languages = |tr: &Translator| -> ModelRc<SharedString> {
            model(
                homelink_core::settings::language_choices()
                    .iter()
                    .map(|c| tr.language_name(*c).into())
                    .collect(),
            )
        };
        set.set_language_choices(languages(&tr));
        set.set_language_index(0);
        set.set_page(0);
        shot(&sw, 900, 480, &format!("{out}/settings-list-{suffix}.png"));
        if dark {
            // The list in Ukrainian, with "УКР" chosen.
            let uk = Translator::new(Lang::Uk);
            fill_words(&set.global::<Words>(), &uk);
            set.set_language_choices(languages(&uk));
            set.set_language_index(1);
            shot(&sw, 900, 480, &format!("{out}/settings-list-uk.png"));
            fill_words(&set.global::<Words>(), &tr);
            set.set_language_choices(languages(&tr));
            set.set_language_index(0);
            set.set_version_text("Version 1.0.0-rc.10".into());
            set.set_slint_version("Slint 1.18.1".into());
            set.set_slint_url("https://slint.dev".into());
            set.set_page(1);
            shot(&sw, 900, 480, &format!("{out}/settings-help.png"));
            // The capabilities page with the paths of a Windows machine (from the check of 16.11).
            let mut caps = homelink_core::capabilities::Capabilities::from_statuses(
                homelink_core::capabilities::Tool::ALL.map(|tool| {
                    let path = match tool {
                        homelink_core::capabilities::Tool::WireGuard => {
                            "C:/Program Files/WireGuard/wireguard.exe".to_owned()
                        }
                        homelink_core::capabilities::Tool::Wg => {
                            "C:/Program Files/WireGuard/wg.exe".to_owned()
                        }
                        homelink_core::capabilities::Tool::Ssh
                        | homelink_core::capabilities::Tool::SshKeygen
                        | homelink_core::capabilities::Tool::SshKeyscan => {
                            format!("C:/Windows/System32/OpenSSH/{}.exe", tool.program())
                        }
                        homelink_core::capabilities::Tool::PowerShell => {
                            "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe".to_owned()
                        }
                        other => format!("C:/Windows/System32/{}.exe", other.program()),
                    };
                    (
                        tool,
                        homelink_core::capabilities::ToolStatus::Found(path.into()),
                    )
                }),
            );
            for (name, missing) in [("settings-caps", false), ("settings-caps-missing", true)] {
                if missing {
                    caps = homelink_core::capabilities::Capabilities::from_statuses([]);
                }
                let tiles: Vec<CapabilityTile> = homelink_core::capability_page::tiles(&tr, &caps)
                    .into_iter()
                    .map(|tile| CapabilityTile {
                        title: tile.title.into(),
                        found: tile.found,
                        line: tile.line.into(),
                        details: slint::ModelRc::new(slint::VecModel::from(
                            tile.details
                                .into_iter()
                                .map(SharedString::from)
                                .collect::<Vec<_>>(),
                        )),
                        link_label: tile
                            .link
                            .as_ref()
                            .map(|l| l.label.clone())
                            .unwrap_or_default()
                            .into(),
                        link_url: tile.link.map(|l| l.url).unwrap_or_default().into(),
                    })
                    .collect();
                set.set_capability_tiles(ModelRc::new(VecModel::from(tiles)));
                set.set_capabilities_checked("Checked 12:37".into());
                set.set_page(4);
                shot(&sw, 900, 480, &format!("{out}/{name}.png"));
            }
        }
    }

    // The tray panel with a PC whose actions lack OpenSSH, and a tunnel without WireGuard.
    let panel = Panel::new().unwrap();
    panel.global::<Theme>().set_dark(true);
    panel.show().unwrap();
    let pw = list.0.borrow().last().unwrap().clone();
    let tunnel = TunnelRow {
        id: "office".into(),
        name: "Office".into(),
        mark: Mark::Unknown,
        inactive: false,
        toggle_on: false,
        toggle_label: "Raise".into(),
        reason: "WireGuard client not found".into(),
    };
    let pc = PcRow {
        id: "desktop".into(),
        name: "Desktop".into(),
        mark: Mark::Off,
        inactive: false,
        wake_on: false,
        sleep_on: false,
        shutdown_on: false,
        rdp_on: true,
        reason: "Wake, sleep, shut down: OpenSSH client not found".into(),
    };
    let group = GroupRow {
        has_tunnel: true,
        tunnel: tunnel.clone(),
        pcs: ModelRc::new(VecModel::from(vec![pc.clone()])),
    };
    panel.set_groups(ModelRc::new(VecModel::from(vec![group])));
    for (name, expanded) in [("panel-pc", "p:desktop"), ("panel-tunnel", "t:office")] {
        panel.global::<Nav>().set_expanded(expanded.into());
        shot(&pw, 420, 400, "/dev/null");
        let h = panel.get_content_height();
        println!("{name}: content-height={h}");
        shot(&pw, 420, h.ceil() as u32 + 40, &format!("{out}/{name}.png"));
    }

    // The details window of a PC: the ports in their colours.
    let details = DetailsWindow::new().unwrap();
    details.global::<Theme>().set_dark(true);
    details.show().unwrap();
    let dw = list.0.borrow().last().unwrap().clone();
    let plain = |label: &str, value: &str| DetailRow {
        label: label.into(),
        value: value.into(),
        strong: false,
        fail: false,
        ports: ModelRc::default(),
    };
    let mut ports = plain("Ports", "22, 3389");
    ports.ports = ModelRc::new(VecModel::from(vec![
        PortMark {
            port: "22".into(),
            state: 2,
        },
        PortMark {
            port: "3389".into(),
            state: 1,
        },
    ]));
    details.set_name("win10-vm".into());
    details.set_mark(Mark::Off);
    details.set_rows(ModelRc::new(VecModel::from(vec![
        plain("State", "off"),
        plain("Address", "10.10.70.110"),
        plain("Tunnel", "Home"),
        plain("Wake-up", "Relay 10.10.1.1"),
        ports.clone(),
        plain("Last check", "just now"),
        plain("Last error", "none"),
    ])));
    shot(&dw, 300, 260, &format!("{out}/details-ports.png"));
    let mut ports_uk = ports;
    ports_uk.label = "Порти готовності".into();
    details.set_rows(ModelRc::new(VecModel::from(vec![
        plain("Стан", "вимкнено"),
        plain("Адреса", "10.10.70.110"),
        plain("Тунель", "Home"),
        plain("Пробудження", "Ретранслятор 10.10.1.1"),
        ports_uk,
        plain("Остання перевірка", "щойно"),
        plain("Остання помилка", "немає"),
    ])));
    shot(&dw, 300, 260, &format!("{out}/details-ports-uk.png"));
}
