import json
import sys

import pytest
import requests

from homelink.i18n import LOCALES_DIR, Translator
from homelink.icons import BUILTIN_DIR, IconSet, icon_name
from homelink.rdp import ensure_rdp_file
from homelink.router import MikroTikRouter, RouterError
from homelink.state import PcState, Status, TunnelState
from homelink.tunnel import WireGuardTunnel, tunnel_name


# ---------- tunnel ----------
@pytest.mark.parametrize(
    ("path", "name"),
    [
        (r"C:\Program Files\WireGuard\Data\Configurations\home.conf.dpapi", "home"),
        (r"C:\wg\office-vpn.conf", "office-vpn"),
        (r"C:\wg\My.Tunnel.CONF.DPAPI", "My.Tunnel"),
    ],
)
def test_tunnel_name(path, name):
    assert tunnel_name(path) == name


def test_service_name():
    assert WireGuardTunnel("wireguard.exe", r"C:\wg\home.conf.dpapi").service == (
        "WireGuardTunnel$home"
    )


# ---------- router ----------
class FakeResponse:
    def __init__(self, status_code, text=""):
        self.status_code = status_code
        self.text = text


def make_router():
    return MikroTikRouter("http://router/", "user", "secret", verify_tls=False, timeout=3)


def test_router_sends_wol_request(monkeypatch):
    sent = {}

    def fake_post(url, **kwargs):
        sent.update(url=url, **kwargs)
        return FakeResponse(200)

    monkeypatch.setattr(requests, "post", fake_post)
    make_router().wake("AA:BB:CC:DD:EE:FF", "bridge")
    assert sent["url"] == "http://router/rest/tool/wol"
    assert sent["json"] == {"mac": "AA:BB:CC:DD:EE:FF", "interface": "bridge"}
    assert sent["auth"] == ("user", "secret")
    assert sent["verify"] is False
    assert sent["timeout"] == 3


def test_router_http_error(monkeypatch):
    monkeypatch.setattr(requests, "post", lambda *a, **k: FakeResponse(500, "not allowed"))
    with pytest.raises(RouterError, match="HTTP 500: not allowed"):
        make_router().wake("AA:BB:CC:DD:EE:FF", "bridge")


def test_router_unreachable(monkeypatch):
    def fail(*args, **kwargs):
        raise requests.ConnectionError("no route")

    monkeypatch.setattr(requests, "post", fail)
    with pytest.raises(RouterError, match="no route"):
        make_router().wake("AA:BB:CC:DD:EE:FF", "bridge")


# ---------- rdp ----------
def test_rdp_file_is_created_once(tmp_path):
    path = tmp_path / "home.rdp"
    ensure_rdp_file(path, "10.0.0.5", "user", ["screen mode id:i:2"])
    text = path.read_text(encoding="utf-16")
    assert text.splitlines() == [
        "full address:s:10.0.0.5",
        "username:s:user",
        "screen mode id:i:2",
    ]
    ensure_rdp_file(path, "10.0.0.9", "other", [])  # an existing (signed) file is kept
    assert path.read_text(encoding="utf-16") == text


# ---------- icons ----------
def test_builtin_icons_cover_every_state():
    icons = IconSet()
    for pc in PcState:
        for tunnel in TunnelState:
            status = Status(tunnel=tunnel, pc=pc)
            assert (BUILTIN_DIR / icon_name(status)).is_file()
            assert icons.get(status).mode == "RGBA"


def test_icon_name():
    assert icon_name(Status(TunnelState.UP, PcState.ONLINE)) == "online_up.png"


# ---------- i18n ----------
def test_locales_have_identical_keys():
    keys = {
        path.stem: set(json.loads(path.read_text(encoding="utf-8")))
        for path in LOCALES_DIR.glob("*.json")
    }
    assert len(keys) >= 2
    assert len({frozenset(k) for k in keys.values()}) == 1


@pytest.mark.parametrize("language", ["uk", "en"])
def test_locale_covers_states_and_formats(language):
    t = Translator(language)
    for tunnel in TunnelState:
        assert t(f"tunnel.{tunnel}")
    for pc in PcState:
        assert t(f"pc.{pc}")
    assert "X" in t("tooltip", tunnel="X", pc="Y")


def test_unknown_language():
    with pytest.raises(ValueError, match="available: en, uk"):
        Translator("xx")


# ---------- ports ----------
def test_ports_open():
    import socket

    from homelink.net import ports_open

    with socket.socket() as server:
        server.bind(("127.0.0.1", 0))
        server.listen()
        port = server.getsockname()[1]
        assert ports_open("127.0.0.1", [port], 500)
        with socket.socket() as unused:
            unused.bind(("127.0.0.1", 0))
            closed = unused.getsockname()[1]
        assert not ports_open("127.0.0.1", [port, closed], 500)


# ---------- commands ----------
# run_hidden relies on Windows-only subprocess arguments
windows_only = pytest.mark.skipif(sys.platform != "win32", reason="run_hidden is Windows-only")


@windows_only
def test_run_command_returns_code_and_output():
    from homelink.net import run_command

    code, output = run_command(
        [sys.executable, "-c", "import sys; print('x'); print('e', file=sys.stderr); sys.exit(3)"],
        10,
    )
    assert code == 3
    assert output.splitlines() == ["e", "x"]


@windows_only
def test_run_command_timeout():
    from homelink.net import run_command

    code, output = run_command([sys.executable, "-c", "import time; time.sleep(30)"], 0.5)
    assert code != 0
    assert "timed out" in output


@windows_only
def test_run_command_missing_program():
    from homelink.net import run_command

    code, output = run_command(["homelink-no-such-program"], 5)
    assert code != 0
    assert output
