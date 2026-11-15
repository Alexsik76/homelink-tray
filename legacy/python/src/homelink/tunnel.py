"""WireGuard tunnel managed as a Windows service of the official client."""

import logging
from pathlib import PureWindowsPath

import psutil

from homelink.net import run_hidden

SERVICE_PREFIX = "WireGuardTunnel$"
CONF_SUFFIXES = (".dpapi", ".conf")
COMMAND_TIMEOUT = 30

log = logging.getLogger(__name__)


class TunnelError(Exception):
    """The WireGuard client refused to install the tunnel service."""


def tunnel_name(conf_path: str) -> str:
    """Tunnel name as WireGuard derives it from the configuration file name."""
    name = PureWindowsPath(conf_path).name
    for suffix in CONF_SUFFIXES:
        if name.lower().endswith(suffix):
            name = name[: -len(suffix)]
    return name


class WireGuardTunnel:
    def __init__(self, wireguard_exe: str, conf_path: str):
        self.wireguard_exe = wireguard_exe
        self.conf_path = conf_path
        self.name = tunnel_name(conf_path)
        self.service = SERVICE_PREFIX + self.name

    def is_running(self) -> bool:
        try:
            return psutil.win_service_get(self.service).status() == "running"
        except Exception:
            return False

    def up(self) -> None:
        if self.is_running():
            return
        result = run_hidden(
            [self.wireguard_exe, "/installtunnelservice", self.conf_path], COMMAND_TIMEOUT
        )
        output = (result.stderr + result.stdout).strip()
        log.info("install service %s: exit code %s %s", self.service, result.returncode, output)
        if result.returncode != 0 and "already installed" not in output.lower():
            raise TunnelError(output)

    def down(self) -> None:
        result = run_hidden(
            [self.wireguard_exe, "/uninstalltunnelservice", self.name], COMMAND_TIMEOUT
        )
        output = (result.stderr + result.stdout).strip()
        log.info("uninstall service %s: exit code %s %s", self.service, result.returncode, output)
