# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""CDC-ECM / CDC-NCM USB Networking stress test suite.

This test suite verifies the resilience, memory stability, and long-term
reliability of Fuchsia's virtual USB Ethernet drivers (CDC-ECM and CDC-NCM)
under continuous strain, addressing b/523412025 and b/519743129.

Specifically, it exercises:
1. Sustained Packet Transfers: Verifies that intensive data and RPC traffic
   over the USB link-local Ethernet link does not cause dropped packets,
   driver lockups, or memory leaks.
2. Large File Transfer Stress: Simulates multi-megabyte high-throughput
   payload streaming across SSH over the virtual Ethernet link while stressing
   Bulk IN/OUT DMA transfer paths.
3. Varying Packet Size Stress: Exercises packet boundary handling, MTU
   thresholds, and CDC-NCM Network Transfer Block aggregation across varying
   burst sizes (from 64B up to 64KB jumbo frames).
4. Continuous FIDL Interface Polling: Verifies that continuous state querying
   over native FIDL (fuchsia.net.interfaces/State) exercises driver property
   tables and MAC/IP structures without triggering Overnet transport deadlocks.
5. Repeated USB Power Cycling: Verifies that physically cutting and restoring
   VBUS power via a hardware USB power hub allows the CDC network adapter to
   cleanly de-enumerate and automatically recover network routing upon reboot.
