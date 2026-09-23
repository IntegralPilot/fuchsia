#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import concurrent.futures
import datetime
import ipaddress
import json
import logging
import os
import platform
import random
import re
import signal
import socket
import string
import subprocess
import time
import traceback
import zipfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

import fuchsia_async_extension
from libs.proc import job
from libs.proc.runner import CalledProcessError, Runner
from mobly import signals

if TYPE_CHECKING:
    from antlion.controllers.android_device import AndroidDevice
    from antlion.controllers.fuchsia_device import FuchsiaDevice
    from libs.ssh.connection import SshConnection

# File name length is limited to 255 chars on some OS, so we need to make sure
# the file names we output fits within the limit.
MAX_FILENAME_LEN = 255

# All Fuchsia devices use this suffix for link-local mDNS host names.
FUCHSIA_MDNS_TYPE = "_fuchsia._udp.local."

# Default max seconds it takes to Duplicate Address Detection to finish before
# assigning an IPv6 address.
DAD_TIMEOUT_SEC = 30


class ActsUtilsError(Exception):
    """Generic error raised for exceptions in ACTS utils."""


ascii_letters_and_digits = string.ascii_letters + string.digits
valid_filename_chars = f"-_.{ascii_letters_and_digits}"

models = (
    "sprout",
    "occam",
    "hammerhead",
    "bullhead",
    "razor",
    "razorg",
    "shamu",
    "angler",
    "volantis",
    "volantisg",
    "mantaray",
    "fugu",
    "ryu",
    "marlin",
    "sailfish",
)

manufacture_name_to_model = {
    "flo": "razor",
    "flo_lte": "razorg",
    "flounder": "volantis",
    "flounder_lte": "volantisg",
    "dragon": "ryu",
}

GMT_to_olson = {
    "GMT-9": "America/Anchorage",
    "GMT-8": "US/Pacific",
    "GMT-7": "US/Mountain",
    "GMT-6": "US/Central",
    "GMT-5": "US/Eastern",
    "GMT-4": "America/Barbados",
    "GMT-3": "America/Buenos_Aires",
    "GMT-2": "Atlantic/South_Georgia",
    "GMT-1": "Atlantic/Azores",
    "GMT+0": "Africa/Casablanca",
    "GMT+1": "Europe/Amsterdam",
    "GMT+2": "Europe/Athens",
    "GMT+3": "Europe/Moscow",
    "GMT+4": "Asia/Baku",
    "GMT+5": "Asia/Oral",
    "GMT+6": "Asia/Almaty",
    "GMT+7": "Asia/Bangkok",
    "GMT+8": "Asia/Hong_Kong",
    "GMT+9": "Asia/Tokyo",
    "GMT+10": "Pacific/Guam",
    "GMT+11": "Pacific/Noumea",
    "GMT+12": "Pacific/Fiji",
    "GMT+13": "Pacific/Tongatapu",
    "GMT-11": "Pacific/Midway",
    "GMT-10": "Pacific/Honolulu",
}


def abs_path(path: str) -> str:
    """Resolve the '.' and '~' in a path to get the absolute path.

    Args:
        path: The path to expand.

    Returns:
        The absolute path of the input path.
    """
    return os.path.abspath(os.path.expanduser(path))


def get_current_epoch_time() -> int:
    """Current epoch time in milliseconds.

    Returns:
        An integer representing the current epoch time in milliseconds.
    """
    return int(round(time.time() * 1000))


def get_current_human_time() -> str:
    """Returns the current time in human readable format.

    Returns:
        The current time stamp in Month-Day-Year Hour:Min:Sec format.
    """
    return time.strftime("%m-%d-%Y %H:%M:%S ")


def epoch_to_human_time(epoch_time: int) -> str | None:
    """Converts an epoch timestamp to human readable time.

    This essentially converts an output of get_current_epoch_time to an output
    of get_current_human_time

    Args:
        epoch_time: An integer representing an epoch timestamp in milliseconds.

    Returns:
        A time string representing the input time.
        None if input param is invalid.
    """
    if isinstance(epoch_time, int):
        try:
            d = datetime.datetime.fromtimestamp(epoch_time / 1000)
            return d.strftime("%m-%d-%Y %H:%M:%S ")
        except ValueError:
            return None


