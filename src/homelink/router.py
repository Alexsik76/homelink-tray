"""Wake-on-LAN sent by a MikroTik router through its REST API."""

import logging

import requests
import urllib3

log = logging.getLogger(__name__)


class RouterError(Exception):
    """The router is unreachable or rejected the request."""


class MikroTikRouter:
    def __init__(self, api_url: str, user: str, password: str, verify_tls: bool, timeout: float):
        self.url = api_url.rstrip("/") + "/rest/tool/wol"
        self.auth = (user, password)
        self.verify_tls = verify_tls
        self.timeout = timeout
        if not verify_tls:
            urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)

    def wake(self, mac: str, interface: str) -> None:
        try:
            response = requests.post(
                self.url,
                json={"mac": mac, "interface": interface},
                auth=self.auth,
                timeout=self.timeout,
                verify=self.verify_tls,
            )
        except requests.RequestException as exc:
            raise RouterError(str(exc)) from exc
        log.info("POST %s -> HTTP %s %s", self.url, response.status_code, response.text)
        if response.status_code >= 300:
            raise RouterError(f"HTTP {response.status_code}: {response.text}")
