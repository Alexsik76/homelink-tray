"""System tray interface."""

import ctypes
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
from homelink.net import ping, ports_open, run_command
from homelink.rdp import ensure_rdp_file, open_rdp
from homelink.router import MikroTikRouter
from homelink.state import PcState, Status, TunnelState
from homelink.tunnel import WireGuardTunnel

APP_NAME = "HomeLink"

# MessageBoxW flags and result
MB_YESNO = 0x00000004
MB_ICONQUESTION = 0x00000020
MB_DEFBUTTON2 = 0x00000100
MB_SETFOREGROUND = 0x00010000
MB_TOPMOST = 0x00040000
IDYES = 6

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
            run_command=run_command,
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

        def pc_online(_: object) -> bool:
            return status().pc is PcState.ONLINE

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
                enabled=pc_online,
            ),
            Item(
                self.t("menu.power"),
                Menu(
                    Item(
                        self.t("menu.suspend"),
                        self._confirmed("confirm.suspend", self.controller.suspend),
                        enabled=pc_online,
                    ),
                    Item(
                        self.t("menu.shutdown"),
                        self._confirmed("confirm.shutdown", self.controller.shutdown),
                        enabled=pc_online,
                    ),
                ),
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

    def _confirmed(self, question_key: str, action: Callable[[], None]) -> Callable[..., None]:
        """A menu handler that asks first and runs the action only after "Yes"."""

        def ask_and_run() -> None:
            if self._confirm(self.t(question_key)):
                action()

        return self._in_thread(ask_and_run)

    def _confirm(self, question: str) -> bool:
        flags = MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_SETFOREGROUND | MB_TOPMOST
        answer = ctypes.windll.user32.MessageBoxW(None, question, self.t("confirm.title"), flags)
        return answer == IDYES

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