def get_timezone_olson_id() -> str:
    """Return the Olson ID of the local (non-DST) timezone.

    Returns:
        A string representing one of the Olson IDs of the local (non-DST)
        timezone.
    """
    tzoffset = int(time.timezone / 3600)
    gmt = None
    if tzoffset <= 0:
        gmt = f"GMT+{-tzoffset}"
    else:
        gmt = f"GMT-{tzoffset}"
    return GMT_to_olson[gmt]


def load_config(file_full_path: str, log_errors: bool = True) -> Any:
    """Loads a JSON config file.

    Returns:
        A JSON object.
    """
    with open(file_full_path, "r") as f:
        try:
            return json.load(f)
        except Exception as e:
            if log_errors:
                logging.error("Exception error to load %s: %s", f, e)
            raise


def rand_ascii_str(length: int) -> str:
    """Generates a random string of specified length, composed of ascii letters
    and digits.

    Args:
        length: The number of characters in the string.

    Returns:
        The random string generated.
    """
    letters = [random.choice(ascii_letters_and_digits) for i in range(length)]
    return "".join(letters)


def rand_hex_str(length: int) -> str:
    """Generates a random string of specified length, composed of hex digits

    Args:
        length: The number of characters in the string.

    Returns:
        The random string generated.
    """
    letters = [random.choice(string.hexdigits) for i in range(length)]
    return "".join(letters)


# Thead/Process related functions.
def concurrent_exec(func: Any, param_list: Any) -> list[Any]:
    """Executes a function with different parameters pseudo-concurrently.

    This is basically a map function. Each element (should be an iterable) in
    the param_list is unpacked and passed into the function. Due to Python's
    GIL, there's no true concurrency. This is suited for IO-bound tasks.

    Args:
        func: The function that parforms a task.
        param_list: A list of iterables, each being a set of params to be
            passed into the function.

    Returns:
        A list of return values from each function execution. If an execution
        caused an exception, the exception object will be the corresponding
        result.
    """
    with concurrent.futures.ThreadPoolExecutor(max_workers=30) as executor:
        # Start the load operations and mark each future with its params
        future_to_params = {executor.submit(func, *p): p for p in param_list}
        return_vals = []
        for future in concurrent.futures.as_completed(future_to_params):
            params = future_to_params[future]
            try:
                return_vals.append(future.result())
            except Exception as exc:
                print(
                    f"{params} generated an exception: {traceback.format_exc()}"
                )
                return_vals.append(exc)
        return return_vals


def exe_cmd(*cmds: Any) -> bytes:
    """Executes commands in a new shell.

    Args:
        cmds: A sequence of commands and arguments.

    Returns:
        The output of the command run.

    Raises:
        OSError is raised if an error occurred during the command execution.
    """
    cmd = " ".join(cmds)
    proc = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, shell=True
    )
    (out, err) = proc.communicate()
    if not err:
        return out
    raise OSError(err)


def require_sl4a(android_devices: list[AndroidDevice]) -> None:
    """Makes sure sl4a connection is established on the given AndroidDevice
    objects.

    Args:
        android_devices: A list of AndroidDevice objects.

    Raises:
        AssertionError is raised if any given android device does not have SL4A
        connection established.
    """
    for ad in android_devices:
        msg = f"SL4A connection not established properly on {ad.serial}."
        assert ad.droid, msg


def _assert_subprocess_running(proc: subprocess.Popen[bytes]) -> None:
    """Checks if a subprocess has terminated on its own.

    Args:
        proc: A subprocess returned by subprocess.Popen.

    Raises:
        ActsUtilsError is raised if the subprocess has stopped.
    """
    ret = proc.poll()
    if ret is not None:
        out, err = proc.communicate()
        raise ActsUtilsError(
            "Process %d has terminated. ret: %d, stderr: %s,"
            " stdout: %s" % (proc.pid, ret, str(err), str(out))
        )


