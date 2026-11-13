//! The portable adapters against local stand-ins: listeners, UDP sockets, `sh` and a fake
//! `ssh` script. No external network; every test is bounded to a fraction of a second or two.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io::Read;
use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use homelink_core::adapters::command::{CommandWake, StdCommandRunner};
use homelink_core::adapters::default_wake_registry;
use homelink_core::adapters::magic::{MAGIC_PACKET_LEN, MagicPacketWake, magic_packet};
use homelink_core::adapters::power::CommandPowerRunner;
use homelink_core::adapters::tcp::{TcpPortProbe, all_open};
use homelink_core::domain::{Host, MacAddr, PowerAction, WakeMethod};
use homelink_core::ports::{
    CommandOutcome, CommandRunner, PortProbe, PowerError, PowerRunner, WakeError, WakeRequest,
    WakeSender,
};
use homelink_core::testing::{multi_config, pc_id, relay_config};
use homelink_core::wake::{RelayWake, relay_ssh_argv};

const SECOND: Duration = Duration::from_secs(1);

fn temp_dir(test: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "homelink-adapters-{}-{test}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

// ======================================================================================
// TCP port probe
// ======================================================================================

fn listen() -> (TcpListener, u16) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// A port on which nothing listens.
fn closed_port() -> u16 {
    let (listener, port) = listen();
    drop(listener);
    port
}

#[test]
fn every_open_port_means_ready() {
    let listeners: Vec<_> = (0..3).map(|_| listen()).collect();
    let ports: Vec<u16> = listeners.iter().map(|(_, p)| *p).collect();
    assert!(all_open("127.0.0.1", &ports, 5 * SECOND));
    let host = Host::parse("127.0.0.1").unwrap();
    assert!(TcpPortProbe.all_open(&host, &ports, 5 * SECOND));
}

#[test]
fn one_closed_port_means_not_ready() {
    let (_open, open) = listen();
    assert!(!all_open("127.0.0.1", &[open, closed_port()], 5 * SECOND));
    assert!(!all_open("127.0.0.1", &[closed_port()], 5 * SECOND));
}

#[test]
fn many_closed_ports_do_not_cost_one_timeout_each() {
    let ports: Vec<u16> = (0..40).map(|_| closed_port()).collect();
    let start = Instant::now();
    assert!(!all_open("127.0.0.1", &ports, 5 * SECOND));
    assert!(start.elapsed() < 2 * SECOND, "took {:?}", start.elapsed());
}

#[test]
fn no_ports_means_nothing_is_closed() {
    assert!(all_open("127.0.0.1", &[], SECOND));
}

#[test]
fn a_host_name_is_resolved() {
    let (_listener, port) = listen();
    assert!(all_open("localhost", &[port], 5 * SECOND));
}

#[test]
fn a_name_that_does_not_resolve_is_not_ready_within_the_timeout() {
    let start = Instant::now();
    assert!(!all_open(
        "no-such-host.invalid",
        &[22],
        Duration::from_millis(500)
    ));
    assert!(start.elapsed() < 3 * SECOND, "took {:?}", start.elapsed());
}

#[test]
fn the_probe_connects_and_closes_without_sending_anything() {
    let (listener, port) = listen();
    let reader = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = Vec::new();
        stream.read_to_end(&mut received).unwrap(); // returns when the probe closes
        received
    });
    assert!(all_open("127.0.0.1", &[port], 5 * SECOND));
    assert!(reader.join().unwrap().is_empty());
}

// ======================================================================================
// Magic packet
// ======================================================================================

fn receiver() -> (UdpSocket, u16) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.set_read_timeout(Some(2 * SECOND)).unwrap();
    let port = socket.local_addr().unwrap().port();
    (socket, port)
}

fn magic_request(address: Ipv4Addr, port: u16) -> (homelink_core::domain::Pc, WakeMethod) {
    let config = multi_config();
    let pc = config.pc(&pc_id("nas")).unwrap().clone();
    let mac = MacAddr::parse("AA:BB:CC:DD:EE:FF").unwrap();
    (pc, WakeMethod::MagicPacket { mac, address, port })
}

