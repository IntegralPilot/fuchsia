# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Generator for Starnix line discipline traces.

This script runs scenarios defined in `scenarios.py` against a PTY pair and records the
interaction to a JSON file. These traces are then used by the `replayer` test runner
to verify the Starnix line discipline implementation.
"""

import argparse
import fcntl
import json
import logging
import os
import select
import struct
import termios
import time
from typing import Any, Dict, List, Optional, Union

# Configure logging
logging.basicConfig(level=logging.INFO, format="%(message)s")
logger = logging.getLogger(__name__)

# Patch termios.IUTF8 if missing (e.g. in prebuilt python environments)
if not hasattr(termios, "IUTF8"):
    # Start bit for IUTF8 in Linux is 0o40000 (16384)
    termios.IUTF8 = 0o40000

# LFLAGS naming is a bit inconsistent (ICANON, ISIG, ECHO...).
# Let's just list common ones we care about to avoid noise.

INTERESTING_IFLAGS = [
    "IGNBRK",
    "BRKINT",
    "IGNPAR",
    "PARMRK",
    "INPCK",
    "ISTRIP",
    "INLCR",
    "IGNCR",
    "ICRNL",
    "IUCLC",
    "IXON",
    "IXANY",
    "IXOFF",
    "IMAXBEL",
    "IUTF8",
]
INTERESTING_OFLAGS = [
    "OPOST",
    "OLCUC",
    "ONLCR",
    "OCRNL",
    "ONOCR",
    "ONLRET",
    "OFILL",
    "OFDEL",
]
INTERESTING_LFLAGS = [
    "ISIG",
    "ICANON",
    "XCASE",
    "ECHO",
    "ECHOE",
    "ECHOK",
    "ECHONL",
    "ECHOCTL",
    "ECHOPRT",
    "ECHOKE",
    "DEFECHO",
    "FLUSHO",
    "NOFLSH",
    "TOSTOP",
    "PENDIN",
    "IEXTEN",
]


def decompose_flags(value: int, names: List[str]) -> List[str]:
    """Decomposes a flag integer into a list of symbolic names."""
    result = []
    for name in names:
        if hasattr(termios, name):
            flag = getattr(termios, name)
            if (value & flag) == flag:
                result.append(name)
    return result


def get_termios_dict(fd: int) -> Dict[str, Any]:
    """Reads current termios from fd and returns a dict representation."""
    iflag, oflag, cflag, lflag, ispeed, ospeed, cc = termios.tcgetattr(fd)

    return {
        "c_iflag": decompose_flags(iflag, INTERESTING_IFLAGS),
        "c_oflag": decompose_flags(oflag, INTERESTING_OFLAGS),
        "c_lflag": decompose_flags(lflag, INTERESTING_LFLAGS),
        # c_cflag and c_cc can remain as is or be improved later if needed
        # For line discipline, iflag/oflag/lflag are most critical.
    }


def set_termios(fd: int, config: Dict[str, Any]) -> None:
    """Sets termios on fd from a dict representation."""
    iflag = 0
    for name in config.get("c_iflag", []):
        if hasattr(termios, name):
            iflag |= getattr(termios, name)

    oflag = 0
    for name in config.get("c_oflag", []):
        if hasattr(termios, name):
            oflag |= getattr(termios, name)

    lflag = 0
    for name in config.get("c_lflag", []):
        if hasattr(termios, name):
            lflag |= getattr(termios, name)

    # Default cflag if not provided
    cflag = termios.CS8 | termios.CREAD | termios.B38400

    # Get current to preserve others
    current = termios.tcgetattr(fd)
    current_cc = current[6]
    # Make a mutable copy (list)
    cc = list(current_cc)

    if "c_cc" in config:
        for name, value in config["c_cc"].items():
            if hasattr(termios, name):
                idx = getattr(termios, name)
                if idx < len(cc):
                    cc[idx] = value
                else:
                    logger.warning("Index %d out of range for c_cc", idx)
            else:
                logger.warning("Unknown c_cc name %s", name)

    termios.tcsetattr(
        fd,
        termios.TCSANOW,
        [iflag, oflag, cflag, lflag, current[4], current[5], cc],
    )


def encode_data(data: Union[str, bytes, List[int]]) -> Union[str, List[int]]:
    """Encodes data for JSON serialization."""
    # Try to encode as string if valid utf-8, otherwise list of ints
    if isinstance(data, str):
        return data
    try:
        if isinstance(data, list):
            # Already a list (e.g. from previous load?), just return or check content
            return data
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return list(data)


class ScenarioRunner:
    """Runs a single scenario using a PTY pair."""

    def __init__(self, scenario: Dict[str, Any]):
        self.scenario = scenario
        self.master_fd: Optional[int] = None
        self.slave_fd: Optional[int] = None
        self.recorded_events: List[Dict[str, Any]] = []

    def run(self) -> Dict[str, Any]:
        """Runs the scenario and returns the result dict."""
        self.master_fd, self.slave_fd = os.openpty()
        try:
            self._setup_pty()
            self._run_events()
            final_termios = get_termios_dict(self.slave_fd)
        finally:
            if self.master_fd is not None:
                os.close(self.master_fd)
            if self.slave_fd is not None:
                os.close(self.slave_fd)

        return {
            "name": self.scenario["name"],
            "initial_termios": self.scenario.get("initial_termios", {}),
            "events": self.recorded_events,
            "final_termios": final_termios,
        }

    def _setup_pty(self) -> None:
        # Set non-blocking
        for fd in [self.master_fd, self.slave_fd]:
            fl = fcntl.fcntl(fd, fcntl.F_GETFL)
            fcntl.fcntl(fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)

        if "initial_termios" in self.scenario:
            set_termios(self.slave_fd, self.scenario["initial_termios"])

        if "window_size" in self.scenario:
            ws = self.scenario["window_size"]
            winsize = struct.pack(
                "HHHH", ws.get("ws_row", 24), ws.get("ws_col", 80), 0, 0
            )
            fcntl.ioctl(self.slave_fd, termios.TIOCSWINSZ, winsize)

    def _run_events(self) -> None:
        events = self.scenario.get("events", [])
        for evt in events:
            action = evt["action"]
            if action == "write_to_master":
                self._handle_write(self.master_fd, evt, "write_to_master")
            elif action == "write_to_slave":
                self._handle_write(self.slave_fd, evt, "write_to_slave")
            elif action == "read_from_master":
                self._handle_read(self.master_fd, evt, "read_from_master")
            elif action == "read_from_slave":
                self._handle_read(self.slave_fd, evt, "read_from_slave")
            elif action == "set_packet_mode":
                self._handle_set_packet_mode(
                    self.master_fd, evt, "set_packet_mode"
                )
            elif action == "set_termios":
                self._handle_set_termios(self.slave_fd, evt, "set_termios")
            elif action == "flush":
                self._handle_flush(evt, "flush")
            elif action == "wait_until_readable":
                self._handle_wait_until_readable(evt, "wait_until_readable")
            elif action == "sleep":
                time.sleep(evt.get("duration", 0.05))

    def _handle_set_packet_mode(
        self, fd: int, evt: Dict[str, Any], event_type: str
    ) -> None:
        enabled = evt["enabled"]

        TIOCPKT = getattr(termios, "TIOCPKT", 0x5420)
        mode = struct.pack("i", 1 if enabled else 0)
        fcntl.ioctl(fd, TIOCPKT, mode)
        self.recorded_events.append({"type": event_type, "enabled": enabled})

    def _handle_set_termios(
        self, fd: int, evt: Dict[str, Any], event_type: str
    ) -> None:
        set_termios(fd, evt["termios"])
        self.recorded_events.append(
            {"type": event_type, "termios": evt["termios"]}
        )

    def _handle_flush(self, evt: Dict[str, Any], event_type: str) -> None:
        side = evt["side"]
        queue_selector_str = evt["queue_selector"]

        if side == "main":
            fd = self.master_fd
        elif side == "replica":
            fd = self.slave_fd
        else:
            raise ValueError(f"Unknown side: {side}")

        if queue_selector_str == "TCIFLUSH":
            queue_selector = termios.TCIFLUSH
        elif queue_selector_str == "TCOFLUSH":
            queue_selector = termios.TCOFLUSH
        elif queue_selector_str == "TCIOFLUSH":
            queue_selector = termios.TCIOFLUSH
        else:
            raise ValueError(f"Unknown queue_selector: {queue_selector_str}")

        termios.tcflush(fd, queue_selector)
        self.recorded_events.append(
            {
                "type": event_type,
                "side": side,
                "queue_selector": queue_selector_str,
            }
        )

    def _handle_wait_until_readable(
        self, evt: Dict[str, Any], event_type: str
    ) -> None:
        side = evt["side"]
        if side == "main":
            fd = self.master_fd
        elif side == "replica":
            fd = self.slave_fd
        else:
            raise ValueError(f"Unknown side: {side}")

        # Wait up to 2 seconds for the file descriptor to become readable.
        select.select([fd], [], [], 2.0)
        self.recorded_events.append({"type": event_type, "side": side})

    def _handle_write(
        self, fd: int, evt: Dict[str, Any], event_type: str
    ) -> None:
        data_in = evt["data"]
        data_bytes = (
            data_in.encode("utf-8")
            if isinstance(data_in, str)
            else bytes(data_in)
        )
        expect_block = evt.get("expect_block", False)

        start_time = time.time()
        written = False
        while True:
            try:
                os.write(fd, data_bytes)
                written = True
                break
            except BlockingIOError:
                if expect_block:
                    break
                if time.time() - start_time > 1.0:
                    break  # Timeout
                time.sleep(0.01)

        if expect_block:
            if written:
                self.recorded_events.append(
                    {
                        "type": f"{event_type}_unexpected_success",
                        "data": encode_data(data_bytes),
                    }
                )
            else:
                self.recorded_events.append(
                    {
                        "type": f"{event_type}_blocked",
                        "data": encode_data(data_bytes),
                    }
                )
        else:
            if not written:
                # Master write shouldn't usually block
                raise BlockingIOError(f"{event_type} blocked unexpectedly")
            self.recorded_events.append(
                {"type": event_type, "data": encode_data(data_bytes)}
            )

    def _handle_read(
        self, fd: int, evt: Dict[str, Any], event_type: str
    ) -> None:
        # Wait for data to be available
        select.select([fd], [], [], 0.1)

        total_data = b""
        while True:
            try:
                chunk = os.read(fd, 4096)
                if not chunk:
                    break
                total_data += chunk
            except BlockingIOError:
                break

        # If we expected data but got none, check if we should have waited longer?
        # The original code just passed.

        if total_data:
            self.recorded_events.append(
                {
                    "type": event_type,
                    "data": encode_data(total_data),
                }
            )


def main():
    parser = argparse.ArgumentParser(
        description="Generate Starnix line discipline traces."
    )

    group = parser.add_mutually_exclusive_group(required=False)
    group.add_argument(
        "--generate-trace",
        action="store_true",
        help="Generate a single trace file from a scenario input",
    )
    group.add_argument(
        "--generate-rs",
        action="store_true",
        help="Generate scenarios.rs from a list of scenario names",
    )

    parser.add_argument(
        "--input", help="Input scenario JSON file (for --generate-trace)"
    )
    parser.add_argument(
        "--output",
        help="Output file path (trace JSON or scenarios.rs)",
    )
    parser.add_argument(
        "--trace-dir",
        help="Directory containing trace files (for --generate-rs)",
    )
    parser.add_argument(
        "--names", nargs="+", help="List of scenario names (for --generate-rs)"
    )

    args = parser.parse_args()

    # Locate the script directory
    script_dir = os.path.dirname(os.path.abspath(__file__))

    # Default to manual mode if no action specified
    if not args.generate_trace and not args.generate_rs:
        scenarios_list_path = os.path.join(script_dir, "scenarios_list.json")

        if not os.path.exists(scenarios_list_path):
            raise FileNotFoundError(
                f"Could not find scenarios_list.json at {scenarios_list_path}"
            )

        with open(scenarios_list_path, "r") as f:
            scenarios = json.load(f)

        logger.info(f"Found {len(scenarios)} scenarios.")

        output_dir = os.path.join(script_dir, "generated")
        os.makedirs(output_dir, exist_ok=True)

        # Cleanup stale files
        expected_files = {f"{name}.json" for name in scenarios}
        for filename in os.listdir(output_dir):
            if filename not in expected_files:
                file_path = os.path.join(output_dir, filename)
                if os.path.isfile(file_path):
                    logger.info(f"Removing stale trace: {filename}")
                    os.remove(file_path)

        scenarios_dir = os.path.join(script_dir, "scenarios")

        for name in scenarios:
            input_path = os.path.join(scenarios_dir, f"{name}.json")
            output_path = os.path.join(output_dir, f"{name}.json")

            if not os.path.exists(input_path):
                logger.warning(f"Scenario input not found: {input_path}")
                continue

            logger.info(f"Generating {name}...")
            with open(input_path, "r") as f:
                scenario_data = json.load(f)

            # Handle list vs dict
            if isinstance(scenario_data, list):
                scenario_data = scenario_data[0]

            runner = ScenarioRunner(scenario_data)
            result = runner.run()

            with open(output_path, "w") as f:
                json.dump(result, f, indent=4)
                f.write("\n")

        logger.info(f"Done. Traces written to {output_dir}")
        return

    if args.generate_trace:
        if not args.input:
            parser.error("--input is required for --generate-trace")
        if not args.output:
            parser.error("--output is required for --generate-trace")

        with open(args.input, "r") as f:
            scenario = json.load(f)

        if isinstance(scenario, list):
            if len(scenario) != 1:
                raise ValueError("Expected single scenario in input file")
            scenario = scenario[0]

        runner = ScenarioRunner(scenario)
        result = runner.run()

        with open(args.output, "w") as f:
            json.dump(result, f, indent=4)
            f.write("\n")

    elif args.generate_rs:
        if not args.names:
            parser.error("--names is required for --generate-rs")
        if not args.output:
            parser.error("--output is required for --generate-rs")

        with open(args.output, "w") as f:
            f.write(
                "// Copyright 2026 The Fuchsia Authors. All rights reserved.\n"
            )
            f.write(
                "// Use of this source code is governed by a BSD-style license that can be\n"
            )
            f.write("// found in the LICENSE file.\n\n")
            f.write("// Generated by trace_generator.py. DO NOT EDIT.\n\n")
            f.write("#[cfg(test)]\n")
            f.write("mod tests {\n")
            f.write("    use test_case::test_case;\n\n")

            # Sort names to ensure stable output
            for name in sorted(args.names):
                trace_content = ""
                if args.trace_dir:
                    trace_path = os.path.join(args.trace_dir, f"{name}.json")
                    with open(trace_path, "r") as tf:
                        trace_content = tf.read().strip()

                if not trace_content:
                    raise ValueError(
                        f"Trace content for scenario '{name}' is empty or missing. "
                        f"Did you forget to run trace_generator.py manually? "
                        f"See src/starnix/lib/line_discipline/testing/README.md for instructions."
                    )

                f.write(
                    f'    #[test_case("{name}", r####"{trace_content}"####; "{name}")]\n'
                )

            f.write("    fn test_replay_trace(name: &str, json_data: &str) {\n")
            f.write(
                "        line_discipline::testing::test_replay_trace(name, json_data);\n"
            )
            f.write("    }\n")
            f.write("}\n")


if __name__ == "__main__":
    main()