def start_standing_subprocess(
    cmd: str, check_health_delay: int = 0, shell: bool = True
) -> subprocess.Popen[bytes]:
    """Starts a long-running subprocess.

    This is not a blocking call and the subprocess started by it should be
    explicitly terminated with stop_standing_subprocess.

    For short-running commands, you should use exe_cmd, which blocks.

    You can specify a health check after the subprocess is started to make sure
    it did not stop prematurely.

    Args:
        cmd: string, the command to start the subprocess with.
        check_health_delay: float, the number of seconds to wait after the
                            subprocess starts to check its health. Default is 0,
                            which means no check.

    Returns:
        The subprocess that got started.
    """
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        shell=shell,
        preexec_fn=os.setpgrp,
    )
    logging.debug("Start standing subprocess with cmd: %s", cmd)
    if check_health_delay > 0:
        time.sleep(check_health_delay)
        _assert_subprocess_running(proc)
    return proc


def stop_standing_subprocess(
    proc: subprocess.Popen[bytes], kill_signal: signal.Signals = signal.SIGTERM
) -> None:
    """Stops a subprocess started by start_standing_subprocess.

    Before killing the process, we check if the process is running, if it has
    terminated, ActsUtilsError is raised.

    Catches and ignores the PermissionError which only happens on Macs.

    Args:
        proc: Subprocess to terminate.
    """
    pid = proc.pid
    logging.debug("Stop standing subprocess %d", pid)
    _assert_subprocess_running(proc)
    try:
        os.killpg(pid, kill_signal)
    except PermissionError:
        pass


def wait_for_standing_subprocess(
    proc: subprocess.Popen[bytes], timeout: int | None = None
) -> None:
    """Waits for a subprocess started by start_standing_subprocess to finish
    or times out.

    Propagates the exception raised by the subprocess.wait(.) function.
    The subprocess.TimeoutExpired exception is raised if the process timed-out
    rather then terminating.

    If no exception is raised: the subprocess terminated on its own. No need
    to call stop_standing_subprocess() to kill it.

    If an exception is raised: the subprocess is still alive - it did not
    terminate. Either call stop_standing_subprocess() to kill it, or call
    wait_for_standing_subprocess() to keep waiting for it to terminate on its
    own.

    Args:
        p: Subprocess to wait for.
        timeout: An integer number of seconds to wait before timing out.
    """
    proc.wait(timeout)


def sync_device_time(
    ad: AndroidDevice,
) -> None:
    """Sync the time of an android device with the current system time.

    Both epoch time and the timezone will be synced.

    Args:
        ad: The android device to sync time on.
    """
    ad.adb.shell("settings put global auto_time 0", ignore_status=True)
    ad.adb.shell("settings put global auto_time_zone 0", ignore_status=True)
    droid = ad.droid
    if not droid:
        raise signals.ControllerError("missing ad.droid")
    droid.setTimeZone(get_timezone_olson_id())
    droid.setTime(get_current_epoch_time())


def set_ambient_display(ad: AndroidDevice, new_state: bool) -> None:
    """Set "Ambient Display" in Settings->Display

    Args:
        ad: android device object.
        new_state: new state for "Ambient Display". True or False.
    """
    ad.adb.shell(f"settings put secure doze_enabled {1 if new_state else 0}")


def set_location_service(ad: AndroidDevice, new_state: bool) -> None:
    """Set Location service on/off in Settings->Location

    Args:
        ad: android device object.
        new_state: new state for "Location service".
            If new_state is False, turn off location service.
            If new_state if True, set location service to "High accuracy".
    """
    ad.adb.shell(
        "content insert --uri "
        " content://com.google.settings/partner --bind "
        "name:s:network_location_opt_in --bind value:s:1"
    )
    ad.adb.shell(
        "content insert --uri "
        " content://com.google.settings/partner --bind "
        "name:s:use_location_for_services --bind value:s:1"
    )
    if new_state:
        ad.adb.shell("settings put secure location_mode 3")
    else:
        ad.adb.shell("settings put secure location_mode 0")


