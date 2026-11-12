"""Application logic without any user interface.

The controller owns the current status, performs the actions and reports changes
through two callbacks: ``on_change(status)`` and ``on_notify(key, **params)``,
where ``key`` is a prefix of a pair of strings in the locale files.
"""

import logging
import threading
import time
from collections.abc import Callable
from typing import Protocol

from homelink.config import Config
from homelink.router import RouterError
from homelink.state import PcState, Status, TunnelState
from homelink.tunnel import TunnelError

DETAIL_LIMIT = 200

log = logging.getLogger(__name__)


class Tunnel(Protocol):
    def is_running(self) -> bool: ...
    def up(self) -> None: ...
    def down(self) -> None: ...


class Router(Protocol):
    def wake(self, mac: str, interface: str) -> None: ...


class Controller:
    def __init__(
        self,
        config: Config,
        tunnel: Tunnel,
        router: Router,
        ping: Callable[[str, int], bool],
        ports_open: Callable[[str, tuple[int, ...], int], bool],
        run_command: Callable[[list[str], float], tuple[int, str]],
        on_change: Callable[[Status], None],
        on_notify: Callable[..., None],
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
    ):
        self.config = config
        self.tunnel = tunnel
        self.router = router
        self._ping = ping
        self._ports_open = ports_open
        self._run_command = run_command
        self._on_change = on_change
        self._on_notify = on_notify
        self._clock = clock
        self._sleep = sleep
        self._busy = threading.Lock()
        self._waking_until = 0.0
        self._pc_failures = 0
        self.status = Status()

    # ---------- helpers ----------
    @property
    def busy(self) -> bool:
        return self._busy.locked()

    @property
    def poll_interval(self) -> float:
        if self.status.pc is PcState.WAKING:
            return self.config.wake_poll_seconds
        return self.config.poll_seconds

    def _set(self, tunnel: TunnelState | None = None, pc: PcState | None = None) -> None:
        new = Status(tunnel=tunnel or self.status.tunnel, pc=pc or self.status.pc)
        if new != self.status:
            log.info(
                "state: tunnel %s -> %s, pc %s -> %s",
                self.status.tunnel,
                new.tunnel,
                self.status.pc,
                new.pc,
            )
            self.status = new
            self._on_change(new)

    def _router_reachable(self) -> bool:
        return self._ping(self.config.router_ip, self.config.ping_timeout_ms)

    def _pc_ready(self) -> bool:
        """The PC is ready when every port from the settings accepts a connection."""
        return self._ports_open(
            self.config.pc_ip, self.config.pc_ports, self.config.port_timeout_ms
        )

    def _start_tunnel(self) -> bool:
        """Bring the tunnel up and wait until the router answers through it."""
        self._set(tunnel=TunnelState.STARTING)
        try:
            self.tunnel.up()
        except TunnelError as exc:
            log.error("tunnel start failed: %s", exc)
            self._on_notify("notify.tunnel_failed", detail=str(exc)[:DETAIL_LIMIT])
            return False
        deadline = self._clock() + self.config.tunnel_timeout
        while self._clock() < deadline:
            if self._router_reachable():
                self._set(tunnel=TunnelState.UP)
                return True
            self._sleep(1)
        log.warning(
            "router %s did not answer within %s s",
            self.config.router_ip,
            self.config.tunnel_timeout,
        )
        return False

    def _send_wol(self) -> None:
        try:
            self.router.wake(self.config.wol_mac, self.config.wol_interface)
        except RouterError as exc:
            log.error("wake-on-lan failed: %s", exc)
            self._on_notify("notify.wol_failed", detail=str(exc)[:DETAIL_LIMIT])
            return
        log.info("wake-on-lan sent to %s via %s", self.config.wol_mac, self.config.wol_interface)
        self._waking_until = self._clock() + self.config.wake_timeout
        self._set(pc=PcState.WAKING)

    # ---------- actions ----------
    def connect_and_wake(self) -> None:
        """Bring the tunnel up and wake the PC unless it already answers."""
        log.info("action: connect and wake")
        if not self._busy.acquire(blocking=False):
            log.info("skipped: another action is in progress")
            return
        try:
            if not self._start_tunnel():
                self._refresh()
                return
            if self._pc_ready():
                self._pc_failures = 0
                self._set(pc=PcState.ONLINE)
            else:
                self._send_wol()
        finally:
            self._busy.release()

    def wake(self) -> None:
        log.info("action: wake")
        if self.status.tunnel is not TunnelState.UP:
            self._on_notify("notify.tunnel_required")
            return
        self._send_wol()

    def toggle_tunnel(self) -> None:
        log.info("action: toggle tunnel (current state: %s)", self.status.tunnel)
        if not self._busy.acquire(blocking=False):
            log.info("skipped: another action is in progress")
            return
        try:
            if self.status.tunnel is TunnelState.DOWN:
                self._start_tunnel()
            else:
                self.tunnel.down()
            self._refresh()
        finally:
            self._busy.release()

    def shutdown(self) -> None:
        """Power the PC off with the configured command."""
        self._power("shutdown", self.config.shutdown_command, "notify.shutdown_sent")

    def suspend(self) -> None:
        """Put the PC to sleep with the configured command."""
        self._power("suspend", self.config.suspend_command, "notify.suspend_sent")

    def _power(self, name: str, command: tuple[str, ...], sent_key: str) -> None:
        """Run a power command; the state is left to the monitoring to update."""
        log.info("action: %s", name)
        if not self._busy.acquire(blocking=False):
            log.info("skipped: another action is in progress")
            return
        try:
            if self.status.pc is not PcState.ONLINE:
                log.info("skipped: pc is %s", self.status.pc)
                self._on_notify("notify.power_unavailable")
                return
            log.info("running: %s", " ".join(command))
            code, output = self._run_command(list(command), self.config.power_command_timeout)
            log.info("%s command exit code %s: %s", name, code, output)
            if code == 0:
                self._on_notify(sent_key, host=self.config.pc_ip)
            else:
                self._on_notify("notify.power_failed", detail=output[:DETAIL_LIMIT])
        finally:
            self._busy.release()

    # ---------- monitoring ----------
    def refresh(self) -> None:
        """Re-read the real state; skipped while an action is in progress."""
        if not self.busy:
            self._refresh()

    def _refresh(self) -> None:
        if not self.tunnel.is_running():
            self._waking_until = 0.0
            self._pc_failures = 0
            self._set(tunnel=TunnelState.DOWN, pc=PcState.UNKNOWN)
            return
        if not self._router_reachable():
            self._pc_failures = 0
            self._set(tunnel=TunnelState.STARTING, pc=PcState.UNKNOWN)
            return

        was_waking = self.status.pc is PcState.WAKING
        if self._pc_ready():
            self._waking_until = 0.0
            self._pc_failures = 0
            self._set(tunnel=TunnelState.UP, pc=PcState.ONLINE)
            if was_waking:
                self._on_notify("notify.pc_online", host=self.config.pc_ip)
            return

        # a single failed check must not flip an online PC to offline
        self._pc_failures += 1
        if (
            self.status.pc is PcState.ONLINE
            and self._pc_failures < self.config.offline_after_failures
        ):
            log.info(
                "pc check failed (%s of %s)", self._pc_failures, self.config.offline_after_failures
            )
            self._set(tunnel=TunnelState.UP)
        elif was_waking and self._clock() < self._waking_until:
            self._set(tunnel=TunnelState.UP)
        else:
            self._set(tunnel=TunnelState.UP, pc=PcState.OFFLINE)
            if was_waking:
                log.warning(
                    "pc %s did not answer within %s s after wake-on-lan",
                    self.config.pc_ip,
                    self.config.wake_timeout,
                )
                self._on_notify("notify.pc_timeout")
