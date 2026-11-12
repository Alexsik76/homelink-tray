"""Interface strings loaded from locales/<language>.json."""

import json
from pathlib import Path

LOCALES_DIR = Path(__file__).resolve().parent / "locales"


class Translator:
    def __init__(self, language: str, locales_dir: Path = LOCALES_DIR):
        path = locales_dir / f"{language}.json"
        if not path.is_file():
            available = ", ".join(sorted(p.stem for p in locales_dir.glob("*.json")))
            raise ValueError(f"unknown language '{language}' (available: {available})")
        self._strings: dict[str, str] = json.loads(path.read_text(encoding="utf-8"))

    def __call__(self, key: str, **params: object) -> str:
        return self._strings[key].format(**params)