def parse_ping_ouput(
    ad: AndroidDevice, count: int, out: str, loss_tolerance: int = 20
) -> bool:
    """Ping Parsing util.

    Args:
        ad: Android Device Object.
        count: Number of ICMP packets sent
        out: shell output text of ping operation
        loss_tolerance: Threshold after which flag test as false
    Returns:
        False: if packet loss is more than loss_tolerance%
        True: if all good
    """
    result = re.search(
        r"(\d+) packets transmitted, (\d+) received, (\d+)% packet loss", out
    )
    if not result:
        ad.log.info("Ping failed with %s", out)
        return False

    packet_loss = int(result.group(3))
    packet_xmit = int(result.group(1))
    packet_rcvd = int(result.group(2))
    min_packet_xmit_rcvd = (100 - loss_tolerance) * 0.01
    if (
        packet_loss > loss_tolerance
        or packet_xmit < count * min_packet_xmit_rcvd
        or packet_rcvd < count * min_packet_xmit_rcvd
    ):
        ad.log.error(
            "%s, ping failed with loss more than tolerance %s%%",
            result.group(0),
            loss_tolerance,
        )
        return False
    ad.log.info("Ping succeed with %s", result.group(0))
    return True


def adb_shell_ping(
    ad: AndroidDevice,
    dest_ip: str,
    count: int = 120,
    timeout: int = 200,
    loss_tolerance: int = 20,
) -> bool:
    """Ping utility using adb shell.

    Args:
        ad: Android Device Object.
        count: Number of ICMP packets to send
        dest_ip: hostname or IP address
                 default www.google.com
        timeout: timeout for icmp pings to complete.
    """
    ping_cmd = "ping -W 1"
    if count:
        ping_cmd += f" -c {count}"
    if dest_ip:
        ping_cmd += f" {dest_ip}"
    try:
        ad.log.info(
            "Starting ping test to %s using adb command %s", dest_ip, ping_cmd
        )
        out = str(ad.adb.shell(ping_cmd, timeout=timeout, ignore_status=True))
        if not parse_ping_ouput(ad, count, out, loss_tolerance):
            return False
        return True
    except Exception as e:
        ad.log.warning("Ping Test to %s failed with exception %s", dest_ip, e)
        return False


def zip_directory(zip_name: str, src_dir: str) -> None:
    """Compress a directory to a .zip file.

    This implementation is thread-safe.

    Args:
        zip_name: str, name of the generated archive
        src_dir: str, path to the source directory
    """
    with zipfile.ZipFile(zip_name, "w", zipfile.ZIP_DEFLATED) as zip:
        for root, dirs, files in os.walk(src_dir):
            for file in files:
                path = os.path.join(root, file)
                zip.write(path, os.path.relpath(path, src_dir))


def unzip_maintain_permissions(zip_path: str, extract_location: str) -> None:
    """Unzip a .zip file while maintaining permissions.

    Args:
        zip_path: The path to the zipped file.
        extract_location: the directory to extract to.
    """
    with zipfile.ZipFile(zip_path, "r") as zip_file:
        for info in zip_file.infolist():
            _extract_file(zip_file, info, extract_location)


def _extract_file(
    zip_file: zipfile.ZipFile, zip_info: zipfile.ZipInfo, extract_location: str
) -> None:
    """Extracts a single entry from a ZipFile while maintaining permissions.

    Args:
        zip_file: A zipfile.ZipFile.
        zip_info: A ZipInfo object from zip_file.
        extract_location: The directory to extract to.
    """
    out_path = zip_file.extract(zip_info.filename, path=extract_location)
    perm = zip_info.external_attr >> 16
    os.chmod(out_path, perm)


def get_command_uptime(command_regex: str) -> str:
    """Returns the uptime for a given command.

    Args:
        command_regex: A regex that matches the command line given. Must be
            pgrep compatible.
    """
    pid = job.run(f"pgrep -f {command_regex}").stdout.decode("utf-8")
    runtime = ""
    if pid:
        runtime = job.run(f'ps -o etime= -p "{pid}"').stdout.decode("utf-8")
    return runtime


def get_device_process_uptime(adb: Any, process: str | int) -> Any:
    """Returns the uptime of a device process."""
    pid = adb.shell(f"pidof {process}", ignore_status=True)
    runtime = ""
    if pid:
        runtime = adb.shell(f'ps -o etime= -p "{pid}"')
    return runtime


def is_valid_ipv4_address(address: str) -> bool:
    try:
        socket.inet_pton(socket.AF_INET, address)
    except AttributeError:  # no inet_pton here, sorry
        try:
            socket.inet_aton(address)
        except socket.error:
            return False
        return address.count(".") == 3
    except socket.error:  # not a valid address
        return False

    return True