#[test]
fn the_magic_packet_arrives_over_udp_unicast() {
    let (socket, port) = receiver();
    let (pc, method) = magic_request(Ipv4Addr::LOCALHOST, port);
    MagicPacketWake
        .wake(&WakeRequest {
            pc: &pc,
            method: &method,
            relay: None,
        })
        .unwrap();
    let mut buffer = [0u8; 256];
    let (n, _) = socket.recv_from(&mut buffer).unwrap();
    assert_eq!(n, MAGIC_PACKET_LEN);
    let mac = MacAddr::parse("AA:BB:CC:DD:EE:FF").unwrap();
    assert_eq!(&buffer[..n], &magic_packet(&mac)[..]);
}

#[test]
fn a_broadcast_destination_is_sent_with_so_broadcast_when_the_system_needs_it() {
    // 255.255.255.255 needs SO_BROADCAST. Where the sandbox has no route at all the send
    // fails for that reason alone, which is not what is tested here.
    let (pc, method) = magic_request(Ipv4Addr::BROADCAST, 9);
    let result = MagicPacketWake.wake(&WakeRequest {
        pc: &pc,
        method: &method,
        relay: None,
    });
    if let Err(WakeError::Failed(reason)) = &result {
        assert!(
            !reason.to_lowercase().contains("permission"),
            "the send was refused as a broadcast without SO_BROADCAST: {reason}"
        );
    }
}

#[test]
fn the_magic_sender_refuses_other_methods() {
    let config = multi_config();
    let pc = config.pc(&pc_id("laptop")).unwrap();
    let method = config
        .pc(&pc_id("office-pc"))
        .unwrap()
        .wake
        .as_ref()
        .unwrap();
    assert_eq!(
        MagicPacketWake.wake(&WakeRequest {
            pc,
            method,
            relay: None
        }),
        Err(WakeError::Unsupported)
    );
}

// ======================================================================================
// Command runner (Unix: `sh`, `sleep`, `true`, `false`)
// ======================================================================================

#[cfg(unix)]
mod commands {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn run(items: &[&str]) -> homelink_core::ports::CommandOutput {
        StdCommandRunner::new().run(&argv(items), 5 * SECOND)
    }

    fn sh(script: &str) -> homelink_core::ports::CommandOutput {
        run(&["sh", "-c", script])
    }

