"""Entry point: ``python -m homelink`` or the packaged HomeLink.exe."""

import argparse
import ctypes
import logging
import sys
from logging.handlers import RotatingFileHandler
from pathlib import Path

from homelink import __version__
from homelink.config import ConfigError, create_from_example, default_config_path, load_config
from homelink.elevation import is_admin, relaunch_as_admin
from homelink.i18n import Translator

MB_ICONERROR = 0x10
MB_ICONINFORMATION = 0x40
FALLBACK_LANGUAGE = "en"
LOG_MAX_BYTES = 1_000_000
LOG_BACKUPS = 2
LOG_FORMAT = "%(asctime)s %(levelname)-7s %(name)s: %(message)s"


def message_box(text: str, title: str, flags: int) -> None:
    ctypes.windll.user32.MessageBoxW(None, text, title, flags)


def setup_logging(path: Path, level: str) -> None:
    handler = RotatingFileHandler(
        path, maxBytes=LOG_MAX_BYTES, backupCount=LOG_BACKUPS, encoding="utf-8"
    )
    handler.setFormatter(logging.Formatter(LOG_FORMAT))
    logging.basicConfig(level=level.upper(), handlers=[handler])


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="homelink", description=__doc__)
    parser.add_argument("--config", type=Path, help="path to config.json")
    parser.add_argument("--version", action="version", version=__version__)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if not is_admin():
        relaunch_as_admin()
        return 0

    config_path = (args.config or default_config_path()).resolve()
    fallback = Translator(FALLBACK_LANGUAGE)
    if not config_path.exists():
        create_from_example(config_path)
        message_box(
            fallback("error.config_created", path=config_path),
            fallback("error.title"),
            MB_ICONINFORMATION,
        )
        return 1
    try:
        config = load_config(config_path)
        translate = Translator(config.language)
    except (ConfigError, ValueError) as exc:
        message_box(fallback("error.config", detail=exc), fallback("error.title"), MB_ICONERROR)
        return 1

    setup_logging(config_path.parent / config.log_file, config.log_level)
    logging.getLogger(__name__).info("HomeLink %s started, settings: %s", __version__, config_path)

    # imported late: the tray backend is needed only once the settings are valid
    from homelink.app import TrayApp

    TrayApp(config, config_path, translate).run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