def is_valid_ipv6_address(address: str) -> bool:
    if "%" in address:
        address = address.split("%")[0]
    try:
        socket.inet_pton(socket.AF_INET6, address)
    except socket.error:  # not a valid address
        return False
    return True


def get_interface_ip_addresses(
    comm_channel: AndroidDevice | SshConnection | FuchsiaDevice,
    interface: str,
) -> dict[str, list[str]]:
    """Gets all of the ip addresses, ipv4 and ipv6, associated with a
       particular interface name.

    Args:
        comm_channel: How to send commands to a device.  Can be ssh, adb serial,
            etc.  Must have the run function implemented.
        interface: The interface name on the device, ie eth0

    Returns:
        A list of dictionaries of the the various IP addresses:
            ipv4_private: Any 192.168, 172.16, 10, or 169.254 addresses
            ipv4_public: Any IPv4 public addresses
            ipv6_link_local: Any fe80:: addresses
            ipv6_private_local: Any fd00:: addresses
            ipv6_public: Any publicly routable addresses
    """
    # Local imports are used here to prevent cyclic dependency.
    from antlion.controllers.android_device import AndroidDevice
    from antlion.controllers.fuchsia_device import FuchsiaDevice
    from libs.ssh.connection import SshConnection

    addrs: list[str] = []

    if isinstance(comm_channel, AndroidDevice):
        addrs = str(
            comm_channel.adb.shell(
                f'ip -o addr show {interface} | awk \'{{gsub("/", " "); print $4}}\''
            )
        ).splitlines()
    elif isinstance(comm_channel, SshConnection):
        ip = comm_channel.run(["ip", "-o", "addr", "show", interface])
        addrs = [
            addr.replace("/", " ").split()[3]
            for addr in ip.stdout.decode("utf-8").splitlines()
        ]
    elif isinstance(comm_channel, FuchsiaDevice):
        for iface in fuchsia_async_extension.get_loop().run_until_complete(
            comm_channel.honeydew_fd.netstack.list_interfaces()
        ):
            if iface.name != interface:
                continue
            for ipv4_address in iface.ipv4_addresses:
                addrs.append(str(ipv4_address))
            for ipv6_address in iface.ipv6_addresses:
                addrs.append(str(ipv6_address))
    else:
        raise ValueError("Unsupported method to send command to device.")

    ipv4_private_addresses = []
    ipv4_public_addresses = []
    ipv6_link_local_addresses = []
    ipv6_private_local_addresses = []
    ipv6_public_addresses = []

    for addr in addrs:
        on_device_ip = ipaddress.ip_address(addr)
        if on_device_ip.version == 4:
            if on_device_ip.is_private:
                ipv4_private_addresses.append(str(on_device_ip))
            elif on_device_ip.is_global or (
                # Carrier private doesn't have a property, so we check if
                # all other values are left unset.
                not on_device_ip.is_reserved
                and not on_device_ip.is_unspecified
                and not on_device_ip.is_link_local
                and not on_device_ip.is_loopback
                and not on_device_ip.is_multicast
            ):
                ipv4_public_addresses.append(str(on_device_ip))
        elif on_device_ip.version == 6:
            if on_device_ip.is_link_local:
                ipv6_link_local_addresses.append(str(on_device_ip))
            elif on_device_ip.is_private:
                ipv6_private_local_addresses.append(str(on_device_ip))
            elif on_device_ip.is_global:
                ipv6_public_addresses.append(str(on_device_ip))

    return {
        "ipv4_private": ipv4_private_addresses,
        "ipv4_public": ipv4_public_addresses,
        "ipv6_link_local": ipv6_link_local_addresses,
        "ipv6_private_local": ipv6_private_local_addresses,
        "ipv6_public": ipv6_public_addresses,
    }


class AddressTimeout(signals.TestError):
    pass


class MultipleAddresses(signals.TestError):
    pass


