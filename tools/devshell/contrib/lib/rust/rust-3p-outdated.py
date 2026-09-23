#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Checks how out of date our third-party rust deps are

import concurrent.futures
import json
import os
import re
import subprocess
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Tuple
from urllib import error, request

from rust import PREBUILT_THIRD_PARTY_DIR, ROOT_PATH

manifest = ROOT_PATH / "third_party/rust_crates/Cargo.toml"
cargo_binary = PREBUILT_THIRD_PARTY_DIR / "cargo"

cache_dir = (
    Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    / "fuchsia-rust-3p-outdated"
)
cache_dir.mkdir(exist_ok=True)

url = "https://crates.io/api/v1/crates/{}"
headers = {}
opener = request.build_opener()
opener.addheaders = [("user-agent", "fuchsia-rust-3p-outdated")]
request.install_opener(opener)


def parse_semver(
    version_str: str,
) -> Tuple[int, int, int, bool, Tuple[Tuple[int, str], ...]]:
    # Try to match major.minor.patch optional pre-release/build strings if available
    # This is the conventional semantic version format for almost all crates on crates.io.
    # If no match is found, treat it as a 0.0.0 version.
    match = re.match(
        r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$",
        version_str,
    )
    if not match:
        return (0, 0, 0, False, ())

    major, minor, patch, prerelease, build = match.groups()
    major = int(major)
    minor = int(minor)
    patch = int(patch)

    # Prepare for semantic sorting in fetch_crate_versions()
    # Prerelease versions are older than release versions.
    # False < True, so release versions sort higher.
    is_release = prerelease is None

    pre_parts = []
    if prerelease:
        for part in prerelease.split("."):
            if part.isdigit():
                pre_parts.append((0, int(part)))
            else:
                pre_parts.append((1, part))

    return (major, minor, patch, is_release, tuple(pre_parts))


def fetch_crate_versions(crate: str) -> List[Tuple[str, datetime]]:
    cache_path = cache_dir / crate
    if not cache_path.exists():
        try:
            print("Info: fetching", crate, "release data from crates.io")
            request.urlretrieve(url.format(crate), cache_dir / crate)
        except error.HTTPError:
            print("Warning:", crate, "was not found on crates.io")
            return []
    with open(cache_path) as f:
        data = json.load(f)
    versions = [
        (v["num"], datetime.fromisoformat(v["created_at"]))
        for v in data["versions"]
    ]
    # Sort semantically with newest crates first
    versions.sort(key=lambda x: parse_semver(x[0]), reverse=True)
    return versions


def process_crate(
    crate: str, version: str
) -> Tuple[timedelta, int, str, str, Tuple[str, datetime]] | None:
    versions = fetch_crate_versions(crate)
    if not versions:
        return None

    current_semver = parse_semver(version)
    current_is_release = current_semver[3]

    if current_is_release:
        # If the current version is a stable release, ignore the previous pre-releases
        versions = [v for v in versions if parse_semver(v[0])[3]]

    try:
        versions = versions[
            : next((i for i, (v, _) in enumerate(versions) if v == version)) + 1
        ]
    except StopIteration:
        return None
    if not versions:
        return None
    current = versions[-1][1]
    newest = versions[0][1]

    age = newest - current
    if age < timedelta(0):
        age = timedelta(0)

    return (age, len(versions) - 1, crate, version, versions[0])


if __name__ == "__main__":
    cargo_process = subprocess.run(
        ["cargo", "metadata", "--manifest-path", manifest],
        text=True,
        capture_output=True,
    )
    metadata = json.loads(cargo_process.stdout)
    crates = {p["name"]: p["version"] for p in metadata["packages"]}
    updates = []

    # Limit workers to avoid overwhelming crates.io and hitting rate limits.
    with concurrent.futures.ThreadPoolExecutor(max_workers=16) as executor:
        future_to_crate = {
            executor.submit(process_crate, crate, version): crate
            for crate, version in crates.items()
        }
        for future in concurrent.futures.as_completed(future_to_crate):
            crate = future_to_crate[future]
            try:
                result = future.result()
                if result is not None:
                    updates.append(result)
            except Exception as exc:
                print(
                    f"Warning: processing {crate} generated an exception: {exc}"
                )

    updates.sort()
    for age, releases, crate, version, newest in updates:
        print(crate, version, "is", end=" ")
        if version != newest[0]:
            print(age.days, "days out of date")
            print(f"\t{releases} newer versions")
            print(
                f"\tmost recent release was {newest[0]} on {newest[1].date()}"
            )
        else:
            print("up to date")

    print()
    print("median outdatedness:", updates[len(updates) // 2][0].days, "days")
    print(
        f"average outdatedness: {sum(u[0].days for u in updates) // len(updates)} days"
    )
    print("most out of date:", updates[-1][0].days, "days")
    releases_behind = sorted(u[1] for u in updates)
    print()
    print("median releases behind:", releases_behind[len(releases_behind) // 2])
    print(
        f"average releases behind {sum(releases_behind) / len(releases_behind):.1f}"
    )
    print("most releases behind:", releases_behind[-1])
