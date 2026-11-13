#!/usr/bin/env python3
"""Third-party notices and the licence and network policy of the Windows build of HomeLink.

    python3 tools/third_party_notices.py          write THIRD-PARTY-NOTICES.txt
    python3 tools/third_party_notices.py --check  fail on a policy violation or a stale file

The dependency graph is read from `cargo metadata --locked --filter-platform x86_64-pc-windows-msvc`
(no network, no tool to install). Only the packages the `homelink` package reaches through
normal and build dependencies count; `packages` in the metadata lists the whole lock file, which
includes crates of features the build does not use. The policy fails when a crate has a licence
that cannot be satisfied with the allow list, when a crate does not come from crates.io, or when
an HTTP or TLS client crate is in the graph (docs/rust-architecture.md, section 4).
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "rust" / "Cargo.toml"
NOTICES = ROOT / "THIRD-PARTY-NOTICES.txt"
OFL_TEXT = ROOT / "rust" / "app" / "ui" / "fonts" / "OFL.txt"
TARGET = "x86_64-pc-windows-msvc"
APP = "homelink"
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
INTER_VERSION = "4.1"  # rust/app/ui/fonts/README.md
SLINT_CHOSEN = "LicenseRef-Slint-Royalty-free-2.0"

# Every licence identifier that a crate of the graph may be under, or that an OR alternative
# may be chosen as. GPL and LicenseRef-Slint-Software are not listed: they are only reachable
# through an OR whose other branch is allowed.
ALLOWED = frozenset(
    {
        "0BSD",
        "Apache-2.0",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "BSL-1.0",
        "CC0-1.0",
        "ISC",
        "MIT",
        "Unicode-3.0",
        "Unlicense",
        "Zlib",
        SLINT_CHOSEN,
    }
)

# HTTP and TLS client crates: the program must not contain one (`name` or `name-*`).
NETWORK_CLIENTS = (
    "reqwest",
    "ureq",
    "hyper",
    "rustls",
    "native-tls",
    "openssl",
    "curl",
    "attohttpc",
    "isahc",
    "minreq",
)


class PolicyError(Exception):
    """A licence expression that this script cannot read."""


def tokenize(expression):
    """Splits an SPDX-like expression into parentheses, AND, OR and identifiers.

    The old slash form (`MIT/Apache-2.0`) is read as OR, as the crates mean it.
    """
    spaced = expression.replace("/", " OR ").replace("(", " ( ").replace(")", " ) ")
    tokens = spaced.split()
    for token in tokens:
        if token not in {"(", ")", "AND", "OR"} and not re.fullmatch(r"[A-Za-z0-9.+-]+", token):
            raise PolicyError(f"unreadable licence expression {expression!r}")
    return tokens


def parse(expression):
    """Returns a tree: an identifier string, or ("AND"|"OR", [children])."""
    tokens = tokenize(expression)
    position = 0

    def peek():
        return tokens[position] if position < len(tokens) else None

    def take(expected=None):
        nonlocal position
        token = peek()
        if token is None or (expected is not None and token != expected):
            raise PolicyError(f"unreadable licence expression {expression!r}")
        position += 1
        return token

    def or_expr():
        items = [and_expr()]
        while peek() == "OR":
            take("OR")
            items.append(and_expr())
        return items[0] if len(items) == 1 else ("OR", items)

    def and_expr():
        items = [atom()]
        while peek() == "AND":
            take("AND")
            items.append(atom())
        return items[0] if len(items) == 1 else ("AND", items)

    def atom():
        if peek() == "(":
            take("(")
            inner = or_expr()
            take(")")
            return inner
        token = take()
        if token in {"AND", "OR", ")"}:
            raise PolicyError(f"unreadable licence expression {expression!r}")
        return token

    tree = or_expr()
    if position != len(tokens):
        raise PolicyError(f"unreadable licence expression {expression!r}")
    return tree


def identifiers(tree):
    if isinstance(tree, str):
        return {tree}
    return set().union(*(identifiers(child) for child in tree[1]))


def is_allowed(tree):
    if isinstance(tree, str):
        return tree in ALLOWED
    operator, children = tree
    results = [is_allowed(child) for child in children]
    return all(results) if operator == "AND" else any(results)


def metadata():
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--filter-platform",
            TARGET,
            "--format-version",
            "1",
            "--manifest-path",
            str(MANIFEST),
        ],
        capture_output=True,
        encoding="utf-8",
        check=False,
    )
    if result.returncode != 0:
        sys.exit(f"cargo metadata failed:\n{result.stderr}")
    return json.loads(result.stdout)


def build_graph(meta):
    """Returns (crates, problems): the crates of the build, sorted, and the policy problems."""
    packages = {package["id"]: package for package in meta["packages"]}
    nodes = {node["id"]: node for node in meta["resolve"]["nodes"]}
    members = set(meta["workspace_members"])
    app_id = next(member for member in members if packages[member]["name"] == APP)

    reached = {app_id}
    pending = [app_id]
    while pending:
        node = nodes[pending.pop()]
        for dep in node["deps"]:
            kinds = {entry.get("kind") for entry in dep["dep_kinds"]}
            if (kinds & {None, "build"}) and dep["pkg"] not in reached:
                reached.add(dep["pkg"])
                pending.append(dep["pkg"])

    problems = []
    crates = []
    for package_id in reached - members:
        package = packages[package_id]
        name, version = package["name"], package["version"]
        license_text = package.get("license")
        if package.get("source") != CRATES_IO:
            problems.append(f"{name} {version}: not from crates.io ({package.get('source')})")
        if not license_text:
            problems.append(f"{name} {version}: no licence in the Cargo metadata")
            license_text = "(none)"
        else:
            try:
                tree = parse(license_text)
            except PolicyError as error:
                problems.append(f"{name} {version}: {error}")
            else:
                if not is_allowed(tree):
                    problems.append(
                        f"{name} {version}: licence {license_text!r} is not in the allow list"
                    )
        crates.append((name, version, license_text))
        if any(name == client or name.startswith(client + "-") for client in NETWORK_CLIENTS):
            problems.append(f"{name} {version}: HTTP or TLS client crate in the graph")

    crates.sort()
    slint = next(
        (packages[package_id] for package_id in reached if packages[package_id]["name"] == "slint"),
        None,
    )
    if slint is None or not slint.get("license"):
        problems.append("slint is not in the graph with a licence")
        return crates, problems, ("unknown", "")
    try:
        offered = identifiers(parse(slint["license"]))
    except PolicyError as error:
        problems.append(f"slint: {error}")
    else:
        if SLINT_CHOSEN not in offered:
            problems.append(f"slint's licence {slint['license']!r} does not offer {SLINT_CHOSEN}")
    return crates, problems, (slint["version"], slint["license"])


def licence_summary(crates):
    counts = {}
    for _, _, license_text in crates:
        counts[license_text] = counts.get(license_text, 0) + 1
    return counts


def render(crates, slint):
    slint_version, slint_license = slint
    lines = [
        "THIRD-PARTY NOTICES: HomeLink, Windows build",
        "=" * 44,
        "",
        "HomeLink itself is licensed under the MIT licence (the LICENSE file).",
        "",
        "This file is generated by tools/third_party_notices.py from",
        "`cargo metadata --locked --filter-platform x86_64-pc-windows-msvc`.",
        "Do not edit it by hand: run `python3 tools/third_party_notices.py` and commit",
        "the result. CI fails when it is out of date.",
        "",
        "1. Crates in the build",
        "",
        "The crates the `homelink` package reaches through normal and build dependencies,",
        "with the licence expression that Cargo metadata gives for each. Build-only crates",
        "are included because they are part of the build. Licence expressions are shown as",
        "written in the crate; `/` in an old form means OR. The list is what Cargo resolves",
        "for this target, so it may name crates that the executable does not link.",
        "",
    ]
    for name, version, license_text in crates:
        lines.append(f"{name} {version}: {license_text}")
    lines += [
        "",
        "2. Slint",
        "",
        f"Slint {slint_version} (the crates i-slint-* and slint-* above), https://slint.dev",
        "Licence expression as Cargo metadata gives it:",
        slint_license,
        f"HomeLink uses Slint under the {SLINT_CHOSEN} alternative.",
        "",
        f"3. Inter {INTER_VERSION} font, embedded in HomeLink.exe",
        "",
        "Font: Inter (Regular and SemiBold), https://github.com/rsms/inter",
        "Licence: SIL Open Font License 1.1, text below.",
        "",
        "-" * 72,
    ]
    lines.append(OFL_TEXT.read_text(encoding="utf-8").rstrip("\n"))
    lines.append("-" * 72)
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="do not write; fail when the policy is violated or the file is out of date",
    )
    args = parser.parse_args()

    crates, problems, slint = build_graph(metadata())

    print(f"{len(crates)} crates in the Windows build of homelink. Licences found:")
    for license_text, count in sorted(licence_summary(crates).items()):
        print(f"  {count:4}  {license_text}")

    if problems:
        print("Policy violations:")
        for problem in problems:
            print(f"  {problem}")
        return 1

    text = render(crates, slint)
    if args.check:
        current = NOTICES.read_bytes().decode("utf-8") if NOTICES.exists() else ""
        if current != text:
            print(
                "THIRD-PARTY-NOTICES.txt is out of date: run python3 tools/third_party_notices.py"
            )
            return 1
        print("Policy holds and THIRD-PARTY-NOTICES.txt is up to date.")
        return 0

    NOTICES.write_bytes(text.encode("utf-8"))
    print(f"Wrote {NOTICES.name}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