def get_addr(
    comm_channel: AndroidDevice | SshConnection | FuchsiaDevice,
    interface: str,
    addr_type: str = "ipv4_private",
    timeout_sec: int | None = None,
) -> str:
    """Get the requested type of IP address for an interface; if an address is
    not available, retry until the timeout has been reached.

    Args:
        addr_type: Type of address to get as defined by the return value of
            utils.get_interface_ip_addresses.
        timeout_sec: Seconds to wait to acquire an address if there isn't one
            already available. If fetching an IPv4 address, the default is 3
            seconds. If IPv6, the default is 30 seconds for Duplicate Address
            Detection.

    Returns:
        A string containing the requested address.

    Raises:
        TestAbortClass: timeout_sec is None and invalid addr_type
        AddressTimeout: No address is available after timeout_sec
        MultipleAddresses: Several addresses are available
    """
    if not timeout_sec:
        if "ipv4" in addr_type:
            timeout_sec = 3
        elif "ipv6" in addr_type:
            timeout_sec = DAD_TIMEOUT_SEC
        else:
            raise signals.TestAbortClass(f'Unknown addr_type "{addr_type}"')

    timeout = time.time() + timeout_sec
    while time.time() < timeout:
        ip_addrs = get_interface_ip_addresses(comm_channel, interface)[
            addr_type
        ]
        if len(ip_addrs) > 1:
            raise MultipleAddresses(
                f'Expected only one "{addr_type}" address, got {ip_addrs}'
            )
        elif len(ip_addrs) == 1:
            return ip_addrs[0]

    raise AddressTimeout(
        f'No available "{addr_type}" address after {timeout_sec}s'
    )


def get_interface_based_on_ip(runner: Runner, desired_ip_address: str) -> str:
    """Gets the interface for a particular IP

    Args:
        comm_channel: How to send commands to a device.  Can be ssh, adb serial,
            etc.  Must have the run function implemented.
        desired_ip_address: The IP address that is being looked for on a device.

    Returns:
        The name of the test interface.

    Raises:
        RuntimeError: when desired_ip_address is not found
    """

    desired_ip_address = desired_ip_address.split("%", 1)[0]
    ip = runner.run(["ip", "-o", "addr", "show"])
    for line in ip.stdout.decode("utf-8").splitlines():
        if desired_ip_address in line:
            return line.split()[1]
    raise RuntimeError(
        f'IP "{desired_ip_address}" not found in list:\n{ip.stdout.decode("utf-8")}'
    )


def renew_linux_ip_address(runner: Runner, interface: str) -> None:
    # Ensure the interface is not empty, so we don't accidentally target _all_ interfaces.
    if not interface:
        raise ValueError("Interface name must not be empty.")

    # If dhcpcd is managing this interface (common on Raspberry Pi OS), release
    # its lease so it stops managing the interface and does not race with dhclient.
    try:
        runner.run(f"sudo dhcpcd -k {interface}")
    except CalledProcessError:
        # dhcpcd may not be installed or running on this device.
        pass

    # Ensure interface is UP.
    runner.run(f"sudo ip link set {interface} up")
    # Release any existing lease on the DHCP server and stop the running
    # dhclient daemon to prevent multiple competing daemons.
    try:
        runner.run(f"sudo dhclient -r {interface}")
    except CalledProcessError:
        # No prior dhclient process or lease to release.
        pass
    # Flush existing IP addresses to ensure stale leases from previous networks
    # are removed, preventing MultipleAddresses errors when a new lease is assigned.
    runner.run(f"sudo ip addr flush dev {interface}")
    # Negotiate a fresh lease for the current network scope and start a new daemon.
    runner.run(f"sudo dhclient {interface}")


