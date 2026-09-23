# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import ipaddress
import json
import logging
import os
import pathlib
import stat
import statistics
import subprocess
from collections.abc import Awaitable
from enum import Enum
from importlib.resources import as_file, files
from typing import Any, Callable

import fuchsia_base_test
import test_data
from honeydew.transports.ffx.ffx import FFX
from mobly import asserts, test_runner
from perf_publish import publish
from reporting import metrics
from trace_processing import trace_importing
from trace_processing.metrics import cpu

# The TCP/UDP port number that the Fuchsia side will listen on.
LISTEN_PORT: int = 9001

_LOGGER: logging.Logger = logging.getLogger(__name__)


class Protocol(Enum):
    TCP = 1
    UDP = 2

    @classmethod
    def from_str(cls, value: str) -> "Protocol":
        value = value.lower()
        if value == "tcp":
            return Protocol.TCP
        if value == "udp":
            return Protocol.UDP
        raise ValueError(f"Invalid Protocol variant string: {value}")

    def __str__(self) -> str:
        if self == Protocol.TCP:
            return "tcp"
        if self == Protocol.UDP:
            return "udp"
        raise ValueError("Unknown Protocol variant")


class Direction(Enum):
    DEVICE_TO_HOST = 1
    HOST_TO_DEVICE = 2
    LOOPBACK = 3

    @classmethod
    def from_str(cls, value: str) -> "Direction":
        value = value.lower()
        if value == "send":
            return Direction.DEVICE_TO_HOST
        if value == "recv":
            return Direction.HOST_TO_DEVICE
        if value == "loopback":
            return Direction.LOOPBACK
        raise ValueError(f"Invalid Direction variant string: {value}")

    def __str__(self) -> str:
        if self == Direction.DEVICE_TO_HOST:
            return "send"
        if self == Direction.HOST_TO_DEVICE:
            return "recv"
        if self == Direction.LOOPBACK:
            return "loopback"
        raise ValueError("Unknown Direction variant")


class Stats:
    def __init__(
        self,
        protocol: Protocol,
        direction: Direction,
        netstack3: bool,
        message_size: int,
        flows: int,
    ) -> None:
        self._protocol = protocol
        self._direction = direction
        self._netstack3 = netstack3
        self._message_size = message_size
        self._flows = flows
        self._throughputs: list[float] = []
        self._packets: int = 0
        self._lost_packets: int = 0
        self._jitter_weighted: float = 0.0

    def add(self, iperf_results: dict[str, Any]) -> None:
        setup = iperf_results["start"]["test_start"]
        # Verify iperf parameters are as we'd expect.
        asserts.assert_equal(setup["protocol"], str(self._protocol).upper())
        asserts.assert_equal(setup["blksize"], self._message_size)

        end = iperf_results["end"]
        if self._protocol == Protocol.TCP:
            if self._direction == Direction.DEVICE_TO_HOST:
                self._throughputs.append(end["sum_sent"]["bits_per_second"])
            else:
                self._throughputs.append(end["sum_received"]["bits_per_second"])
            return
        if self._direction == Direction.DEVICE_TO_HOST:
            self._throughputs.append(end["sum"]["bits_per_second"])
            return
        # For UDP, there is no sum_received record, but we gather the
        # receiver information from server-output.
        receiver = iperf_results["server_output_json"]["end"]["sum"]
        # TODO(https://github.com/esnet/iperf/issues/754): Remove the following
        # once iperf calculates throughput correctly when the server is the
        # receiver. In the meantime, derive the value from the other stats.
        self._throughputs.append(
            (self._message_size * 8) * receiver["packets"] / receiver["seconds"]
        )
        self._packets += receiver["packets"]
        self._lost_packets += receiver["lost_packets"]
        # Note that in order to compute the average jitter of packets
        # across all flows, the jitter for each flow must be weighted by
        # the packet count of said flow to produce the total jitter across
        # all packets, which when divided by the number of packets yields
        # the correct statistic.
        self._jitter_weighted += receiver["packets"] * receiver["jitter_ms"]

    def results(self, cpu_percentages: list[float]) -> list[dict[str, Any]]:
        asserts.assert_equal(self._flows, len(self._throughputs))
        label: str = f"{str(self._protocol).upper()}/{self._direction}/{self._message_size}bytes"
        if self._flows > 1:
            label += f"/{self._flows}flows"
        results: list[dict[str, Any]] = []
        throughput: float = sum(self._throughputs)
        results.append(
            generate_result(
                label, "bits_per_second", self._netstack3, [throughput]
            )
        )
        results.append(
            generate_result(label, "CPU", self._netstack3, cpu_percentages)
        )
        if (
            self._protocol != Protocol.TCP
            and self._direction != Direction.DEVICE_TO_HOST
        ):
            results.append(
                generate_result(
                    label, "lost_packets", self._netstack3, [self._lost_packets]
                )
            )
            results.append(
                generate_result(
                    label,
                    "lost_percent",
                    self._netstack3,
                    [
                        self._lost_packets
                        / (self._lost_packets + self._packets)
                        * 100
                    ],
                )
            )
            results.append(
                generate_result(
                    label,
                    "jitter_ms",
                    self._netstack3,
                    [self._jitter_weighted / self._packets],
                )
            )
        if self._flows > 1:
            std_dev = statistics.stdev(self._throughputs)
            mean = statistics.mean(self._throughputs)
            results.append(
                generate_result(
                    label,
                    "bits_per_second_coefficient_of_variation",
                    self._netstack3,
                    [std_dev / mean * 100],
                )
            )
        return results


