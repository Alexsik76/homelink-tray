import pytest

from homelink.controller import DETAIL_LIMIT, Controller
from homelink.router import RouterError
from homelink.state import PcState, Status, TunnelState
from homelink.tunnel import TunnelError


class FakeTunnel:
    def __init__(self, running=False, error=None):
        self.running = running
        self.error = error

    def is_running(self):
        return self.running

    def up(self):
        if self.error:
            raise TunnelError(self.error)
        self.running = True

    def down(self):
        self.running = False


class FakeRouter:
    def __init__(self, error=None):
        self.error = error
        self.calls = []

    def wake(self, mac, interface):
        if self.error:
            raise RouterError(self.error)
        self.calls.append((mac, interface))


class Env:
    """A controller wired to fakes, with a manual clock and a set of reachable hosts."""

    def __init__(self, config, tunnel=None, router=None):
        self.config = config
        self.tunnel = tunnel or FakeTunnel()
        self.router = router or FakeRouter()
        self.reachable = set()
        self.now = 0.0
        self.changes = []
        self.notes = []
        self.commands = []
        self.command_result = (0, "")
        self.controller = Controller(
            config=config,
            tunnel=self.tunnel,
            router=self.router,
            ping=lambda host, _timeout: host in self.reachable,
            ports_open=lambda host, _ports, _timeout: host in self.reachable,
            run_command=self.run_command,
            on_change=self.changes.append,
            on_notify=lambda key, **params: self.notes.append((key, params)),
            clock=lambda: self.now,
            sleep=self.advance,
        )

    def run_command(self, command, timeout):
        self.commands.append((command, timeout))
        return self.command_result

    def advance(self, seconds):
        self.now += seconds

    @property
    def status(self):
        return self.controller.status


@pytest.fixture
def env(config):
    return Env(config)


def test_connect_and_wake_sends_wol_when_pc_is_silent(env):
    env.reachable = {env.config.router_ip}
    env.controller.connect_and_wake()
    assert env.status == Status(TunnelState.UP, PcState.WAKING)
    assert env.router.calls == [(env.config.wol_mac, env.config.wol_interface)]
    assert env.changes[0].tunnel is TunnelState.STARTING


def test_connect_and_wake_skips_wol_when_pc_answers(env):
    env.reachable = {env.config.router_ip, env.config.pc_ip}
    env.controller.connect_and_wake()
    assert env.status == Status(TunnelState.UP, PcState.ONLINE)
    assert env.router.calls == []


def test_tunnel_failure_is_reported(config):
    env = Env(config, tunnel=FakeTunnel(error="access denied"))
    env.controller.connect_and_wake()
    assert env.status == Status(TunnelState.DOWN, PcState.UNKNOWN)
    assert env.notes == [("notify.tunnel_failed", {"detail": "access denied"})]


def test_tunnel_start_gives_up_after_timeout(env):
    env.controller.connect_and_wake()  # the router never answers
    assert env.now >= env.config.tunnel_timeout
    assert env.status == Status(TunnelState.STARTING, PcState.UNKNOWN)
    assert env.router.calls == []


def test_router_failure_is_reported(config):
    env = Env(config, router=FakeRouter(error="HTTP 500"))
    env.reachable = {config.router_ip}
    env.controller.connect_and_wake()
    assert env.status.pc is not PcState.WAKING
    assert env.notes == [("notify.wol_failed", {"detail": "HTTP 500"})]


def test_waking_pc_comes_online(env):
    env.reachable = {env.config.router_ip}
    env.controller.connect_and_wake()
    env.controller.refresh()
    assert env.status.pc is PcState.WAKING
    env.reachable.add(env.config.pc_ip)
    env.controller.refresh()
    assert env.status.pc is PcState.ONLINE
    assert env.notes == [("notify.pc_online", {"host": env.config.pc_ip})]


def test_waking_times_out(env):
    env.reachable = {env.config.router_ip}
    env.controller.connect_and_wake()
    env.advance(env.config.wake_timeout + 1)
    env.controller.refresh()
    assert env.status.pc is PcState.OFFLINE
    assert env.notes == [("notify.pc_timeout", {})]


def test_wake_requires_tunnel(env):
    env.controller.wake()
    assert env.router.calls == []
    assert env.notes == [("notify.tunnel_required", {})]