def get_ping_command(
    dest_ip: str,
    count: int = 3,
    interval: int = 1000,
    timeout: int = 1000,
    size: int = 56,
    os_type: str = "Linux",
    additional_ping_params: str = "",
) -> str:
    """Builds ping command string based on address type, os, and params.

    Args:
        dest_ip: string, address to ping (ipv4 or ipv6)
        count: int, number of requests to send
        interval: int, time in seconds between requests
        timeout: int, time in seconds to wait for response
        size: int, number of bytes to send,
        os_type: string, os type of the source device (supports 'Linux',
            'Darwin')
        additional_ping_params: string, command option flags to
            append to the command string

    Returns:
        The ping command.
    """
    if is_valid_ipv4_address(dest_ip):
        ping_binary = "ping"
    elif is_valid_ipv6_address(dest_ip):
        ping_binary = "ping6"
    else:
        raise ValueError(f"Invalid ip addr: {dest_ip}")

    if os_type == "Darwin":
        if is_valid_ipv6_address(dest_ip):
            # ping6 on MacOS doesn't support timeout
            logging.debug(
                "Ignoring timeout, as ping6 on MacOS does not support it."
            )
            timeout_flag = []
        else:
            timeout_flag = ["-t", str(timeout / 1000)]
    elif os_type == "Linux":
        timeout_flag = ["-W", str(timeout / 1000)]
    else:
        raise ValueError("Invalid OS.  Only Linux and MacOS are supported.")

    ping_cmd = [
        ping_binary,
        *timeout_flag,
        "-c",
        str(count),
        "-i",
        str(interval / 1000),
        "-s",
        str(size),
        additional_ping_params,
        dest_ip,
    ]
    return " ".join(ping_cmd)


def ping(
    comm_channel: Runner,
    dest_ip: str,
    count: int = 3,
    interval: int = 1000,
    timeout: int = 1000,
    size: int = 56,
    additional_ping_params: str = "",
) -> PingResult:
    """Generic linux ping function, supports local (acts.libs.proc.job) and
    SshConnections (acts.libs.proc.job over ssh) to Linux based OSs and MacOS.

    NOTES: This will work with Android over SSH, but does not function over ADB
    as that has a unique return format.

    Args:
        comm_channel: communication channel over which to send ping command.
            Must have 'run' function that returns at least command, stdout,
            stderr, and exit_status (see acts.libs.proc.job)
        dest_ip: address to ping (ipv4 or ipv6)
        count: int, number of packets to send
        interval: int, time in milliseconds between pings
        timeout: int, time in milliseconds to wait for response
        size: int, size of packets in bytes
        additional_ping_params: string, command option flags to
            append to the command string

    Returns:
        Dict containing:
            command: string
            exit_status: int (0 or 1)
            stdout: string
            stderr: string
            transmitted: int, number of packets transmitted
            received: int, number of packets received
            packet_loss: int, percentage packet loss
            time: int, time of ping command execution (in milliseconds)
            rtt_min: float, minimum round trip time
            rtt_avg: float, average round trip time
            rtt_max: float, maximum round trip time
            rtt_mdev: float, round trip time standard deviation

        Any values that cannot be parsed are left as None
    """
    from libs.ssh.connection import SshConnection

    is_local = comm_channel == job  # type: ignore # Blanket ignore to enable mypy
    os_type = platform.system() if is_local else "Linux"
    ping_cmd = get_ping_command(
        dest_ip,
        count=count,
        interval=interval,
        timeout=timeout,
        size=size,
        os_type=os_type,
        additional_ping_params=additional_ping_params,
    )

    if isinstance(comm_channel, SshConnection) or is_local:
        logging.debug(
            "Running ping with parameters (count: %s, interval: %s, "
            "timeout: %s, size: %s)",
            count,
            interval,
            timeout,
            size,
        )
        try:
            ping_result: (
                subprocess.CompletedProcess[bytes] | CalledProcessError
            ) = comm_channel.run(ping_cmd)
        except CalledProcessError as e:
            ping_result = e
    else:
        raise ValueError(f"Unsupported comm_channel: {type(comm_channel)}")

    stdout = ping_result.stdout.decode("utf-8")
    stderr = ping_result.stderr.decode("utf-8")
    raw_output = stdout
    if stderr:
        raw_output += "\n" + stderr

    summary = re.search(
        "([0-9]+) packets transmitted.*?([0-9]+) received.*?([0-9]+)% packet "
        "loss.*?time ([0-9]+)",
        stdout,
    )
    rtt_stats = re.search(
        "= ([0-9.]+)/([0-9.]+)/([0-9.]+)/([0-9.]+)",
        stdout,
    )
    return PingResult(
        raw_output=raw_output,
        success=ping_result.returncode == 0,
        requested=count,
        transmitted=int(summary.group(1)) if summary else 0,
        received=int(summary.group(2)) if summary else 0,
        time_ms=float(summary.group(4)) / 1000 if summary else None,
        rtt_min_ms=float(rtt_stats.group(1)) if rtt_stats else None,
        rtt_avg_ms=float(rtt_stats.group(2)) if rtt_stats else None,
        rtt_max_ms=float(rtt_stats.group(3)) if rtt_stats else None,
        rtt_mdev_ms=float(rtt_stats.group(4)) if rtt_stats else None,
    )