"""

import asyncio
import logging
import os
import subprocess
import tempfile

import fuchsia_base_test
from honeydew.affordances.connectivity.netstack.types import PortClass
from honeydew.auxiliary_devices.usb_power_hub import usb_power_hub
from honeydew.transports.ffx.types import MachineFormat
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

# 64KB chunk size for streaming payload into target shell builtins
_TRANSFER_CHUNK_SIZE_BYTES: int = 65536

# Timeout for CDC Ethernet interface and route discovery under CQ / virtualization load.
_CDC_ROUTING_TIMEOUT_SEC: float = 60.0


class CdcStressTest(fuchsia_base_test.FuchsiaBaseTest):
    """Mobly test suite verifying CDC-ECM/NCM stack resilience under strain."""

    async def setup_class(self) -> None:
        """Called once before running test cases in the class."""
        await super().setup_class()
        await self._wait_for_network_settled()

    async def _wait_for_ssh_ready(self, timeout_sec: float = 60.0) -> None:
        """Actively polls until the target SSH daemon accepts a new connection."""
        start_time = asyncio.get_running_loop().time()
        while asyncio.get_running_loop().time() - start_time < timeout_sec:
            try:
                await asyncio.to_thread(self.dut.ffx.run_ssh_cmd, ":")
                return
            except Exception:
                # Brief sleep before retrying SSH connection to avoid hammering sshd during reconnection.
                await asyncio.sleep(0.5)
        raise TimeoutError(
            f"Timed out after {timeout_sec}s waiting for target SSH daemon to accept connections."
        )

    async def _wait_for_network_settled(
        self, timeout_sec: float = 30.0
    ) -> None:
        """Actively waits until Netstack registers the CDC Ethernet interface and verifies connectivity."""
        _LOGGER.info(
            "Waiting for CDC Ethernet interface and network stack to stabilize..."
        )
        await self.dut.wait_for_online()
        self.dut.health_check()
        start_time = asyncio.get_running_loop().time()
        while asyncio.get_running_loop().time() - start_time < timeout_sec:
            try:
                interfaces = await asyncio.wait_for(
                    self.dut.netstack.list_interfaces(), timeout=5.0
                )
                cdc_ready = any(
                    iface.port_class == PortClass.ETHERNET
                    and bool(iface.ipv6_addresses or iface.ipv4_addresses)
                    for iface in interfaces
                )
                if cdc_ready:
                    await asyncio.to_thread(self.dut.ffx.run_ssh_cmd, ":")
                    _LOGGER.info(
                        "CDC Ethernet interface and SSH transport stabilized."
                    )
                    return
            except Exception:
                pass
            # Brief sleep before polling again to allow Netstack to update.
            await asyncio.sleep(0.5)
        raise TimeoutError(
            f"Timed out after {timeout_sec}s waiting for CDC Ethernet interface to stabilize."
        )

    async def _verify_cdc_routing(self) -> None:
        """Verifies via FIDL that active FFX SSH traffic traverses the USB CDC Ethernet adapter."""
        await self.dut.wait_for_online()
        cdc_ips: list[str] = []
        wlan_ips: list[str] = []
        start_time = asyncio.get_running_loop().time()
        while (
            asyncio.get_running_loop().time() - start_time
            < _CDC_ROUTING_TIMEOUT_SEC
        ):
            interfaces = await self.dut.netstack.list_interfaces()
            cdc_ips = []
            wlan_ips = []
            for iface in interfaces:
                if iface.name != "lo":
                    ip_strs = [
                        str(ip).split("/")[0].split("%")[0]
                        for ip in (iface.ipv4_addresses + iface.ipv6_addresses)
                    ]
                    if iface.port_class == PortClass.ETHERNET:
                        cdc_ips.extend(ip_strs)
                        _LOGGER.info(
                            "Verified CDC Ethernet interface '%s' (ID: %s, MAC: %s, IPs: %s)",
                            iface.name,
                            iface.id_,
                            iface.mac,
                            ip_strs,
                        )
                    elif iface.port_class == PortClass.WLAN_CLIENT:
                        wlan_ips.extend(ip_strs)
            if cdc_ips:
                break
            # Brief sleep before re-querying interfaces to allow Netstack to complete IP address assignment.
            await asyncio.sleep(0.5)

        asserts.assert_true(
            bool(cdc_ips),
            "Pre-flight check failed: No active CDC USB Ethernet IP addresses detected via FIDL.",
        )

        ssh_addr = self.dut.ffx.get_target_ssh_address()
        asserts.assert_is_not_none(
            ssh_addr,
            "Pre-flight check failed: Unable to obtain FFX SSH target address.",
        )
        assert ssh_addr is not None  # satisfy mypy typing
        raw_ssh_ip = str(ssh_addr.ip).split("%")[0]
        _LOGGER.info(
            "Active FFX SSH raw IP: %s | CDC IPs: %s | WLAN IPs: %s",
            raw_ssh_ip,
            cdc_ips,
            wlan_ips,
        )

        asserts.assert_in(
            raw_ssh_ip,
            cdc_ips,
            f"Pre-flight check failed: SSH IP '{raw_ssh_ip}' does not match CDC Ethernet ({cdc_ips}). Traffic might be leaking over WLAN ({wlan_ips})!",
        )

    # TODO(b/528457918): Add `test_vsock_sustained_packet_transfer` suite once FFX and
    # Honeydew implement host-side `TargetVsock` targeting (blocked on b/406262417 and b/346425048).
    async def test_cdc_sustained_packet_transfer(self) -> None:
        """Verifies CDC network stack under continuous packet transfer strain.

        Performs repeated intensive payload queries and data transfers across the
        link-local USB Ethernet connection to verify 0 dropped RPC frames or timeouts.
        """
        num_iterations = int(self.user_params.get("packet_iterations", 15))
        _LOGGER.info(
            "Starting CDC sustained packet transfer stress test across %d iterations.",
            num_iterations,
        )

        await self._verify_cdc_routing()

        for i in range(1, num_iterations + 1):
            _LOGGER.info(
                "Packet transfer stress iteration %d/%d", i, num_iterations
            )
            # Execute heavy data stream commands over the CDC link
            try:
                # Stream large payload over SSH across link-local virtual Ethernet IP
                large_payload = "CDC_STRAIN_TEST_" + ("X" * 10000)
                ssh_output = await asyncio.to_thread(
                    self.dut.ffx.run_ssh_cmd,
                    f"echo '{large_payload}'",
                )
            except Exception as e:
                asserts.fail(
                    f"CDC network stack failed during packet transfer iteration {i}: {e}"
                )

            asserts.assert_equal(
                ssh_output.strip(),
                large_payload,
                f"Iteration {i}: SSH packet payload mismatch over CDC network link.",
            )

        _LOGGER.info(
            "Successfully completed %d iterations of CDC packet stress.",
            num_iterations,
        )

    async def test_cdc_large_file_transfer_target_to_host(self) -> None:
        """Verifies CDC network stack under Target->Host multi-megabyte data transfers.

        Simulates high-throughput streaming by transferring multi-megabyte binary streams
        from Target to Host over SSH across the CDC USB Ethernet link while stressing Bulk IN DMA.
        """
        num_iterations = int(
            self.user_params.get("large_transfer_iterations", 5)
        )
        transfer_mb = int(self.user_params.get("transfer_size_mb", 5))
        expected_bytes = transfer_mb * 1024 * 1024
        _LOGGER.info(
            "Starting CDC Target->Host large file transfer stress test (%d MB per transfer) across %d iterations.",
            transfer_mb,
            num_iterations,
        )

        await self._verify_cdc_routing()

        for i in range(1, num_iterations + 1):
            _LOGGER.info(
                "Target->Host large file transfer stress iteration %d/%d (%d MB)",
                i,
                num_iterations,
                transfer_mb,
            )
            try:
                ssh_output = await asyncio.to_thread(
                    self.dut.ffx.run_ssh_cmd,
                    f'printf "X%{expected_bytes - 2}sX" ""',
                )
            except Exception as e:
                asserts.fail(
                    f"CDC network stack failed during Target->Host large file transfer iteration {i}: {e}"
                )

            actual_bytes = len(ssh_output)
            asserts.assert_equal(
                actual_bytes,
                expected_bytes,
                f"Iteration {i}: Transferred Target->Host payload size mismatch over CDC link (expected {expected_bytes}, got {actual_bytes}).",
            )
            _LOGGER.info(
                "Iteration %d successfully streamed %d bytes from Target->Host over CDC link.",
                i,
                actual_bytes,
            )
            await self._wait_for_ssh_ready()

        _LOGGER.info(
            "Successfully completed %d iterations of CDC Target->Host large file transfer stress.",
            num_iterations,
        )

    # TODO(b/553616205): Explore using iperf3 (similar to netstack_iperf_test.py via
    # //third_party/iperf:iperf3_pkg) for large contiguous transfers to avoid target CPU
    # parsing overhead in /boot/bin/sh on embedded ARM targets.
    async def test_cdc_large_file_transfer_host_to_target(self) -> None:
        """Verifies CDC network stack under Host->Target multi-megabyte data transfers.

        Simulates OTA payload delivery by streaming multi-megabyte binary payloads from
        Host to Target over SSH across the CDC USB Ethernet link while stressing Bulk OUT DMA.
        """
        num_iterations = int(
            self.user_params.get("large_transfer_iterations", 5)
        )
        transfer_mb = int(self.user_params.get("transfer_size_mb", 5))
        expected_bytes = transfer_mb * 1024 * 1024
        _LOGGER.info(
            "Starting CDC Host->Target large file transfer stress test (%d MB per transfer) across %d iterations.",
            transfer_mb,
            num_iterations,
        )

        await self._verify_cdc_routing()

        with tempfile.TemporaryDirectory(prefix="cdc_ota_") as tmp_dir:
            host_payload_path = os.path.join(tmp_dir, "host_payload.bin")
            _LOGGER.info(
                "Generating %d MB temporary host payload at %s",
                transfer_mb,
                host_payload_path,
            )
            # Format payload in newline-delimited no-op shell commands (": <data>\n")
            # to ensure /boot/bin/sh processes and discards incoming stream cleanly on target.
            num_chunks = expected_bytes // _TRANSFER_CHUNK_SIZE_BYTES
            payload = (
                b": " + b"X" * (_TRANSFER_CHUNK_SIZE_BYTES - 3) + b"\n"
            ) * num_chunks
            with open(host_payload_path, "wb") as f:
                f.write(payload)

            for i in range(1, num_iterations + 1):
                _LOGGER.info(
                    "Host->Target large file transfer stress iteration %d/%d (%d MB)",
                    i,
                    num_iterations,
                    transfer_mb,
                )
                try:
                    # Stream payload into /boot/bin/sh on target which executes no-op colons across all chunks
                    ffx_cmd = self.dut.ffx.generate_ffx_cmd(
                        cmd=[
                            "target",
                            "ssh",
                            "/boot/bin/sh",
                        ],
                        include_target=True,
                        machine=MachineFormat.RAW,
                    )
                    with open(host_payload_path, "rb") as f:
                        result = await asyncio.to_thread(
                            subprocess.run,
                            ffx_cmd,
                            stdin=f,
                            stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE,
                            check=True,
                        )

                    _LOGGER.info(
                        "Iteration %d successfully streamed %d bytes from Host->Target over CDC link.",
                        i,
                        expected_bytes,
                    )
                except subprocess.CalledProcessError as e:
                    err_msg = str(e)
                    if e.stderr:
                        err_msg += f" (stderr: {e.stderr.decode('utf-8', errors='replace')})"
                    asserts.fail(
                        f"CDC network stack failed during Host->Target large file transfer iteration {i}: {err_msg}"
                    )
                except Exception as e:
                    asserts.fail(
                        f"CDC network stack failed during Host->Target large file transfer iteration {i}: {e}"
                    )

                await self._wait_for_ssh_ready()

        _LOGGER.info(
            "Successfully completed %d iterations of CDC Host->Target large file transfer stress.",
            num_iterations,
        )

    async def test_cdc_varying_packet_size_stress(self) -> None:
        """Verifies CDC network stack stability across varying packet and block sizes.

        Exercises packet boundary handling, MTU thresholds, and CDC-NCM Network Transfer Block (NTB)
        aggregation by streaming ICMPv6 echo datagram bursts of varying payload sizes (from tiny 64-byte
        chunks up to 32KB jumbo frames) across the USB Ethernet link. Using a packet-oriented protocol
        ensures datagram boundaries are preserved without TCP stream re-segmentation.
        """
        # We test sizes up to 32,768 bytes (32 KB) because CDC Ethernet has an MTU of 1500.
        # Payloads larger than 1452 bytes require IPv6 fragmentation (32 KB = 23 fragments).
        # When testing larger jumbo frames (e.g., 65 KB / 46 fragments), the target's network
        # stack (Netstack3) drops ICMP datagrams exceeding its fragment reassembly queue/buffer
        # threshold. 32 KB provides robust multi-packet NTB aggregation and fragment testing
        # without exceeding stack reassembly limits.
        packet_sizes = [64, 256, 512, 1280, 1420, 4096, 16384, 32768]
        burst_count = int(self.user_params.get("varying_burst_count", 50))
        _LOGGER.info(
            "Starting CDC varying packet size stress across block sizes: %s (burst count: %d)",
            packet_sizes,
            burst_count,
        )

        await self._verify_cdc_routing()

        # Execute ICMP bursts across all block sizes in a single SSH session on target
        # to avoid repeated connection setup/teardown overhead while stressing CDC-NCM MTU/aggregation.
        sizes_str = " ".join(str(bs) for bs in packet_sizes)
        batch_script = (
            'host_ip="${SSH_CONNECTION%% *}"\n'
            f"for bs in {sizes_str}; do\n"
            f'    out=$(ping -c {burst_count} -i 200 -s $bs -t 5000 "$host_ip" 2>&1)\n'
            "    status=$?\n"
            "    if [ $status -ne 0 ]; then\n"
            '        echo "FAIL: bs=$bs status=$status output=$out"\n'
            "        exit 1\n"
            "    fi\n"
            '    case "$out" in\n'
            '        *"0% packet loss"*) : ;;\n'
            '        *) echo "FAIL: bs=$bs status=$status output=$out"\n'
            "           exit 1\n"
            "           ;;\n"
            "    esac\n"
            '    echo "PASS: bs=$bs"\n'
            "done\n"
            'echo "ALL_PACKET_SIZES_PASSED"\n'
        )

        cmd = ["target", "ssh", batch_script]
        ffx_cmd = self.dut.ffx.generate_ffx_cmd(
            cmd=cmd, include_target=True, machine=MachineFormat.RAW
        )
        try:
            result = await asyncio.to_thread(
                subprocess.run,
                ffx_cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
        except Exception as e:
            asserts.fail(
                f"CDC network stack failed during varying packet size stress: {e}"
            )

        if (
            result.returncode != 0
            or "ALL_PACKET_SIZES_PASSED" not in result.stdout
        ):
            asserts.fail(
                f"CDC network stack failed during varying packet size stress: "
                f"ICMP batch stress failed (code {result.returncode}). Stderr: {result.stderr}, Stdout: {result.stdout}"
            )

        _LOGGER.info(
            "Successfully completed CDC varying packet size stress across all block sizes: %s",
            packet_sizes,
        )

    # TODO(b/530262848): Explore a self-recovering link toggle mechanism (like a restart
    # command instead of disable/enable) that automatically recovers the link without
    # permanently locking out the USB connection, and add a dedicated link toggling test.
    async def test_cdc_interface_polling_stress(self) -> None:
        """Verifies CDC network driver stability via continuous FIDL polling.

        Note on architectural design and the Overnet Transport Catch-22:
        In test frameworks communicating from the host PC over Overnet/RCS tunneled
        through the CDC USB Ethernet interface (fe80::...), invoking destructive
        administrative teardown methods (such as fuchsia.net.interfaces.admin/Control.Disable)
        sever the underlying link-local carrier. Once the carrier drops, Overnet
        disconnects immediately, locking out any subsequent RPCs (such as Enable)
        from reaching the device.

        To rigorously verify driver IPC resilience and table management without
        severing our own transport channel, this test uses Honeydew's native FIDL
        netstack affordance (fuchsia.net.interfaces/State.GetWatcher). By continuously
        querying and inspecting full interface property tables across rapid polling
        iterations, we force the CDC network driver and netstack to continuously
        serialize, transmit, and validate MAC addresses, device class attributes,
        and online state flags under continuous RPC pressure.
        """
        num_iterations = int(
            self.user_params.get(
                "polling_iterations",
                self.user_params.get("toggle_iterations", 10),
            )
        )
        _LOGGER.info(
            "Starting CDC interface FIDL polling stress test across %d iterations.",
            num_iterations,
        )

        for i in range(1, num_iterations + 1):
            _LOGGER.info(
                "FIDL interface polling iteration %d/%d", i, num_iterations
            )
            cdc_iface_found = False
            try:
                # Query network interfaces via native FIDL netstack affordance
                # (invokes fuchsia.net.interfaces/State.GetWatcher over Overnet).
                interfaces = await self.dut.netstack.list_interfaces()
                for iface in interfaces:
                    # Filter for non-loopback interfaces with active IPv4 or IPv6 addresses (CDC Ethernet link).
                    if iface.name != "lo" and bool(
                        iface.ipv6_addresses or iface.ipv4_addresses
                    ):
                        cdc_iface_found = True
                        _LOGGER.debug(
                            "Iteration %d: Verified active CDC interface '%s' (ID: %s, MAC: %s)",
                            i,
                            iface.name,
                            iface.id_,
                            iface.mac,
                        )
                        break
            except Exception as e:
                asserts.fail(
                    f"CDC network stack failed during FIDL interface polling iteration {i}: {e}"
                )

            asserts.assert_true(
                cdc_iface_found,
                f"Iteration {i}: Failed to discover online CDC network interface via FIDL.",
            )
            # Brief pacing between polling iterations to allow netstack to service driver events.
            await asyncio.sleep(0.5)

        _LOGGER.info(
            "Successfully completed %d iterations of FIDL interface polling.",
            num_iterations,
        )

    async def test_cdc_power_cycle_stress(self) -> None:
        """Verifies CDC virtual Ethernet auto-recovery across repeated VBUS power cycles.

        Physically cuts USB VBUS power, verifies device drop from host bus, restores
        power, and confirms automatic re-enumeration and IP connection recovery.
        """
        # Only this test case needs a hub, so fail just this case rather than
        # aborting the class. We fail instead of skipping because a skipped test
        # still shows up as passing in CI/CQ, hiding the missing hub.
        hub: usb_power_hub.UsbPowerHub | None = self.dut.usb_power_hub
        assert (
            hub is not None
        ), f"USB power hub is not configured for {self.dut.device_name}."
        port: int | None = self.dut.usb_power_hub_port

        num_iterations = int(self.user_params.get("power_iterations", 3))
        disconnect_duration = int(
            self.user_params.get("disconnect_duration_sec", 5)
        )

        _LOGGER.info(
            "Starting CDC USB power cycle stress test across %d iterations.",
            num_iterations,
        )

        for i in range(1, num_iterations + 1):
            _LOGGER.info("Power cycle iteration %d/%d", i, num_iterations)
            await self.dut.wait_for_online()

            try:
                # Cut physical VBUS power via hardware USB hub
                self.dut.ffx.notify_intentional_disconnect()
                hub.power_off(port=port)
                _LOGGER.info(
                    "Powered off USB port %s. Waiting for offline...",
                    port,
                )
                await asyncio.to_thread(self.dut.wait_for_offline)

                if disconnect_duration > 0:
                    # Keep VBUS power removed for the configured duration to simulate a sustained physical disconnect.
                    await asyncio.sleep(disconnect_duration)
            finally:
                # Restore VBUS power and verify CDC Ethernet network re-enumeration
                hub.power_on(port=port)
                _LOGGER.info(
                    "Powered on USB port %s. Waiting for CDC network recovery...",
                    port,
                )
                await self.dut.wait_for_online()
                await self.dut.on_device_boot()

            # Wait for network interface and SSH transport to settle after recovery
            await self._wait_for_network_settled()

            # Verify network connectivity is fully functional after recovery
            ssh_check = await asyncio.to_thread(
                self.dut.ffx.run_ssh_cmd, "echo 'cdc_recovered'"
            )
            asserts.assert_in(
                "cdc_recovered",
                ssh_check,
                f"Iteration {i}: CDC network link failed to transmit data after power recovery.",
            )

        _LOGGER.info(
            "Successfully completed %d iterations of CDC power cycle stress.",
            num_iterations,
        )


if __name__ == "__main__":
    test_runner.main()