def test_toggle_tunnel_up_and_down(env):
    env.reachable = {env.config.router_ip}
    env.controller.toggle_tunnel()
    assert env.status == Status(TunnelState.UP, PcState.OFFLINE)
    env.controller.toggle_tunnel()
    assert env.status == Status(TunnelState.DOWN, PcState.UNKNOWN)
    assert not env.tunnel.running


def test_refresh_detects_external_tunnel(config):
    env = Env(config, tunnel=FakeTunnel(running=True))
    env.reachable = {config.router_ip, config.pc_ip}
    env.controller.refresh()
    assert env.status == Status(TunnelState.UP, PcState.ONLINE)
    assert env.notes == []


def test_poll_interval_is_shorter_while_waking(env):
    assert env.controller.poll_interval == env.config.poll_seconds
    env.reachable = {env.config.router_ip}
    env.controller.connect_and_wake()
    assert env.controller.poll_interval == env.config.wake_poll_seconds


def test_single_failed_check_keeps_pc_online(env):
    env.reachable = {env.config.router_ip, env.config.pc_ip}
    env.controller.connect_and_wake()
    env.reachable.discard(env.config.pc_ip)
    for _ in range(env.config.offline_after_failures - 1):
        env.controller.refresh()
        assert env.status.pc is PcState.ONLINE
    env.controller.refresh()
    assert env.status.pc is PcState.OFFLINE


def test_one_successful_check_is_enough_for_online(env):
    env.reachable = {env.config.router_ip, env.config.pc_ip}
    env.controller.connect_and_wake()
    env.reachable.discard(env.config.pc_ip)
    env.controller.refresh()  # one failure, still online
    env.reachable.add(env.config.pc_ip)
    env.controller.refresh()  # the failure counter starts over
    env.reachable.discard(env.config.pc_ip)
    for _ in range(env.config.offline_after_failures - 1):
        env.controller.refresh()
    assert env.status.pc is PcState.ONLINE

    env.controller.refresh()
    assert env.status.pc is PcState.OFFLINE
    env.reachable.add(env.config.pc_ip)
    env.controller.refresh()
    assert env.status.pc is PcState.ONLINE


# ---------- power commands ----------
@pytest.fixture
def online(env):
    env.reachable = {env.config.router_ip, env.config.pc_ip}
    env.controller.connect_and_wake()
    assert env.status == Status(TunnelState.UP, PcState.ONLINE)
    return env


@pytest.mark.parametrize(
    ("action", "command_key", "note"),
    [
        ("shutdown", "shutdown_command", "notify.shutdown_sent"),
        ("suspend", "suspend_command", "notify.suspend_sent"),
    ],
)
def test_power_action_runs_configured_command(online, action, command_key, note):
    getattr(online.controller, action)()
    command = list(getattr(online.config, command_key))
    assert online.commands == [(command, online.config.power_command_timeout)]
    assert online.notes == [(note, {"host": online.config.pc_ip})]


@pytest.mark.parametrize("action", ["shutdown", "suspend"])
def test_power_action_does_not_change_state(online, action):
    changes = list(online.changes)
    getattr(online.controller, action)()
    assert online.status == Status(TunnelState.UP, PcState.ONLINE)
    assert online.changes == changes


@pytest.mark.parametrize("action", ["shutdown", "suspend"])
def test_power_failure_is_reported(online, action):
    online.command_result = (255, "Permission denied")
    getattr(online.controller, action)()
    assert online.notes == [("notify.power_failed", {"detail": "Permission denied"})]


def test_power_failure_detail_is_trimmed(online):
    online.command_result = (1, "x" * (DETAIL_LIMIT + 100))
    online.controller.shutdown()
    assert online.notes == [("notify.power_failed", {"detail": "x" * DETAIL_LIMIT})]


@pytest.mark.parametrize("action", ["shutdown", "suspend"])
def test_power_action_needs_online_pc(env, action):
    env.reachable = {env.config.router_ip}
    env.controller.connect_and_wake()  # the PC is only waking
    getattr(env.controller, action)()
    assert env.commands == []
    assert env.notes == [("notify.power_unavailable", {})]


def test_power_action_is_skipped_while_busy(online):
    online.controller._busy.acquire()
    online.controller.shutdown()
    assert online.commands == []
    assert online.notes == []
