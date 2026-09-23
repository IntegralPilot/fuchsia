# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
from functools import cached_property
from pathlib import Path
from typing import Dict, Iterator, List, Set

import debug_json
import images_json
import smaps
from multidict import multidict


def _parse_android_ps_into_command_by_pid(path: Path) -> Dict[int, str]:
    with open(path) as f:
        pid_command = (l.strip().split(maxsplit=1) for l in f.readlines()[1:])
        return {int(pid): command for pid, command in pid_command}


def _parse_inspect_into_koid_by_pid(inspect_path: Path) -> Dict[int, int]:
    koid_by_pid = {}
    with open(inspect_path) as f:
        for proc in json.load(f):
            moniker = proc["moniker"]
            if not moniker.startswith("core/starnix_runner/kernels"):
                continue
            for tg in proc["payload"]["root"]["container"]["kernel"][
                "thread_groups"
            ].values():
                if "koid" in tg:
                    try:
                        koid_by_pid[tg["pid"]] = tg["koid"]
                    except Exception:
                        print(
                            f"Error: moniker {moniker}  does no have 'pid', 'koid'. {tg.keys()}"
                        )
        return koid_by_pid


class Snapshot:
    """
    Accesses the resource of a snapshot directory with the following layout:

    ./android-ps.txt    output of `ps -e -o PID,ARGS` on the target.
    ./proc/$PID/smaps   a copy of target system `/proc` files.
    ./snapshot          (Fuchsia) `fx ffx target snapshot` result unzipped.
    ./images.json       (Fuchsia) assembly image output.
    ./ffx-profile-memory-debug-json.json" (Fuchsia) `fx ffx profile memory --debug-json` result.
    """

    def __init__(self, directory: Path):
        self.directory = directory
        assert self.directory.is_dir()

    @property
    def name(self) -> str:
        return self.directory.name

    @cached_property
    def command_by_pid(self) -> Dict[int, str]:
        return _parse_android_ps_into_command_by_pid(
            self.directory / "android-ps.txt"
        )

    @cached_property
    def pids_by_command(self) -> Dict[str, List[int]]:
        return multidict(
            (command, pid) for pid, command in self.command_by_pid.items()
        )

    def smaps(self, pid: int) -> List[smaps.SmapEntry]:
        return smaps.parse(self.directory / "proc" / str(pid) / "smaps")

    # Fuchsia only properties below. Will fail because of missing files.
    @cached_property
    def isfuchsia(self) -> bool:
        return (self.directory / "ffx-profile-memory-debug-json.json").is_file()

    @cached_property
    def debug_json(self) -> debug_json.ProfileMemoryDebug:
        return debug_json.parse(
            self.directory / "ffx-profile-memory-debug-json.json"
        )

    @cached_property
    def koid_by_pid(self) -> Dict[int, int]:
        return _parse_inspect_into_koid_by_pid(
            self.directory / "snapshot" / "inspect.json"
        )

    @cached_property
    def pid_by_koid(self) -> Dict[int, int]:
        return {koid: pid for pid, koid in self.koid_by_pid.items()}

    @cached_property
    def images_json(self) -> List[images_json.Image]:
        return images_json.parse(self.directory / "images.json")

    @cached_property
    def blob_file_by_vmo_name(self) -> Dict[str, Set[str]]:
        return multidict(
            (
                (f"blob-{b.merkle[:8]}", b.path)
                for b in images_json.all_blobs(self.images_json)
            ),
            value_container=set,
        )


class Pair:
    """
    Accesses the resources of two snapshot directories.
    """

    def __init__(self, directory: Path):
        self.directory = directory
        assert self.directory.is_dir()
        self.linux = Snapshot(self.directory / "linux")
        self.fuchsia = Snapshot(self.directory / "fuchsia")

    def items(self) -> Iterator[Snapshot]:
        yield self.linux
        yield self.fuchsia