@dataclass
class PingResult:
    raw_output: str
    success: bool
    requested: int
    transmitted: int
    received: int
    time_ms: float | None
    rtt_min_ms: float | None
    rtt_avg_ms: float | None
    rtt_max_ms: float | None
    rtt_mdev_ms: float | None

    @property
    def any_pings_received(self) -> bool:
        """True if at least one ping was received."""
        return self.received > 0

    @property
    def all_pings_received(self) -> bool:
        """True if all requested pings were received."""
        return (
            self.received == self.requested
            and self.transmitted == self.requested
        )


def ip_in_subnet(ip: str, subnet: str) -> bool:
    """Validate that ip is in a given subnet.

    Args:
        ip: string, ip address to verify (eg. '192.168.42.158')
        subnet: string, subnet to check (eg. '192.168.42.0/24')

    Returns:
        True, if ip in subnet, else False
    """
    return ipaddress.ip_address(ip) in ipaddress.ip_network(subnet)


def mac_address_list_to_str(mac_addr_list: bytes) -> str:
    """Converts list of decimal octets representing mac address to string.

    Args:
        mac_addr_list: list, representing mac address octets in decimal
            e.g. [18, 52, 86, 120, 154, 188]

    Returns:
        string, mac address
            e.g. '12:34:56:78:9a:bc'
    """
    # Print each octet as hex, right justified, width of 2, and fill with "0".
    return ":".join([f"{octet:0>2x}" for octet in mac_addr_list])


def get_fuchsia_mdns_ipv6_address(device_mdns_name: str) -> None | str:
    """Finds the IPv6 link-local address of a Fuchsia device matching a mDNS
    name.

    Args:
        device_mdns_name: name of Fuchsia device (e.g. gig-clone-sugar-slash)

    Returns:
        string, IPv6 link-local address
    """
    import psutil
    from zeroconf import IPVersion, Zeroconf

    if not device_mdns_name:
        return None

    def mdns_query(interface: str, address: str) -> None | str:
        logging.info(
            f'Sending mDNS query for device "{device_mdns_name}" using "{address}"'
        )
        try:
            zeroconf = Zeroconf(
                ip_version=IPVersion.V6Only, interfaces=[address]
            )
        except RuntimeError as e:
            if "No adapter found for IP address" in e.args[0]:
                # Most likely, a device went offline and its control
                # interface was deleted. This is acceptable since the
                # device that went offline isn't guaranteed to be the
                # device we're searching for.
                logging.warning(f'No adapter found for "{address}"')
                return None
            raise

        device_records = zeroconf.get_service_info(
            FUCHSIA_MDNS_TYPE, f"{device_mdns_name}.{FUCHSIA_MDNS_TYPE}"
        )

        if device_records:
            for device_address in device_records.parsed_addresses():
                device_ip_address = ipaddress.ip_address(device_address)
                scoped_address = f"{device_address}%{interface}"
                if (
                    device_ip_address.version == 6
                    and device_ip_address.is_link_local
                    and ping(job, dest_ip=scoped_address).success  # type: ignore # Blanket ignore to enable mypy
                ):
                    logging.info(
                        f'Found device "{device_mdns_name}" at "{scoped_address}"'
                    )
                    zeroconf.close()
                    del zeroconf
                    return scoped_address

        zeroconf.close()
        del zeroconf
        return None

    with ThreadPoolExecutor() as executor:
        futures = []

        interfaces = psutil.net_if_addrs()
        for interface in interfaces:
            for addr in interfaces[interface]:
                address = addr.address.split("%")[0]
                if (
                    addr.family == socket.AF_INET6
                    and ipaddress.ip_address(address).is_link_local
                    and address != "fe80::1"
                ):
                    futures.append(
                        executor.submit(mdns_query, interface, address)
                    )

        for future in futures:
            addr = future.result()
            if addr:
                return addr

    logging.error(f'Unable to find IP address for device "{device_mdns_name}"')
    return None
