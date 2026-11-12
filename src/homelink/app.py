"""System tray interface."""

import logging
import os
import threading
from collections.abc import Callable
from pathlib import Path

import pystray
from pystray import Menu
from pystray import MenuItem as Item

from homelink.config import Config
from homelink.controller import Controller
from homelink.i18n import Translator
from homelink.icons import BUILTIN_DIR, IconSet
from homelink.net import ping, ports_open
from homelink.rdp import ensure_rdp_file, open_rdp
from homelink.router import MikroTikRouter
from homelink.state import PcState, Status, TunnelState
from homelink.tunnel import WireGuardTunnel

APP_NAME = "HomeLink"

log = logging.getLogger(__name__)


class TrayApp:
    def __init__(self, config: Config, config_path: Path, translate: Translator):
        self.config = config
        self.config_path = config_path
        self.t = translate
        self.icons = IconSet(Path(config.icons_dir) if config.icons_dir else BUILTIN_DIR)
        self.controller = Controller(
            config=config,
            tunnel=WireGuardTunnel(config.wireguard_exe, config.tunnel_conf),
            router=MikroTikRouter(
                config.router_api,
                config.router_user,
                config.router_password,
                config.router_verify_tls,
                config.router_timeout,
            ),
            ping=ping,
            ports_open=ports_open,
            on_change=self._show,
            on_notify=self._notify,
        )
        self._stop = threading.Event()
        status = self.controller.status
        self.icon = pystray.Icon(
            APP_NAME, self.icons.get(status), self._tooltip(status), menu=self._menu()
        )

    # ---------- presentation ----------
    def _tooltip(self, status: Status) -> str:
        return self.t(
            "tooltip", tunnel=self.t(f"tunnel.{status.tunnel}"), pc=self.t(f"pc.{status.pc}")
        )

    def _show(self, status: Status) -> None:
        self.icon.icon = self.icons.get(status)
        self.icon.title = self._tooltip(status)
        self.icon.update_menu()

    def _notify(self, key: str, **params: object) -> None:
        log.info("notification: %s %s", key, params)
        self.icon.notify(self.t(f"{key}.body", **params), self.t(f"{key}.title"))

    def _menu(self) -> Menu:
        def status() -> Status:
            return self.controller.status

        return Menu(
            Item(
                self.t("menu.connect_and_wake"),
                self._in_thread(self.controller.connect_and_wake),
                default=True,
            ),
            Menu.SEPARATOR,
            Item(
                lambda _: self.t(
                    "menu.tunnel_connect"
                    if status().tunnel is TunnelState.DOWN
                    else "menu.tunnel_disconnect"
                ),
                self._in_thread(self.controller.toggle_tunnel),
            ),
            Item(
                self.t("menu.wake"),
                self._in_thread(self.controller.wake),
                enabled=lambda _: status().tunnel is TunnelState.UP,
            ),
            Item(
                self.t("menu.rdp"),
                self._open_rdp,
                enabled=lambda _: status().pc is PcState.ONLINE,
            ),
            Menu.SEPARATOR,
            Item(self.t("menu.settings"), self._open_config),
            Item(self.t("menu.quit"), self._quit),
        )

    # ---------- menu actions ----------
    @staticmethod
    def _in_thread(action: Callable[[], None]) -> Callable[..., None]:
        """Menu handlers must return at once, so long actions run in a worker thread."""

        def guarded() -> None:
            try:
                action()
            except Exception:
                log.exception("action failed")

        def handler(*_: object) -> None:
            threading.Thread(target=guarded, daemon=True).start()

        return handler

    def _open_rdp(self, *_: object) -> None:
        path = self.config_path.parent / self.config.rdp_file
        ensure_rdp_file(path, self.config.pc_ip, self.config.rdp_user, self.config.rdp_options)
        open_rdp(path)

    def _open_config(self, *_: object) -> None:
        os.startfile(self.config_path)

    def _quit(self, *_: object) -> None:
        log.info("quit")
        self._stop.set()
        self.icon.stop()

    # ---------- lifecycle ----------
    def _monitor(self) -> None:
        while not self._stop.is_set():
            try:
                self.controller.refresh()
            except Exception:
                log.exception("state check failed")
            self._stop.wait(self.controller.poll_interval)

    def run(self) -> None:
        threading.Thread(target=self._monitor, daemon=True).start()
        self.icon.run()
