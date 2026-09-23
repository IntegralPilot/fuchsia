#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for pluggable system samplers and coordinators."""

import shutil
import tempfile
import unittest
from collections.abc import Iterator
from pathlib import Path

import system_profiler

# --- Abstract Test Base Class with Mock /proc File System Lifecycle ---


class ProcMockTestCase(unittest.TestCase):
    """Base test case that automatically manages a hermetic mock /proc filesystem."""

    def setUp(self) -> None:
        super().setUp()
        self.test_dir = Path(tempfile.mkdtemp())
        self.proc_dir = self.test_dir / "proc"
        self.proc_dir.mkdir()

    def tearDown(self) -> None:
        shutil.rmtree(self.test_dir)
        super().tearDown()


# --- Parser Tests ---


class ParseProcStatTest(unittest.TestCase):
    def test_parse_proc_stat(self) -> None:
        lines = [
            "cpu  100 20 30 500 10 5 2 0 0 0\n",
            "intr 1000 1 2 3\n",
            "ctxt 5000\n",
            "procs_running 2\n",
            "procs_blocked 1\n",
        ]
        data = system_profiler.parse_proc_stat(lines)
        expected = system_profiler.ProcStatData(
            cpu_ticks=[100, 20, 30, 500, 10, 5, 2, 0, 0, 0],
            procs_running=2,
            procs_blocked=1,
            intr=1000,
            ctxt=5000,
        )
        self.assertEqual(data, expected)


class ParseProcMeminfoTest(unittest.TestCase):
    def test_parse_proc_meminfo(self) -> None:
        lines = [
            "MemFree:       4000000 kB\n",
            "Buffers:       50000 kB\n",
            "Cached:        1000000 kB\n",
            "SReclaimable:  200000 kB\n",
            "Shmem:         10000 kB\n",
            "SwapTotal:     8000000 kB\n",
            "SwapFree:      6000000 kB\n",
            "Dirty:         3000 kB\n",
            "Writeback:     100 kB\n",
        ]
        data = system_profiler.parse_proc_meminfo(lines)
        expected = system_profiler.MemInfoData(
            free=4000000 * 1024,
            buffers=50000 * 1024,
            cached=1190000 * 1024,
            swap_used=2000000 * 1024,
            dirty=3000 * 1024,
            writeback=100 * 1024,
        )
        self.assertEqual(data, expected)


class ParseProcVmstatTest(unittest.TestCase):
    def test_parse_proc_vmstat(self) -> None:
        lines = [
            "pgpgin 100\n",
            "pgpgout 200\n",
            "pswpin 5\n",
            "pswpout 10\n",
        ]
        data = system_profiler.parse_proc_vmstat(lines)
        expected = system_profiler.VmStatData(
            pgpgin=100 * 1024,
            pgpgout=200 * 1024,
            pswpin=5 * 4096,
            pswpout=10 * 4096,
        )
        self.assertEqual(data, expected)


class ParseFileNrTest(unittest.TestCase):
    def test_parse_file_nr(self) -> None:
        data = system_profiler.parse_file_nr("2000\t500\t100000\n")
        expected = system_profiler.FileNrData(
            allocated=2000, free_allocated=500
        )
        self.assertEqual(data, expected)


class ParseLoadavgTest(unittest.TestCase):
    def test_parse_loadavg(self) -> None:
        data = system_profiler.parse_loadavg("0.25 0.50 0.75 1/200 12345\n")
        expected = system_profiler.LoadAvgData(
            load1=0.25, load5=0.50, load15=0.75
        )
        self.assertEqual(data, expected)


class ParsePressureFileTest(unittest.TestCase):
    def test_parse_pressure_file(self) -> None:
        lines = [
            "some avg10=0.15 avg60=0.10 avg300=0.05 total=1234\n",
            "full avg10=0.05 avg60=0.02 avg300=0.01 total=432\n",
        ]
        data = system_profiler.parse_pressure_file(lines)
        expected = system_profiler.PressureData(
            some=system_profiler.PressureLine(
                avg10=0.15, avg60=0.10, avg300=0.05
            ),
            full=system_profiler.PressureLine(
                avg10=0.05, avg60=0.02, avg300=0.01
            ),
        )
        self.assertEqual(data, expected)


