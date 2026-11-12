import json

import pytest

from homelink.config import EXAMPLE_PATH, Config, parse_config


@pytest.fixture
def raw_config() -> dict:
    return json.loads(EXAMPLE_PATH.read_text(encoding="utf-8"))


@pytest.fixture
def config(raw_config) -> Config:
    return parse_config(raw_config)