    /// Whether a process with this id is still alive.
    fn alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn wait_until_dead(pid: &str) -> bool {
        let deadline = Instant::now() + 3 * SECOND;
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn exit_codes_are_reported() {
        assert_eq!(run(&["true"]).outcome, CommandOutcome::Exited(0));
        assert_eq!(run(&["false"]).outcome, CommandOutcome::Exited(1));
        assert_eq!(sh("exit 42").outcome, CommandOutcome::Exited(42));
        assert!(run(&["true"]).success() && !run(&["false"]).success());
    }

    #[test]
    fn a_program_killed_by_a_signal_has_the_shell_convention_code() {
        assert_eq!(sh("kill -9 $$").outcome, CommandOutcome::Exited(137));
    }

    #[test]
    fn stderr_then_stdout_are_captured_and_trimmed() {
        let out = sh("echo out; echo err >&2; exit 3");
        assert_eq!(out.outcome, CommandOutcome::Exited(3));
        assert_eq!(out.text, "err\nout");
    }

    #[test]
    fn no_shell_is_involved_in_the_arguments() {
        let out = run(&[
            "printf",
            "%s",
            "a; echo hacked && $(id) `id` | cat > /dev/null",
        ]);
        assert_eq!(out.text, "a; echo hacked && $(id) `id` | cat > /dev/null");
    }

    #[test]
    fn standard_input_is_empty() {
        let start = Instant::now();
        assert_eq!(run(&["cat"]).outcome, CommandOutcome::Exited(0));
        assert!(start.elapsed() < 2 * SECOND);
    }

    #[test]
    fn a_missing_program_and_an_empty_command_do_not_start() {
        let out = run(&["definitely-not-a-program-homelink"]);
        assert_eq!(out.outcome, CommandOutcome::NotStarted);
        assert!(
            out.text.contains("definitely-not-a-program-homelink"),
            "{}",
            out.text
        );
        assert_eq!(
            StdCommandRunner::new().run(&[], SECOND).outcome,
            CommandOutcome::NotStarted
        );
        assert_eq!(
            StdCommandRunner::new().run(&argv(&[""]), SECOND).outcome,
            CommandOutcome::NotStarted
        );
    }

    #[test]
    fn a_program_that_takes_too_long_is_killed() {
        let start = Instant::now();
        let out = StdCommandRunner::new().run(&argv(&["sleep", "30"]), Duration::from_millis(300));
        assert_eq!(out.outcome, CommandOutcome::TimedOut);
        assert!(
            out.text.starts_with("timed out after 0.3 s"),
            "{}",
            out.text
        );
        assert!(start.elapsed() < 3 * SECOND, "took {:?}", start.elapsed());
    }

    #[test]
    fn the_killed_program_is_really_gone() {
        let dir = temp_dir("kill");
        let pidfile = dir.join("pid");
        let script = format!("echo $$ > '{}'; exec sleep 30", pidfile.display());
        let out =
            StdCommandRunner::new().run(&argv(&["sh", "-c", &script]), Duration::from_millis(500));
        assert_eq!(out.outcome, CommandOutcome::TimedOut);
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        assert!(wait_until_dead(pid.trim()), "process {pid} is still alive");
    }

    #[test]
    fn what_the_program_started_is_killed_with_it() {
        let dir = temp_dir("group");
        let pidfile = dir.join("child-pid");
        // A background child in the same process group; the shell waits for it.
        let script = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
        let out =
            StdCommandRunner::new().run(&argv(&["sh", "-c", &script]), Duration::from_millis(500));
        assert_eq!(out.outcome, CommandOutcome::TimedOut);
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        assert!(
            wait_until_dead(pid.trim()),
            "the child {pid} survived the kill"
        );
    }

    #[test]
    fn output_written_before_the_timeout_is_kept_in_the_reason() {
        let out = StdCommandRunner::new().run(
            &argv(&["sh", "-c", "echo partial; exec sleep 30"]),
            Duration::from_millis(500),
        );
        assert_eq!(out.outcome, CommandOutcome::TimedOut);
        assert!(out.text.contains("partial"), "{}", out.text);
    }

    #[test]
    fn output_is_bounded_and_the_program_is_not_blocked_by_it() {
        let runner = StdCommandRunner::new().with_max_output(1000);
        let start = Instant::now();
        // About 2 MB on each stream: far more than a pipe buffer.
        let out = runner.run(
            &argv(&["sh", "-c", "head -c 2000000 /dev/zero | tr '\\0' x; head -c 2000000 /dev/zero | tr '\\0' y >&2"]),
            10 * SECOND,
        );
        assert_eq!(out.outcome, CommandOutcome::Exited(0));
        assert!(out.text.len() < 2200, "kept {} bytes", out.text.len());
        assert!(out.text.contains("truncated"));
        assert_eq!(out.text.matches('x').count(), 1000);
        assert_eq!(out.text.matches('y').count(), 1000);
        assert!(start.elapsed() < 5 * SECOND);
    }

    #[test]
    fn the_decoder_is_pluggable() {
        let runner = StdCommandRunner::new().with_decoder(Arc::new(|bytes: &[u8]| {
            format!("decoded {} bytes", bytes.len())
        }));
        let out = runner.run(&argv(&["printf", "%s", "hello"]), 5 * SECOND);
        assert_eq!(out.text, "decoded 5 bytes");
    }

    #[test]
    fn invalid_utf8_is_replaced_by_default() {
        let out = run(&["printf", "a\\377b"]);
        assert!(
            out.text.starts_with('a') && out.text.ends_with('b'),
            "{}",
            out.text
        );
        assert!(out.text.contains('\u{fffd}'));
    }

    #[test]
    fn bare_names_are_looked_up_only_in_the_program_directory_when_one_is_set() {
        let dir = temp_dir("lookup");
        let script = dir.join("tool");
        std::fs::write(&script, "#!/bin/sh\necho from-the-directory\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = StdCommandRunner::new().with_program_dir(&dir);
        assert_eq!(
            runner.run(&argv(&["tool"]), 5 * SECOND).text,
            "from-the-directory"
        );
        // `true` exists on PATH but not in the directory.
        let out = runner.run(&argv(&["true"]), 5 * SECOND);
        assert_eq!(out.outcome, CommandOutcome::NotStarted);
        assert!(out.text.contains("not found in"), "{}", out.text);
        // An explicit path is used as it is.
        let explicit = script.display().to_string();
        assert_eq!(
            runner.run(&[explicit], 5 * SECOND).text,
            "from-the-directory"
        );
    }

    // ----------------------------------------------------------------------------------
    // A fake `ssh` (and other tools) end to end
    // ----------------------------------------------------------------------------------

    /// Installs an executable `name` in `dir` that records its arguments, one per line, in
    /// `<dir>/<name>.argv`, prints `stdout` and `stderr` and exits with `code`.
    fn fake_tool(dir: &Path, name: &str, code: i32, stdout: &str, stderr: &str) -> PathBuf {
        let log = dir.join(format!("{name}.argv"));
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' '{stdout}'\nprintf '%s' '{stderr}' >&2\nexit {code}\n",
            log.display()
        );
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        log
    }

    fn recorded(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn runner_in(dir: &Path) -> Arc<StdCommandRunner> {
        Arc::new(StdCommandRunner::new().with_program_dir(dir))
    }

    fn relay_request<'a>(config: &'a homelink_core::config::Config, pc: &str) -> WakeRequest<'a> {
        let pc = config.pc(&pc_id(pc)).unwrap();
        let method = pc.wake.as_ref().unwrap();
        let relay = match method {
            WakeMethod::Relay { relay, .. } => config.relay(relay),
            _ => None,
        };
        WakeRequest { pc, method, relay }
    }