class ParseNetDevTest(unittest.TestCase):
    def test_parse_net_dev(self) -> None:
        lines = [
            "Inter-|   Receive                                                |  Transmit\n",
            " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
            "  eth0: 100000 1000 0 0 0 0 0 0 200000 2000 0 0 0 0 0 0\n",
            "  lo: 500 5 0 0 0 0 0 0 500 5 0 0 0 0 0 0\n",
            "  veth123: 500 5 0 0 0 0 0 0 500 5 0 0 0 0 0 0\n",  # filtered virtual
        ]
        results = system_profiler.parse_net_dev(lines)
        expected = [
            system_profiler.InterfaceNetworkData(
                ifname="eth0",
                rx_bytes=100000,
                rx_packets=1000,
                tx_bytes=200000,
                tx_packets=2000,
            )
        ]
        self.assertEqual(results, expected)


# --- Directory and Tree Helper Utility Tests ---


class GetProcessTreePidsTest(ProcMockTestCase):
    def write_mock_pid_stat(self, pid: int, ppid: int) -> None:
        pid_dir = self.proc_dir / str(pid)
        pid_dir.mkdir(parents=True, exist_ok=True)
        with open(pid_dir / "stat", "w") as f:
            f.write(f"{pid} (fake_proc) S {ppid} 0 0 0 0\n")

    def test_get_process_tree_pids(self) -> None:
        self.write_mock_pid_stat(pid=200, ppid=1)
        self.write_mock_pid_stat(pid=201, ppid=200)
        self.write_mock_pid_stat(pid=202, ppid=201)
        self.write_mock_pid_stat(pid=203, ppid=10)  # separate tree

        pids = system_profiler.get_process_tree_pids(self.proc_dir, 200)
        self.assertEqual(pids, {200, 201, 202})


class CountProcessTreeFdsTest(ProcMockTestCase):
    def write_mock_pid_fds(self, pid: int, count: int) -> None:
        fd_dir = self.proc_dir / str(pid) / "fd"
        fd_dir.mkdir(parents=True, exist_ok=True)
        for i in range(count):
            with open(fd_dir / str(i), "w") as f:
                f.write("")

    def test_count_process_tree_fds(self) -> None:
        self.write_mock_pid_fds(pid=300, count=4)
        self.write_mock_pid_fds(pid=301, count=2)

        fds_count = system_profiler.count_process_tree_fds(
            self.proc_dir, {300, 301}
        )
        self.assertEqual(fds_count, 6)


# --- Pluggable Sampler Direct Unit Tests ---


class CpuAndSystemSamplerTest(ProcMockTestCase):
    def write_mock_stat(
        self, user: int, idle: int, running: int, intr: int, ctxt: int
    ) -> None:
        content = (
            f"cpu  {user} 0 0 {idle} 0 0 0 0 0 0\n"
            f"intr {intr} 1 2 3\n"
            f"ctxt {ctxt}\n"
            f"procs_running {running}\n"
            f"procs_blocked 1\n"
        )
        with open(self.proc_dir / "stat", "w") as f:
            f.write(content)

    def test_cpu_and_system_sampler(self) -> None:
        sampler = system_profiler.CpuAndSystemSampler(self.proc_dir)

        # First sample primes states
        self.write_mock_stat(
            user=100, idle=500, running=2, intr=1000, ctxt=5000
        )
        events_list = list(sampler.sample(ts_us=0, elapsed=None))
        events = {e["name"]: e for e in events_list}

        # First tick absolute metrics
        self.assertEqual(events["processes.running"]["args"]["count"], 2)
        self.assertEqual(events["processes.blocked"]["args"]["count"], 1)
        self.assertNotIn("cpu.user", events)

        # Second sample calculates rates (elapsed is 2.0s)
        # Deltas: user=+100, idle=+100 -> total delta = 200 -> user% = 50%
        # intr: +100 -> 50/sec. ctxt: +200 -> 100/sec
        self.write_mock_stat(
            user=200, idle=600, running=3, intr=1100, ctxt=5200
        )
        events_list = list(sampler.sample(ts_us=2000000, elapsed=2.0))
        events = {e["name"]: e for e in events_list}

        self.assertEqual(events["cpu.user"]["args"]["percent"], 50.0)
        self.assertEqual(
            events["system.interrupts"]["args"]["count_per_second"], 50.0
        )
        self.assertEqual(
            events["system.context_switches"]["args"]["count_per_second"], 100.0
        )


