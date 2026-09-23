# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import random
import secrets
from pathlib import Path

from utils import run_jiri
from worktree import NoFreeWorktreesError, Worktree, WorktreeState

ADJECTIVES = (
    "amber",
    "blue",
    "bold",
    "calm",
    "cool",
    "coral",
    "crisp",
    "cyan",
    "dawn",
    "dry",
    "dune",
    "echo",
    "fast",
    "firm",
    "gray",
    "green",
    "hazy",
    "jade",
    "keen",
    "lime",
    "mint",
    "mist",
    "navy",
    "neat",
    "pale",
    "pink",
    "pure",
    "quiet",
    "red",
    "rich",
    "sage",
    "slate",
    "solar",
    "stone",
    "swift",
    "teal",
    "vivid",
    "warm",
    "wild",
    "yellow",
)

NOUNS = (
    "badger",
    "basalt",
    "canyon",
    "cavern",
    "crag",
    "crest",
    "falcon",
    "field",
    "forest",
    "glacier",
    "groove",
    "harbor",
    "haven",
    "heron",
    "island",
    "lagoon",
    "meadow",
    "mesa",
    "moss",
    "orchard",
    "osprey",
    "otter",
    "pebble",
    "pine",
    "prairie",
    "quarry",
    "rapids",
    "ravine",
    "reef",
    "ridge",
    "river",
    "summit",
    "thicket",
    "tundra",
    "valley",
)


class WorktreePool:
    def __init__(self, fuchsia_dir: str | None = None):
        if fuchsia_dir is None:
            fuchsia_dir = os.environ.get("FUCHSIA_DIR")
        if fuchsia_dir is None:
            fuchsia_dir = str(
                Path(__file__).resolve().parent.parent.parent.parent
            )

        self.fuchsia_dir = Path(fuchsia_dir).resolve()

        if (
            self.fuchsia_dir.parent.name == "worktrees"
            and self.fuchsia_dir.parent.parent.name == ".jiri_root"
        ):
            self.fuchsia_dir = self.fuchsia_dir.parent.parent.parent

        self.jiri_root = self.fuchsia_dir / ".jiri_root"
        self.registry_file = self.jiri_root / "worktrees_registry"
        self.worktrees_dir = self.jiri_root / "worktrees"

    def get_worktrees(self) -> list[Worktree]:
        if not self.registry_file.exists():
            return []
        worktrees = []
        with open(self.registry_file, "r") as f:
            for line in f:
                line = line.strip()
                if line:
                    path = Path(line)
                    worktrees.append(
                        Worktree(path.name, path, self.fuchsia_dir)
                    )
        return worktrees

    def get_worktree_by_name(self, name: str) -> Worktree:
        for wt in self.get_worktrees():
            if wt.name == name:
                return wt
        raise KeyError(f"Worktree '{name}' not found")

    def find_worktree(self, identifier: str) -> Worktree:
        for wt in self.get_worktrees():
            if wt.name == identifier:
                return wt
            if wt.get_state() == WorktreeState.LEASED:
                lease = wt.get_lease_info()
                if lease and lease.task_id == identifier:
                    return wt
        raise KeyError(f"Worktree '{identifier}' not found")

    def get_any_free_worktree(self) -> Worktree:
        free_worktrees = [
            wt
            for wt in self.get_worktrees()
            if wt.get_state() == WorktreeState.FREE
        ]
        if not free_worktrees:
            raise NoFreeWorktreesError("No free worktrees available")
        return random.choice(free_worktrees)

    def _generate_random_pool_name(self) -> str:
        existing = {wt.name for wt in self.get_worktrees()}
        for _ in range(100):
            candidate = f"{random.choice(ADJECTIVES)}-{random.choice(NOUNS)}"
            if (
                candidate not in existing
                and not (self.worktrees_dir / candidate).exists()
            ):
                return candidate
        return f"slot-{secrets.token_hex(3)}"

    def add_worktree(self, name: str | None = None) -> Worktree:
        if name is None:
            name = self._generate_random_pool_name()
        wt_path = self.worktrees_dir / name
        if wt_path.exists():
            raise FileExistsError(f"Worktree path '{wt_path}' already exists")

        run_jiri(self.jiri_root, ["worktree", "add", str(wt_path)], check=True)
        wt = Worktree(name, wt_path, self.fuchsia_dir)
        wt.meta_dir.mkdir(parents=True, exist_ok=True)
        return wt

    def remove_worktree(self, name: str, force: bool = False) -> None:
        wt_path = self.worktrees_dir / name
        if not wt_path.exists():
            raise FileNotFoundError(f"Worktree '{name}' does not exist on disk")

        cmd = ["worktree", "remove"]
        if force:
            cmd.append("-force")
        cmd.append(str(wt_path))
        run_jiri(self.jiri_root, cmd, check=True)

    def get_completion_choices(self, filter_type: str) -> list[str]:
        matches = []
        if filter_type in ("all", "leased"):
            if self.worktrees_dir.exists() and self.worktrees_dir.is_dir():
                for entry in self.worktrees_dir.iterdir():
                    if entry.is_symlink():
                        try:
                            target = entry.readlink()
                            if not target.is_absolute():
                                target = (self.worktrees_dir / target).resolve()
                            if target.is_dir():
                                matches.append(entry.name)
                        except OSError:
                            pass

        if filter_type != "leased":
            for wt in self.get_worktrees():
                is_leased = wt.get_state() == WorktreeState.LEASED
                if filter_type == "physical_free" and not is_leased:
                    matches.append(wt.name)
                elif filter_type == "physical_leased" and is_leased:
                    matches.append(wt.name)
                elif filter_type in ("physical_all", "all"):
                    matches.append(wt.name)

        seen = set()
        unique_matches = []
        for m in matches:
            if m not in seen:
                seen.add(m)
                unique_matches.append(m)
        return unique_matches