    #[test]
    fn a_relay_wake_runs_ssh_with_the_exact_argument_list() {
        let dir = temp_dir("relay");
        let log = fake_tool(&dir, "ssh", 0, "", "");
        let config = relay_config();
        let request = relay_request(&config, "desktop");
        RelayWake::new(runner_in(&dir)).wake(&request).unwrap();

        let command = "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge";
        let expected = relay_ssh_argv(request.relay.unwrap(), command);
        assert_eq!(
            recorded(&log),
            expected[1..],
            "everything after the program name"
        );
    }

    #[test]
    fn every_relay_preset_reaches_ssh_with_its_own_command() {
        let config = relay_config();
        let cases = [
            (
                "desktop",
                "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge",
            ),
            ("media", "etherwake -i br-lan 00:11:22:33:44:55"),
            ("kiosk", "sudo -n wakeonlan -i 10.0.5.255 00:11:22:33:44:66"),
        ];
        for (pc, command) in cases {
            let dir = temp_dir(pc);
            let log = fake_tool(&dir, "ssh", 0, "", "");
            RelayWake::new(runner_in(&dir))
                .wake(&relay_request(&config, pc))
                .unwrap();
            assert_eq!(
                recorded(&log).last().map(String::as_str),
                Some(command),
                "{pc}"
            );
        }
    }

    #[test]
    fn ssh_failures_are_told_apart() {
        let config = relay_config();
        let request = relay_request(&config, "desktop");
        let wake = |dir: &Path| RelayWake::new(runner_in(dir)).wake(&request).unwrap_err();

        let dir = temp_dir("ssh-255");
        fake_tool(&dir, "ssh", 255, "", "Permission denied (publickey).");
        assert_eq!(
            wake(&dir),
            WakeError::Failed(
                "ssh could not connect or authenticate: Permission denied (publickey).".into()
            )
        );

        let dir = temp_dir("ssh-1");
        fake_tool(&dir, "ssh", 1, "", "bad command");
        assert_eq!(
            wake(&dir),
            WakeError::Failed(
                "the command on the remote host failed (exit code 1): bad command".into()
            )
        );

        let dir = temp_dir("ssh-missing"); // no ssh in the directory
        let WakeError::Failed(reason) = wake(&dir) else {
            panic!("expected a failure")
        };
        assert!(
            reason.starts_with("cannot run ssh (is the OpenSSH client installed?)"),
            "{reason}"
        );
    }

    #[test]
    fn a_command_wake_expands_its_placeholders_and_runs_the_program() {
        let dir = temp_dir("command-wake");
        let log = fake_tool(&dir, "wakeonlan", 0, "", "");
        let config = multi_config();
        let request = relay_request(&config, "office-pc"); // command = ["wakeonlan", "office"]
        CommandWake::new(runner_in(&dir)).wake(&request).unwrap();
        assert_eq!(recorded(&log), ["office"]);

        let dir = temp_dir("command-wake-fail");
        fake_tool(&dir, "wakeonlan", 2, "", "no such interface");
        let err = CommandWake::new(runner_in(&dir))
            .wake(&request)
            .unwrap_err();
        assert_eq!(
            err,
            WakeError::Failed("the command failed (exit code 2): no such interface".into())
        );
    }