class MemorySamplerTest(ProcMockTestCase):
    def write_mock_meminfo(self, free_kb: int, cached_kb: int) -> None:
        content = f"MemFree: {free_kb} kB\nCached: {cached_kb} kB\n"
        with open(self.proc_dir / "meminfo", "w") as f:
            f.write(content)

    def test_memory_sampler(self) -> None:
        sampler = system_profiler.MemorySampler(self.proc_dir)
        self.write_mock_meminfo(free_kb=4000, cached_kb=1000)
        events_list = list(sampler.sample(ts_us=100, elapsed=None))
        events = {e["name"]: e for e in events_list}

        self.assertEqual(events["memory.free"]["args"]["bytes"], 4000 * 1024)
        self.assertEqual(events["memory.cache"]["args"]["bytes"], 1000 * 1024)


class DiskIoAndPagingSamplerTest(ProcMockTestCase):
    def write_mock_vmstat(self, pgpin: int, pgpout: int) -> None:
        content = f"pgpgin {pgpin}\npgpgout {pgpout}\n"
        with open(self.proc_dir / "vmstat", "w") as f:
            f.write(content)

    def test_disk_io_and_paging_sampler(self) -> None:
        sampler = system_profiler.DiskIoAndPagingSampler(self.proc_dir)

        self.write_mock_vmstat(pgpin=100, pgpout=200)
        list(sampler.sample(ts_us=0, elapsed=None))

        # Deltas: pgpgin=+20 KiB (+20480 B) -> rate / 2.0 = 10240 B/s
        self.write_mock_vmstat(pgpin=120, pgpout=200)
        events_list = list(sampler.sample(ts_us=2000000, elapsed=2.0))
        events = {e["name"]: e for e in events_list}

        self.assertEqual(
            events["block.in"]["args"]["bytes_per_second"], 10240.0
        )


class FileDescriptorSamplerTest(ProcMockTestCase):
    def setUp(self) -> None:
        super().setUp()
        (self.proc_dir / "sys" / "fs").mkdir(parents=True)

    def write_mock_file_nr(self, allocated: int, free: int) -> None:
        with open(self.proc_dir / "sys/fs/file-nr", "w") as f:
            f.write(f"{allocated}\t{free}\t100000\n")

    def test_file_descriptor_sampler(self) -> None:
        sampler = system_profiler.FileDescriptorSampler(self.proc_dir)
        self.write_mock_file_nr(allocated=2000, free=500)
        events_list = list(sampler.sample(ts_us=100, elapsed=None))
        events = {e["name"]: e for e in events_list}

        self.assertEqual(events["fds.system"]["args"]["count"], 1500)


class LoadAverageSamplerTest(ProcMockTestCase):
    def write_mock_loadavg(self, load1: float) -> None:
        with open(self.proc_dir / "loadavg", "w") as f:
            f.write(f"{load1} 0.50 0.75 1/200 12345\n")

    def test_load_average_sampler(self) -> None:
        sampler = system_profiler.LoadAverageSampler(self.proc_dir)
        self.write_mock_loadavg(load1=0.25)
        events_list = list(sampler.sample(ts_us=100, elapsed=None))
        events = {e["name"]: e for e in events_list}

        self.assertEqual(events["loadavg.1min"]["args"]["load"], 0.25)


