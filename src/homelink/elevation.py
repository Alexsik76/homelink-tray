"""Restart with administrator rights (needed to install the tunnel service)."""

import ctypes
import subprocess
import sys

SW_SHOWNORMAL = 1


def is_admin() -> bool:
    return bool(ctypes.windll.shell32.IsUserAnAdmin())


def relaunch_as_admin() -> None:
    """Start an elevated copy of the program; the caller is expected to exit afterwards."""
    if getattr(sys, "frozen", False):
        target, args = sys.executable, sys.argv[1:]
    else:
        # pythonw.exe keeps the elevated copy free of a console window
        target, args = sys.executable.replace("python.exe", "pythonw.exe"), sys.argv
    ctypes.windll.shell32.ShellExecuteW(
        None, "runas", target, subprocess.list2cmdline(args), None, SW_SHOWNORMAL
    )
