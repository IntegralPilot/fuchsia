#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Command-line interface and entry point for testusb.

Provides argument parsing, test selection helpers, and the main() execution
function for host-driven USB gadget testing.
"""

from __future__ import annotations

import argparse
import collections.abc
import json
import logging
import os
import sys
import time

from .backend import USBTestBackend
from .ioctl import USBTestIoctlBackend
from .models import ALL_TEST_CASES, TestParams, TestStatus
from .runner import TestRunner, format_text_report, generate_json_report

_LOGGER = logging.getLogger(__name__)


def _create_backend(device: str | None = None) -> USBTestBackend:
    """Instantiate and return the USBTestIoctlBackend.

    Args:
        device: Optional path to the USB character device node.

    Returns:
        An instance of USBTestIoctlBackend configured for the device.
    """
    return USBTestIoctlBackend(device)


def _parse_loop_count(val: str) -> int:
    """Parse loop count CLI parameter string into an integer.

    Accepts positive integers, 0, or 'forever'/'infinite' (treated as 0).

    Args:
        val: Input string value.

    Returns:
        Non-negative integer representing the loop count (0 = infinite).

    Raises:
        argparse.ArgumentTypeError: If value is negative or not a valid
            number/keyword.
    """
    cleaned = val.strip().lower()
    if cleaned in ("forever", "infinite"):
        return 0
    try:
        n = int(cleaned)
        if n < 0:
            raise argparse.ArgumentTypeError(
                f"Loop count must be non-negative: {val}"
            )
        return n
    except ValueError as e:
        raise argparse.ArgumentTypeError(
            f"Invalid loop count value: {val}"
        ) from e


def parse_test_selection(selection_str: str) -> list[int]:
    """Parse test selection string into a sorted list of unique test IDs.

    Supported formats:
    - 'all' -> [0..35]
    - '0' -> [0]
    - '0,1,2' -> [0, 1, 2]
    - '0-31' -> [0..31]
    - '0-35' -> [0..35]
    - '1,2,5-10,32,33'
    - 'none' -> []

    Args:
        selection_str: Raw selection string from CLI or caller.

    Returns:
        Sorted list of unique integer test IDs.

    Raises:
        ValueError: If selection format is invalid or test IDs are out of range.
    """
    cleaned_selection = selection_str.strip().lower()
    if cleaned_selection == "none":
        return []
    if not cleaned_selection:
        raise ValueError("No test cases selected")

    min_id = min(ALL_TEST_CASES.keys())
    max_id = max(ALL_TEST_CASES.keys())

    if cleaned_selection == "all":
        return sorted(ALL_TEST_CASES.keys())

    selected: set[int] = set()
    parts = cleaned_selection.split(",")
    for part in parts:
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            bounds = part.split("-")
            if len(bounds) != 2:
                raise ValueError(f"Invalid range format: '{part}'")
            try:
                start = int(bounds[0].strip())
                end = int(bounds[1].strip())
            except ValueError:
                raise ValueError(f"Invalid integer in range: '{part}'")
            if start > end:
                raise ValueError(
                    f"Range start ({start}) cannot be greater than end "
                    f"({end}) in '{part}'"
                )
            if start < min_id or end > max_id:
                raise ValueError(
                    f"Test IDs must be in range {min_id}..{max_id}, got '{part}'"
                )
            selected.update(range(start, end + 1))
        else:
            try:
                test_id = int(part)
            except ValueError:
                raise ValueError(f"Invalid test ID: '{part}'")
            if test_id not in ALL_TEST_CASES:
                raise ValueError(
                    f"Test ID must be in range {min_id}..{max_id}, got '{test_id}'"
                )
            selected.add(test_id)

    if not selected:
        raise ValueError("No test cases selected")

    return sorted(selected)


def create_parser() -> argparse.ArgumentParser:
    """Create and configure command-line argument parser.

    Returns:
        Configured argparse.ArgumentParser for testusb CLI.
    """
    parser = argparse.ArgumentParser(
        description="USB Function Test (Gadget Zero) Host Utility",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Run all tests on the first recognized USB test device across both configs
  testusb.py -a

  # Run bidirectional loopback test on Configuration 2
  testusb.py --loopback -c 1000 -s 1024

  # Run Source/Sink bulk tests 1 and 2 on Configuration 1
  testusb.py --mode sourcesink -t 1,2 -c 5000

  # Set explicit USB configuration (1 or 2)
  testusb.py --config 2 -t 9,10

  # Run all tests using ioctl backend and output JSON
  testusb.py -a --backend ioctl --json
""",
    )

    # Selection options
    sel_group = parser.add_argument_group("Test Selection")
    sel_group.add_argument(
        "-t",
        "--tests",
        type=str,
        default="all",
        help=(
            "Test case selection (e.g. '0', '0,1,2', '0-31', 'all', "
            "'1,2,5-10'). Default: 'all'"
        ),
    )
    sel_group.add_argument(
        "-a",
        "--all",
        action="store_true",
        help=(
            "Run all test cases across both configurations (equivalent to "
            "--mode both)"
        ),
    )
    sel_group.add_argument(
        "--loopback",
        action="store_true",
        help=(
            "Run Loopback test suite on Configuration 2 (EP0 generic control "
            "tests and bulk roundtrips)"
        ),
    )
    sel_group.add_argument(
        "--list-tests",
        action="store_true",
        help="List all available test cases and exit",
    )
    sel_group.add_argument(
        "--list-devices",
        action="store_true",
        help="List all recognized USB test devices and exit",
    )

    # Mode & Configuration options
    cfg_group = parser.add_argument_group("Configuration & Mode Options")
    cfg_group.add_argument(
        "-C",
        "--config",
        "--configuration",
        type=int,
        choices=[1, 2],
        help=(
            "Explicitly set USB configuration number (1: Source/Sink, 2: "
            "Loopback)"
        ),
    )
    cfg_group.add_argument(
        "-m",
        "--mode",
        type=str,
        choices=["sourcesink", "loopback", "both", "auto"],
        default="auto",
        help=(
            "Testing mode: 'sourcesink' (Config 1), 'loopback' (Config 2), "
            "'both' (dual-configuration sweep), or 'auto'. Default: 'auto'"
        ),
    )

    # Execution options
    exec_group = parser.add_argument_group("Execution Options")
    exec_group.add_argument(
        "-l",
        "--loop",
        nargs="?",
        const=0,
        default=1,
        type=_parse_loop_count,
        help=(
            "Loop count (default: 1). If specified without value or as "
            "'forever', loops infinitely."
        ),
    )
    exec_group.add_argument(
        "-r",
        "--random",
        action="store_true",
        help="Randomize the order of test cases in each loop",
    )
    exec_group.add_argument(
        "-b",
        "--buffer-size",
        "-s",
        type=int,
        default=1024,
        help="Buffer size / transfer length in bytes (default: 1024)",
    )
    exec_group.add_argument(
        "-c",
        "--count",
        type=int,
        default=1000,
        help="Number of iterations per test (default: 1000)",
    )
    exec_group.add_argument(
        "-g",
        "--sglen",
        type=int,
        default=32,
        help="Scatter/gather entries count (default: 32)",
    )
    exec_group.add_argument(
        "-v",
        "--vary",
        type=int,
        default=1024,
        help="Vary packet size by this amount (default: 1024)",
    )
    exec_group.add_argument(
        "--timeout",
        type=int,
        default=5000,
        help="Transfer timeout in milliseconds (default: 5000)",
    )

    # Hardware & Backend options
    hw_group = parser.add_argument_group("Hardware & Backend Options")
    hw_group.add_argument(
        "-D",
        "--device",
        type=str,
        default=os.environ.get("DEVICE"),
        help=(
            "USB device path (e.g. /dev/bus/usb/001/002). Can also be set "
            "via DEVICE env var."
        ),
    )
    hw_group.add_argument(
        "--backend",
        choices=["ioctl"],
        default="ioctl",
        help="USB backend to use (default: ioctl)",
    )

    # Output options
    out_group = parser.add_argument_group("Output Options")
    out_group.add_argument(
        "-V",
        "--verbose",
        action="store_true",
        help="Enable verbose output during test execution",
    )
    out_group.add_argument(
        "--json",
        action="store_true",
        help="Output test results in JSON format to stdout",
    )
    out_group.add_argument(
        "--json-file",
        type=str,
        help="Write test results in JSON format to the specified file",
    )

    return parser


