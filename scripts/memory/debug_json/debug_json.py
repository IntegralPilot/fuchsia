# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Parse `ffx profile memory --debug-json` output into Python objects.
"""

import itertools
import json
from dataclasses import dataclass, field
from functools import cached_property
from pathlib import Path
from typing import Dict, List, Optional, Tuple

from dataclasses_json_lite import Undefined, config, dataclass_json
from multidict import multidict


def table_of(type):
    return lambda table: [
        type.from_dict(dict(zip(table[0], row))) for row in table[1:]
    ]


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Kernel:
    total: int
    free: int
    wired: int
    total_heap: int
    free_heap: int
    vmo: int
    mmu: int
    ipc: int
    other: int
    vmo_pager_total: int
    vmo_pager_newest: int
    vmo_pager_oldest: int
    vmo_discardable_locked: int
    vmo_discardable_unlocked: int
    vmo_reclaim_disabled: int


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class KmemStatsCompression:
    uncompressed_storage_bytes: int
    compressed_storage_bytes: int
    compressed_fragmentation_bytes: int
    compression_time: int
    decompression_time: int
    total_page_compression_attempts: int
    failed_page_compression_attempts: int
    total_page_decompressions: int
    compressed_page_evictions: int
    eager_page_compressions: int
    memory_pressure_page_compressions: int
    critical_memory_page_compressions: int
    pages_decompressed_unit_ns: int
    pages_decompressed_within_log_time: List[int]


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Process:
    koid: int
    name: str
    vmos_koid: List[int] = field(metadata=config(field_name="vmos"))

    @cached_property
    def vmos(self) -> Tuple["VMO"]:
        return tuple(self._capture.vmo_by_koid[koid] for koid in self.vmos_koid)

    @cached_property
    def vmos_and_their_ancestors(self) -> Tuple["VMO"]:
        koids = set()
        for vmo in self.vmos:
            koids.add(vmo.koid)
            koids.update(a.koid for a in vmo.ancestors)
        return tuple(self._capture.vmo_by_koid[koid] for koid in koids)


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class VMO:
    koid: int
    name_index: int = field(metadata=config(field_name="name"))
    parent_koid: int
    committed_bytes: int
    allocated_bytes: int
    populated_bytes: Optional[int] = None

    def __str__(self):
        return f"VMO(koid={self.koid!r} name={self.name!r})"

    @classmethod
    def expand_vmos_with_ancestors(cls, vmos):
        koids = set()
        for vmo in vmos:
            koids.add(vmo.koid)
            koids.update(a.koid for a in vmo.ancestors)
        return tuple(vmos[0]._capture.vmo_by_koid[koid] for koid in koids)

    @cached_property
    def name_or_ancestor_name(self):
        if not self.name and self.parent:
            return self.parent.name_or_ancestor_name
        return self.name

    @cached_property
    def name(self):
        return self._capture.vmo_names[self.name_index]

    @cached_property
    def parent(self):
        return (
            self._capture.vmo_by_koid[self.parent_koid]
            if self.parent_koid
            else None
        )

    @cached_property
    def processes(self):
        return self._capture.processes_by_vmo_koid[self.koid]

    @cached_property
    def children(self):
        return self._capture.vmos_by_parent_vmo_koid[self.koid]

    def descendants(self):
        for vmo in self.children:
            yield vmo
            yield from vmo.descendants()

    @cached_property
    def ancestors(self):
        return (self.parent,) + self.parent.ancestors if self.parent else ()


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Capture:
    time: int = field(metadata=config(field_name="Time"))
    processes: List[Process] = field(
        metadata=config(field_name="Processes", decoder=table_of(Process))
    )
    kernel: Kernel = field(metadata=config(field_name="Kernel"))
    vmo_names: List[str] = field(metadata=config(field_name="VmoNames"))
    vmos: List[VMO] = field(
        metadata=config(field_name="Vmos", decoder=table_of(VMO))
    )
    kmem_stats_compression: Optional[KmemStatsCompression] = None

    @cached_property
    def vmo_by_koid(self):
        return {vmo.koid: vmo for vmo in self.vmos}

    @cached_property
    def processes_by_vmo_koid(self):
        return multidict(
            (vmo_koid, process)
            for process in self.processes
            for vmo_koid in process.vmos_koid
        )

    @cached_property
    def processes_by_vmo_koid_with_ancestors(self):
        return multidict(
            (vmo.koid, process)
            for process in self.processes
            for vmo in VMO.expand_vmos_with_ancestors(process.vmos)
        )

    @cached_property
    def vmos_by_name(self):
        return multidict((vmo.name, vmo) for vmo in self.vmos)

    @cached_property
    def processes_by_name(self) -> Dict[str, List[Process]]:
        return multidict((process.name, process) for process in self.processes)

    @cached_property
    def process_by_koid(self):
        return {process.koid: process for process in self.processes}

    @cached_property
    def vmos_by_parent_vmo_koid(self):
        return multidict((vmo.parent_koid, vmo) for vmo in self.vmos)


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Bucket:
    event_code: int
    name: str
    process: str
    vmo: str


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class ProfileMemoryDebug:
    capture: Capture = field(metadata=config(field_name="Capture"))
    buckets: List[Bucket] = field(metadata=config(field_name="Buckets"))


def parse(path: Path):
    """Parses `ffx profile memory --debug-json`"""
    with open(path, "rt") as f:
        result = ProfileMemoryDebug.from_dict(json.load(f))
    for o in itertools.chain(result.capture.processes, result.capture.vmos):
        o._capture = result.capture
    return result