    #[test]
    fn power_over_ssh_runs_the_preset_for_the_pcs_os() {
        let config = relay_config();
        let desktop = config.pc(&pc_id("desktop")).unwrap(); // Linux, ssh
        for (action, command) in [
            (PowerAction::Sleep, "sudo systemctl suspend -i"),
            (PowerAction::Shutdown, "sudo systemctl poweroff"),
        ] {
            let dir = temp_dir("power");
            let log = fake_tool(&dir, "ssh", 0, "", "");
            CommandPowerRunner::new(runner_in(&dir))
                .run(desktop, action)
                .unwrap();
            let args = recorded(&log);
            assert_eq!(args.last().map(String::as_str), Some(command));
            assert!(args.contains(&"IdentitiesOnly=yes".to_owned()));
            assert!(args.contains(&"192.168.88.10".to_owned()));
        }
    }

    #[test]
    fn power_failures_are_told_apart() {
        let config = relay_config();
        let desktop = config.pc(&pc_id("desktop")).unwrap();
        let run = |dir: &Path| {
            CommandPowerRunner::new(runner_in(dir))
                .run(desktop, PowerAction::Sleep)
                .unwrap_err()
        };
        let dir = temp_dir("power-255");
        fake_tool(&dir, "ssh", 255, "", "Connection refused");
        assert_eq!(
            run(&dir),
            PowerError("ssh could not connect or authenticate: Connection refused".into())
        );
        let dir = temp_dir("power-1");
        fake_tool(&dir, "ssh", 1, "", "sudo: a password is required");
        assert_eq!(
            run(&dir),
            PowerError(
                "the command on the remote host failed (exit code 1): sudo: a password is required"
                    .into()
            )
        );
        let dir = temp_dir("power-missing");
        assert!(run(&dir).0.starts_with("cannot run ssh"));
    }

    #[test]
    fn power_by_command_runs_the_configured_command() {
        let config = multi_config();
        let laptop = config.pc(&pc_id("laptop")).unwrap(); // only a shutdown command
        let dir = temp_dir("power-command");
        let log = fake_tool(&dir, "ssh", 0, "", "");
        let power = CommandPowerRunner::new(runner_in(&dir));
        power.run(laptop, PowerAction::Shutdown).unwrap();
        assert_eq!(
            recorded(&log),
            ["alex@192.168.88.11", "shutdown", "/s", "/t", "0"]
        );
        // No sleep command is configured: nothing is run.
        let err = power.run(laptop, PowerAction::Sleep).unwrap_err();
        assert!(err.0.contains("no Sleep command"), "{}", err.0);
    }

    #[test]
    fn a_pc_without_a_power_method_cannot_be_powered() {
        let config = multi_config();
        let office = config.pc(&pc_id("office-pc")).unwrap();
        let power = CommandPowerRunner::new(Arc::new(StdCommandRunner::new()));
        assert!(power.run(office, PowerAction::Sleep).is_err());
    }

    #[test]
    fn the_default_registry_has_a_working_sender_for_every_method() {
        let config = multi_config();
        let dir = temp_dir("registry");
        fake_tool(&dir, "ssh", 0, "", "");
        let log = fake_tool(&dir, "wakeonlan", 0, "", "");
        let registry = default_wake_registry(runner_in(&dir));
        // Command method.
        registry
            .send(&config, config.pc(&pc_id("office-pc")).unwrap())
            .unwrap();
        assert_eq!(recorded(&log), ["office"]);
        // Relay method.
        let relays = relay_config();
        registry
            .send(&relays, relays.pc(&pc_id("desktop")).unwrap())
            .unwrap();
        // Magic packet: to a local socket.
        let (socket, port) = receiver();
        let mut pc = config.pc(&pc_id("nas")).unwrap().clone();
        pc.wake = Some(WakeMethod::MagicPacket {
            mac: MacAddr::parse("AA:BB:CC:DD:EE:FF").unwrap(),
            address: Ipv4Addr::LOCALHOST,
            port,
        });
        registry.send(&config, &pc).unwrap();
        let mut buffer = [0u8; 256];
        assert_eq!(socket.recv_from(&mut buffer).unwrap().0, MAGIC_PACKET_LEN);
    }
}