def main(argv: collections.abc.Sequence[str] | None = None) -> int:
    """Main CLI entrypoint for testusb.

    Args:
        argv: Optional command line arguments list. Defaults to sys.argv[1:].

    Returns:
        Exit code: 0 on success, 1 on test failures or errors, 130 on SIGINT.
    """
    parser = create_parser()
    args = parser.parse_args(argv)

    # Handle --list-tests
    if args.list_tests:
        print("Available USB Test Cases:")
        print(f"  {'ID':<4} {'Name':<25} {'Description'}")
        print("  " + "-" * 60)
        for tid, tc in sorted(ALL_TEST_CASES.items()):
            print(f"  {tid:<4} {tc.name:<25} {tc.description}")
        return 0

    # Handle --list-devices
    if args.list_devices:
        devices = USBTestBackend.discover_devices()
        if not devices:
            print("No recognized USB test devices found.")
        else:
            print("Recognized USB Test Devices:")
            for dev in devices:
                print(f"  {dev}")
        return 0

    # Resolve mode and test selection
    mode = args.mode
    if args.loopback:
        mode = "loopback"
    elif args.all:
        mode = "both"

    if args.tests != "all":
        test_selection = args.tests
    elif mode == "loopback":
        test_selection = "0,9,10,14,21,32,33,34,35"
    else:
        test_selection = "all"

    # Parse test selection
    try:
        test_ids = parse_test_selection(test_selection)
        params = TestParams(
            iterations=args.count,
            length=args.buffer_size,
            vary=args.vary,
            sglen=args.sglen,
            timeout_ms=args.timeout,
        )
    except ValueError as e:
        _LOGGER.error("Invalid test argument: %s", e)
        print(f"Error: {e}", file=sys.stderr)
        return 1

    # Select backend
    backend_type = "ioctl"
    try:
        backend = _create_backend(args.device)
    except Exception as e:
        _LOGGER.error("Error initializing ioctl backend: %s", e)
        print(f"Error initializing ioctl backend: {e}", file=sys.stderr)
        return 1

    # Execute tests within backend context
    start_time = time.perf_counter()
    try:
        with backend:
            runner = TestRunner(
                backend=backend,
                params=params,
                loop_count=args.loop,
                random_order=args.random,
                verbose=args.verbose,
                mode=mode,
                config=args.config,
                loopback=args.loopback,
                quiet=args.json,
            )
            results = runner.run(test_ids)
            total_duration = time.perf_counter() - start_time

            # Generate reports
            if args.json or args.json_file:
                json_data = generate_json_report(
                    results=results,
                    device_path=backend.device_path or "unknown",
                    speed=backend.speed_name,
                    backend_name=backend_type,
                    total_duration=total_duration,
                    params=params,
                )
                json_str = json.dumps(json_data, indent=2)
                if args.json:
                    print(json_str)
                if args.json_file:
                    with open(args.json_file, "w", encoding="utf-8") as f:
                        f.write(json_str)
                        f.write("\n")
            else:
                text_report = format_text_report(
                    results=results,
                    device_path=backend.device_path or "unknown",
                    speed=backend.speed_name,
                    backend_name=backend_type,
                    total_duration=total_duration,
                )
                print(text_report)

            # Return non-zero if interrupted, or any test failed/errored, or if no tests passed
            if runner.interrupted:
                return 130
            has_failures = any(
                r.status in (TestStatus.FAIL, TestStatus.ERROR) for r in results
            )
            has_passes = any(r.status == TestStatus.PASS for r in results)
            if has_failures or not has_passes:
                return 1
            return 0

    except KeyboardInterrupt:
        print("\n[!] Test execution interrupted by user.", file=sys.stderr)
        return 130
    except Exception as e:
        _LOGGER.error("Error during test execution: %s", e, exc_info=True)
        print(f"Error during test execution: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
