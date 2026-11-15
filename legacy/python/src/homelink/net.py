"""Reachability checks and a helper for running console programs without a window."""

import socket
import subprocess
from collections.abc import Iterable

CREATE_NO_WINDOW = 0x08000000


def run_hidden(cmd: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
    """Run a console program without flashing a console window."""
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=timeout,
        creationflags=CREATE_NO_WINDOW,
        encoding="oem",
        errors="replace",
        check=False,
    )


def run_command(cmd: list[str], timeout: float) -> tuple[int, str]:
    """Run a command and return its exit code with the combined stderr and stdout text.

    A timeout or a failure to start the program gives a non-zero code and a readable message.
    """
    try:
        result = run_hidden(cmd, timeout)
    except subprocess.TimeoutExpired:
        return -1, f"timed out after {timeout:g} s"
    except OSError as exc:
        return -1, str(exc)
    parts = (result.stderr.strip(), result.stdout.strip())
    return result.returncode, "\n".join(part for part in parts if part)


def ping(host: str, timeout_ms: int) -> bool:
    """True when the host answers a single ICMP echo request."""
    try:
        result = run_hidden(
            ["ping", "-n", "1", "-w", str(timeout_ms), host], timeout=timeout_ms / 1000 + 3
        )
    except (OSError, subprocess.SubprocessError):
        return False
    # "TTL=" is present only in a real echo reply, in every Windows display language
    return "TTL=" in result.stdout.upper()


def ports_open(host: str, ports: Iterable[int], timeout_ms: int) -> bool:
    """True when every listed TCP port accepts a connection."""
    for port in ports:
        try:
            with socket.create_connection((host, port), timeout=timeout_ms / 1000):
                pass
        except OSError:
            return False
    return True
