"""States of the tunnel and of the home PC."""

from dataclasses import dataclass
from enum import StrEnum


class TunnelState(StrEnum):
    DOWN = "down"
    STARTING = "starting"
    UP = "up"


class PcState(StrEnum):
    UNKNOWN = "unknown"
    OFFLINE = "offline"
    WAKING = "waking"
    ONLINE = "online"


@dataclass(frozen=True)
class Status:
    tunnel: TunnelState = TunnelState.DOWN
    pc: PcState = PcState.UNKNOWN
