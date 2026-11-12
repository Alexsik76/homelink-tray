import json
from dataclasses import fields

import pytest

from homelink.config import Config, ConfigError, create_from_example, load_config, parse_config


def test_example_matches_schema(raw_config):
    assert set(raw_config) == {f.name for f in fields(Config)}
    assert isinstance(parse_config(raw_config), Config)


def test_missing_keys_are_listed(raw_config):
    del raw_config["pc_ip"], raw_config["wol_mac"]
    with pytest.raises(ConfigError, match="pc_ip") as error:
        parse_config(raw_config)
    assert "wol_mac" in str(error.value)


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("poll_seconds", "5"),
        ("poll_seconds", True),
        ("router_verify_tls", 1),
        ("rdp_options", "screen mode id:i:2"),
        ("rdp_options", [1, 2]),
        ("pc_ip", 10),
        ("pc_ports", ["3389"]),
        ("pc_ports", 3389),
        ("pc_ports", []),
        ("offline_after_failures", 0),
    ],
)
def test_wrong_type_is_rejected(raw_config, key, value):
    raw_config[key] = value
    with pytest.raises(ConfigError, match=key):
        parse_config(raw_config)


def test_unknown_keys_are_ignored(raw_config):
    raw_config["tunnel_name"] = "legacy"
    assert parse_config(raw_config).pc_ip == raw_config["pc_ip"]


def test_load_reports_missing_file_and_bad_json(tmp_path):
    with pytest.raises(ConfigError, match="not found"):
        load_config(tmp_path / "config.json")
    broken = tmp_path / "broken.json"
    broken.write_text("{", encoding="utf-8")
    with pytest.raises(ConfigError, match="invalid JSON"):
        load_config(broken)


def test_created_file_is_loadable(tmp_path):
    path = tmp_path / "config.json"
    create_from_example(path)
    assert load_config(path).rdp_options == tuple(
        json.loads(path.read_text(encoding="utf-8"))["rdp_options"]
    )
