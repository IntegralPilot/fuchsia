# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


import json
import tempfile
import unittest

import debug_json


class DebugJsonTest(unittest.TestCase):
    def test_parse(self) -> None:
        json_data = dict(
            Capture=dict(
                Time=1,
                Processes=[["koid", "name", "vmos"], [2, "p2", [2]]],
                Kernel=dict(
                    total=2,
                    free=2,
                    wired=2,
                    total_heap=2,
                    free_heap=2,
                    vmo=2,
                    mmu=2,
                    ipc=2,
                    other=2,
                    vmo_pager_total=2,
                    vmo_pager_newest=2,
                    vmo_pager_oldest=2,
                    vmo_discardable_locked=2,
                    vmo_discardable_unlocked=2,
                    vmo_reclaim_disabled=2,
                ),
                VmoNames=["", "Vmo1"],
                Vmos=[
                    [
                        "koid",
                        "name",
                        "parent_koid",
                        "committed_bytes",
                        "allocated_bytes",
                        "populated_bytes",
                    ],
                    [1, 1, 0, 1024, 2048, 4096],
                    [2, 0, 1, 2048, 4096, 8192],
                    [3, 0, 2, 4096, 8192, 16384],
                ],
                kmem_stats_compression=dict(
                    uncompressed_storage_bytes=3,
                    compressed_storage_bytes=3,
                    compressed_fragmentation_bytes=3,
                    compression_time=3,
                    decompression_time=3,
                    total_page_compression_attempts=3,
                    failed_page_compression_attempts=3,
                    total_page_decompressions=3,
                    compressed_page_evictions=3,
                    eager_page_compressions=3,
                    memory_pressure_page_compressions=3,
                    critical_memory_page_compressions=3,
                    pages_decompressed_unit_ns=3,
                    pages_decompressed_within_log_time=[1, 2, 3, 4, 5],
                ),
            ),
            Buckets=[dict(event_code=1, name="b1", process="p1", vmo="v1")],
        )
        with tempfile.NamedTemporaryFile("wt") as tf:
            json.dump(json_data, tf)
            tf.flush()
            data = debug_json.parse(tf.name)
        import pprint

        pprint.pprint(data)
        vmo1, vmo2, vmo3 = data.capture.vmos
        (p2,) = data.capture.processes
        self.assertEqual(
            vmo1,
            debug_json.VMO(
                koid=1,
                name_index=1,
                parent_koid=0,
                committed_bytes=1024,
                allocated_bytes=2048,
                populated_bytes=4096,
            ),
        )
        self.assertEqual(vmo1.name, "Vmo1")
        self.assertEqual(vmo1.name_or_ancestor_name, "Vmo1")
        self.assertEqual(vmo1.parent, None)
        self.assertEqual(vmo1.processes, [])
        self.assertEqual(list(vmo1.children), [vmo2])
        self.assertEqual(list(vmo1.descendants()), [vmo2, vmo3])
        self.assertEqual(list(vmo1.ancestors), [])

        self.assertEqual(vmo2.name, "")
        self.assertEqual(vmo2.name_or_ancestor_name, "Vmo1")
        self.assertEqual(vmo2.parent, vmo1)
        self.assertEqual(vmo2.processes, [p2])
        self.assertEqual(list(vmo2.children), [vmo3])
        self.assertEqual(list(vmo2.descendants()), [vmo3])
        self.assertEqual(list(vmo2.ancestors), [vmo1])

        self.assertEqual(vmo3.name, "")
        self.assertEqual(vmo3.name_or_ancestor_name, "Vmo1")
        self.assertEqual(vmo3.parent, vmo2)
        self.assertEqual(vmo3.processes, [])
        self.assertEqual(list(vmo3.children), [])
        self.assertEqual(list(vmo3.descendants()), [])
        self.assertEqual(list(vmo3.ancestors), [vmo2, vmo1])

        self.assertEqual(
            p2,
            debug_json.Process(koid=2, name="p2", vmos_koid=[2]),
        )
        self.assertEqual(p2.vmos, (vmo2,))
        self.assertEqual(p2.vmos_and_their_ancestors, (vmo1, vmo2))

        self.assertEqual(data.capture.vmos_by_name[""], [vmo2, vmo3])
        self.assertEqual(data.capture.vmos_by_name["Vmo1"], [vmo1])
        self.assertEqual(data.capture.process_by_koid[2], p2)
        self.assertEqual(data.capture.vmo_by_koid[1], vmo1)


if __name__ == "__main__":
    unittest.main()
