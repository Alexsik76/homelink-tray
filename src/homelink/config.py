"""Loading and validation of config.json.

There are no default values in code: every key must be present in the file.
A template with all keys lives in ``config.example.json`` next to this module.
"""

import json
import shutil
import sys
from dataclasses import dataclass, fields
from pathlib import Path
from typing import get_args, get_origin

PACKAGE_DIR = Path(__file__).resolve().parent
EXAMPLE_PATH = PACKAGE_DIR / "config.example.json"
CONFIG_NAME = "config.json"


class ConfigError(Exception):
    """config.json is missing, malformed or incomplete."""


@dataclass(frozen=True)
class Config:
    language: str

    wireguard_exe: str
    tunnel_conf: str
    tunnel_timeout: int

    router_ip: str
    router_api: str
    router_user: str
    router_password: str
    router_verify_tls: bool
    router_timeout: int

    wol_mac: str
    wol_interface: str

    pc_ip: str
    pc_ports: tuple[int, ...]
    port_timeout_ms: int
    offline_after_failures: int
    ping_timeout_ms: int
    poll_seconds: int
    wake_poll_seconds: int
    wake_timeout: int

    shutdown_command: tuple[str, ...]
    suspend_command: tuple[str, ...]
    power_command_timeout: int

    rdp_user: str
    rdp_file: str
    rdp_options: tuple[str, ...]

    icons_dir: str

    log_file: str
    log_level: str


def app_dir() -> Path:
    """Directory that holds config.json: next to the .exe, otherwise the working directory."""
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path.cwd()


def default_config_path() -> Path:
    return app_dir() / CONFIG_NAME


def create_from_example(path: Path) -> None:
    shutil.copyfile(EXAMPLE_PATH, path)


def _check_type(name: str, value: object, expected: object) -> object:
    if get_origin(expected) is tuple:
        item_type = get_args(expected)[0]
        if not isinstance(value, list):
            raise ConfigError(f"'{name}' must be a list")
        return tuple(_check_type(name, item, item_type) for item in value)
    # bool is a subclass of int: reject it where a number is expected
    if expected is int and isinstance(value, bool):
        raise ConfigError(f"'{name}' must be a number")
    if not isinstance(value, expected):
        raise ConfigError(f"'{name}' must be of type {getattr(expected, '__name__', expected)}")
    return value


def parse_config(raw: object) -> Config:
    if not isinstance(raw, dict):
        raise ConfigError("the top level of config.json must be an object")
    missing = [f.name for f in fields(Config) if f.name not in raw]
    if missing:
        raise ConfigError("missing keys: " + ", ".join(missing))
    values = {f.name: _check_type(f.name, raw[f.name], f.type) for f in fields(Config)}
    config = Config(**values)
    if not config.pc_ports:
        raise ConfigError("'pc_ports' must list at least one port")
    for name in ("shutdown_command", "suspend_command"):
        if not getattr(config, name):
            raise ConfigError(f"'{name}' must contain at least one element")
    if config.offline_after_failures < 1:
        raise ConfigError("'offline_after_failures' must be at least 1")
    return config


def load_config(path: Path) -> Config:
    try:
        raw = json.loads(path.read_text(encoding="utf-8-sig"))
    except FileNotFoundError as exc:
        raise ConfigError(f"{path} not found") from exc
    except json.JSONDecodeError as exc:
        raise ConfigError(f"{path}: invalid JSON ({exc})") from exc
    return parse_config(raw)