UNIT_MAP = {
    "bits_per_second": "bits/second",
    "bits_per_second_coefficient_of_variation": "percent",
    "lost_packets": "count_smallerIsBetter",
    "lost_percent": "percent",
    "jitter_ms": "milliseconds",
    "CPU": "percent",
}


def generate_result(
    label: str, key: str, netstack3: bool, values: list[Any]
) -> dict[str, Any]:
    unit: str = UNIT_MAP.get(key, "unknown")
    return {
        "label": f"{label}/{key}",
        "test_suite": (
            "fuchsia.netstack.iperf_benchmarks.netstack3"
            if netstack3
            else "fuchsia.netstack.iperf_benchmarks"
        ),
        "unit": unit,
        "values": values,
    }


class IperfServer:
    def __init__(self, ffx: FFX) -> None:
        self._process: subprocess.Popen[bytes] = ffx.popen(
            ["target", "ssh", f"iperf3 --server --port {LISTEN_PORT} --json"],
            text=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    def dump_output_to_file(self, path: str | os.PathLike[str]) -> None:
        self._process.kill()
        output, err = self._process.communicate()
        if err:
            _LOGGER.warning(f"Server wrote errors: {err!r}")
        # NOTE: this file contains a set of JSON objects (not a list of objects, just a bunch of
        # JSON objects). The first one is the one we used to check that the connection had been
        # established. Consider removing that one as it's a test implementation detail.
        with open(path, "wb") as f:
            f.write(output)


class NetstackIperfTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_test(self) -> None:
        await super().setup_test()
        self._protocol = Protocol.from_str(self.user_params["protocol"])
        self._direction = Direction.from_str(self.user_params["direction"])
        self._netstack3 = self.user_params["netstack"] == "netstack3"
        self._label = self.user_params["label"]
        if self._netstack3:
            self._label += ".netstack3"

    async def test_iperf(self) -> None:
        await self._wait_system_metrics_daemon_start()
        await self._run_iperf_client_tests()

    async def _wait_system_metrics_daemon_start(self) -> None:
        for _ in range(10):
            async with self.dut.tracing.trace_session(
                categories=["system_metrics"],
                download=True,
                directory=self.test_case_path,
                trace_file="trace.fxt",
            ):
                # Record a 1-second trace session to give the system metrics daemon a chance to
                # emit CPU usage trace event(s), which it typically does every second.
                await asyncio.sleep(1)
            cpu_results = self._get_cpu_results(
                os.path.join(self.test_case_path, "trace.fxt")
            )
            if len(cpu_results) > 0:
                return
        raise RuntimeError(
            "Failed to retrieve CPU stats from system_metrics daemon"
        )

    async def _run_iperf_client_tests(self) -> None:
        results: list[dict[str, Any]] = []
        MESSAGE_SIZES = [64, 1024, 1400]
        if self._direction != Direction.LOOPBACK:
            server = await self._start_iperf3_server()
            try:
                for message_size in MESSAGE_SIZES:
                    results += await self._run_iperf_client_test_case(
                        self._run_ethernet_tests, message_size, 1
                    )
            finally:
                dir = pathlib.Path(self.test_case_path)
                self._cleanup_iperf_tasks()
                server.dump_output_to_file(dir / f"iperf_server.json")
        else:
            for message_size in MESSAGE_SIZES:
                for flows in [1, 2, 4]:
                    results += await self._run_iperf_client_test_case(
                        self._run_loopback_tests, message_size, flows
                    )

        path = os.path.join(
            self.test_case_path, "netstack_iperf_results.fuchsiaperf.json"
        )
        with open(path, "w") as f:
            json.dump(results, f)
        publish.publish_fuchsiaperf(
            [path],
            f"fuchsia.netstack.iperf_benchmarks.{self._label}.txt",
            test_data_module=test_data,
        )

    async def _run_iperf_client_test_case(
        self,
        test: Callable[[pathlib.Path, int, int], Awaitable[list[str]]],
        message_size: int,
        flows: int,
    ) -> list[dict[str, Any]]:
        dir = pathlib.Path(self.test_case_path) / (
            f"{message_size}bytes_{flows}flow" + ("s" if flows > 1 else "")
        )
        os.makedirs(dir)

        async with self.dut.tracing.trace_session(
            categories=["system_metrics"],
            download=True,
            directory=str(dir),
            trace_file="trace.fxt",
        ):
            result_files = await test(dir, message_size, flows)

        cpu_results = self._get_cpu_results(dir / "trace.fxt")
        asserts.assert_equal(len(cpu_results), 1)
        cpu_percentages = list(cpu_results[0].values)
        return self._iperf_results_to_fuchsiaperf(
            result_files,
            cpu_percentages,
            message_size,
        )

    async def _run_loopback_tests(
        self, dir: pathlib.Path, message_size: int, flows: int
    ) -> list[str]:
        test_component_args = [
            "--protocol",
            f"{self._protocol}",
            "--message-size",
            f"{message_size}",
            "--flows",
            f"{flows}",
        ]
        self.dut.ffx.run_test_component(
            "fuchsia-pkg://fuchsia.com/iperf-benchmark#meta/iperf-benchmark-component.cm",
            ffx_test_args=[
                "--output-directory",
                str(dir),
            ],
            test_component_args=test_component_args,
            capture_output=False,
        )
        result_files = [str(path) for path in dir.rglob(f"iperf_client_*.json")]
        asserts.assert_equal(len(result_files), flows)
        for path in result_files:
            # We want only the last JSON object (starts with a line with
            # just an opening brace) in the client output as there may be
            # output from failed attempts to start the client.
            with open(path, "r") as file:
                lines = file.readlines()
            found_last_object = False
            for i in range(len(lines) - 1, -1, -1):
                if lines[i].strip() == "{":
                    found_last_object = True
                    with open(path, "w") as file:
                        file.writelines(lines[i::])
                    break
            asserts.assert_true(
                found_last_object,
                "client JSON output should have a line with only {",
            )
        return result_files

    async def _run_ethernet_tests(
        self, dir: pathlib.Path, message_size: int, flows: int
    ) -> list[str]:
        asserts.assert_equal(flows, 1)
        ssh_address = self.dut.ffx.get_target_ssh_address()
        assert ssh_address is not None
        return [
            await self._execute_iperf3_commands(
                # NOTE(https://fxbug.dev/42124566): Currently, we are using the link used for
                # ssh to also inject data traffic. This is prone to interference to ssh and to
                # the tests. Ideally we would use a separate usb-ethernet interface for the test
                # traffic.
                ssh_address.ip,
                message_size,
                dir,
            )
        ]

    def _get_cpu_results(
        self, path: str | os.PathLike[str]
    ) -> list[metrics.TestCaseResult]:
        processor = cpu.CpuMetricsProcessor(aggregates_only=False)
        model = trace_importing.create_model_from_trace_file_path(
            path, patterns=processor.event_patterns
        )
        return list(processor.process_metrics(model))

    async def _start_iperf3_server(self) -> IperfServer:
        server = IperfServer(self.dut.ffx)
        while True:
            try:
                output = self.dut.ffx.run_ssh_cmd(
                    cmd=f"iperf3 -n 1 -c 127.0.0.1 -p {LISTEN_PORT}",
                )
                asserts.assert_not_in(
                    "iperf3: error - unable to connect to server: Connection refused",
                    output,
                )
                output = output.strip()
                asserts.assert_true(
                    output.startswith(
                        f"Connecting to host 127.0.0.1, port {LISTEN_PORT}"
                    ),
                    "output has expected beginning",
                )
                asserts.assert_in(
                    f"connected to 127.0.0.1 port {LISTEN_PORT}", output
                )
                asserts.assert_true(
                    output.endswith("iperf Done."), "output has expected end"
                )
                return server
            except Exception:  # pylint: disable=broad-except
                await asyncio.sleep(0.1)

    async def _execute_iperf3_commands(
        self,
        server_ip: ipaddress.IPv4Address | ipaddress.IPv6Address,
        message_size: int,
        output_dir: str | os.PathLike[str],
    ) -> str:
        protocol_option: str = "--udp" if self._protocol == Protocol.UDP else ""
        dir_option: str = (
            "--reverse" if self._direction == Direction.DEVICE_TO_HOST else ""
        )
        command_args = [
            "--client",
            f"{server_ip}",
            "--length",
            f"{message_size}",
            "--json",
            protocol_option,
            "--bitrate",
            # NOTE(https://fxbug.dev/42124566): Until we define separate link for ssh and
            # data, enforce a < 1Gbps rate. If/when the linked bug is resolved, this can be
            # changed to '0' which means as much as the system and link can transmit.
            "100M",
            dir_option,
            "--get-server-output",
            "--time",
            "5",
        ]

        cmd_args = command_args + ["--port", str(LISTEN_PORT)]
        result_path = os.path.join(output_dir, f"iperf_client.json")
        await self._run_host_iperf3_command(cmd_args, result_path)
        return result_path

    async def _run_host_iperf3_command(
        self,
        cmd_args: list[str],
        results_path: str,
    ) -> None:
        # Run iperf3 client from the host-tools
        with as_file(files(test_data).joinpath("iperf3")) as host_bin:
            host_bin.chmod(host_bin.stat().st_mode | stat.S_IEXEC)
            process = await asyncio.create_subprocess_exec(
                str(host_bin),
                *cmd_args,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await process.communicate()
            stdout_str = stdout.decode("utf-8")
            stderr_str = stderr.decode("utf-8")

            asserts.assert_equal(
                process.returncode,
                0,
                f"output: {stdout_str} stderr: {stderr_str}",
            )
            with open(results_path, "wb") as f:
                f.write(stdout)

    def _iperf_results_to_fuchsiaperf(
        self,
        result_files: list[str],
        cpu_percentages: list[float],
        message_size: int,
    ) -> list[dict[str, Any]]:
        flows: int = len(result_files)
        stats: Stats = Stats(
            self._protocol,
            self._direction,
            self._netstack3,
            message_size,
            flows,
        )
        for result_file in result_files:
            with open(result_file, "r") as f:
                iperf_results = json.load(f)
            stats.add(iperf_results)
        return stats.results(cpu_percentages)

    def _cleanup_iperf_tasks(self) -> None:
        try:
            self.dut.ffx.run_ssh_cmd(
                cmd="killall iperf3",
            )
        except Exception:  # pylint: disable=broad-except
            # killall returns -1 and prints "no tasks found" in its output
            # when there's no tasks to kill.
            pass


if __name__ == "__main__":
    test_runner.main()