class PressureSamplerTest(ProcMockTestCase):
    def setUp(self) -> None:
        super().setUp()
        (self.proc_dir / "pressure").mkdir()

    def write_mock_pressure(self, resource: str, avg10: float) -> None:
        with open(self.proc_dir / "pressure" / resource, "w") as f:
            f.write(f"some avg10={avg10} avg60=0.10 avg300=0.05 total=1234\n")

    def test_pressure_sampler(self) -> None:
        sampler = system_profiler.PressureSampler(self.proc_dir)
        self.write_mock_pressure("cpu", avg10=0.15)
        events_list = list(sampler.sample(ts_us=100, elapsed=None))
        events = {e["name"]: e for e in events_list}

        self.assertEqual(
            events["pressure.cpu.some.avg10"]["args"]["percent"], 0.15
        )


class NetworkSamplerTest(ProcMockTestCase):
    def setUp(self) -> None:
        super().setUp()
        (self.proc_dir / "net").mkdir()

    def write_mock_net_dev(self, rx_bytes: int, tx_bytes: int) -> None:
        content = (
            "Inter-|   Receive                                                |  Transmit\n"
            " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n"
            f"  eth0: {rx_bytes} 1000 0 0 0 0 0 0 {tx_bytes} 2000 0 0 0 0 0 0\n"
        )
        with open(self.proc_dir / "net/dev", "w") as f:
            f.write(content)

    def test_network_sampler(self) -> None:
        sampler = system_profiler.NetworkSampler(self.proc_dir)

        self.write_mock_net_dev(rx_bytes=100000, tx_bytes=200000)
        list(sampler.sample(ts_us=0, elapsed=None))

        # Deltas: rx=+50000 -> / 2.0s = 25000 B/s. tx=+80000 -> / 2s = 40000 B/s
        self.write_mock_net_dev(rx_bytes=150000, tx_bytes=280000)
        events_list = list(sampler.sample(ts_us=2000000, elapsed=2.0))
        events = {e["name"]: e for e in events_list}

        self.assertEqual(
            events["network.eth0.rx.bytes"]["args"]["bytes_per_second"], 25000.0
        )
        self.assertEqual(
            events["network.eth0.tx.bytes"]["args"]["bytes_per_second"], 40000.0
        )


# --- Pluggable Coordinator Engine Direct Test ---


class SystemProfilerCoordinatorTest(unittest.TestCase):
    def test_coordinator(self) -> None:
        # A simple stateful mock clock closure
        clock_ticks = [1000.0, 1002.0]

        def mock_time() -> float:
            return clock_ticks.pop(0)

        # Mock a fake pluggable sampler
        class FakeSampler(system_profiler.MetricSampler):
            def sample(
                self, ts_us: int, elapsed: float | None
            ) -> Iterator[system_profiler.TraceEvent]:
                yield system_profiler.make_counter_event(
                    "fake.metric",
                    "custom",
                    ts_us,
                    "count",
                    42 if elapsed else 0,
                )

        samplers = [FakeSampler()]
        profiler = system_profiler.SystemProfiler(
            samplers=samplers,
            metadata={"UUID": "abc"},
            time_fn=mock_time,
        )

        # First tick metadata
        first_events = list(profiler.sample(ts_us=0))
        self.assertEqual(len(first_events), 2)
        self.assertEqual(first_events[0]["name"], "system_profiler_metadata")
        self.assertEqual(first_events[1]["name"], "fake.metric")
        self.assertEqual(first_events[1]["args"]["count"], 0)

        # Second tick drives samplers with computed elapsed rate (1002 - 1000 = 2s)
        second_events = list(profiler.sample(ts_us=2000000))
        self.assertEqual(len(second_events), 1)
        self.assertEqual(second_events[0]["name"], "fake.metric")
        self.assertEqual(second_events[0]["args"]["count"], 42)


if __name__ == "__main__":
    unittest.main()
