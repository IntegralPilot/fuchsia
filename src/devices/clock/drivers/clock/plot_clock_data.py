# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import re
import sys
from collections import defaultdict
from typing import Any

import matplotlib.pyplot as plt


def parse_inspect_data(
    filepath: str,
) -> defaultdict[Any, dict[str, list[Any]]]:
    """
    Parses the clock inspect data file.
    """
    clocks: defaultdict[Any, dict[str, list[Any]]] = defaultdict(
        lambda: {"rates": [], "states": []}
    )
    current_clock_id = None
    current_section = None
    last_time = None

    print(f"Parsing file: {filepath}")

    try:
        with open(filepath, "r") as f:
            for i, line in enumerate(f):
                line = line.strip()
                match = re.match(r"^(clock_rates|clock_states):(.+):0x.+", line)
                if match:
                    current_section = match.group(1)
                    current_clock_id = match.group(2)
                    continue

                time_match = re.search(r"@time = (\d+)", line)
                if time_match:
                    last_time = int(time_match.group(1))

                value_match = re.search(r"value = (\d+)", line)
                if (
                    value_match
                    and last_time is not None
                    and current_clock_id is not None
                ):
                    value = int(value_match.group(1))
                    if current_section == "clock_rates":
                        clocks[current_clock_id]["rates"].append(
                            (last_time, value)
                        )
                    elif current_section == "clock_states":
                        clocks[current_clock_id]["states"].append(
                            (last_time, value)
                        )
                    last_time = None

    except FileNotFoundError:
        print(f"Error: File not found at {filepath}", file=sys.stderr)
        sys.exit(1)

    for clock_id in clocks:
        clocks[clock_id]["rates"].sort()
        clocks[clock_id]["states"].sort()

    return clocks


def plot_clock_data(
    clocks: defaultdict[Any, dict[str, list[Any]]],
    output_filename: str,
    zoom_ms: Any = None,
) -> None:
    """
    Plots the parsed clock data.
    """
    num_clocks = len(clocks)
    if num_clocks == 0:
        print("No clock data found to plot.")
        return

    # Find the global maximum time across all clocks to align the plots.
    global_max_time = 0
    global_min_time = float("inf")
    has_any_data = False
    for clock_id in clocks:
        all_times_for_clock = [t for t, v in clocks[clock_id]["rates"]] + [
            t for t, v in clocks[clock_id]["states"]
        ]
        if all_times_for_clock:
            has_any_data = True
            global_max_time = max(global_max_time, max(all_times_for_clock))
            global_min_time = min(global_min_time, min(all_times_for_clock))

    if not has_any_data:
        print("No time data found in any clock.")
        return

    # Calculate a global end time for the plot that extends beyond the last data point.
    total_duration = (
        global_max_time - global_min_time
        if global_max_time > global_min_time
        else 100
    )
    plot_end_time = global_max_time + total_duration * 0.05

    fig, axes = plt.subplots(
        num_clocks, 1, figsize=(15, 4 * num_clocks), sharex=True, squeeze=False
    )
    axes = axes.flatten()

    for ax, clock_id in zip(axes, sorted(clocks.keys())):
        rates = clocks[clock_id]["rates"]
        states = clocks[clock_id]["states"]

        ax.set_title(f"{clock_id}")
        ax.set_ylabel("Rate (Hz)")
        ax.grid(True, which="both", linestyle="--", linewidth=0.5)
        ax.ticklabel_format(style="plain")

        all_times = sorted(
            list(set([t for t, v in rates] + [t for t, v in states]))
        )

        if not all_times:
            print(f"  Skipping clock {clock_id}: no time points.")
            continue

        rate_idx = 0
        state_idx = 0
        current_rate = 0
        current_state = 0
        last_rate = 0

        # Loop through segments defined by all_times
        for i in range(len(all_times)):
            t_start = all_times[i]
            # Use the global plot_end_time for the last segment of each clock.
            t_end = (
                all_times[i + 1] if i + 1 < len(all_times) else plot_end_time
            )

            # Save previous rate to draw vertical line
            last_rate = current_rate

            # Find current rate and state at t_start
            while rate_idx < len(rates) and rates[rate_idx][0] <= t_start:
                current_rate = rates[rate_idx][1]
                rate_idx += 1

            while state_idx < len(states) and states[state_idx][0] <= t_start:
                current_state = states[state_idx][1]
                state_idx += 1

            color = "g" if current_state == 1 else "r"

            # Plot vertical line from previous rate to current rate at t_start
            if i > 0 and current_rate != last_rate:
                ax.plot(
                    [t_start, t_start],
                    [last_rate, current_rate],
                    color=color,
                    linewidth=2,
                )

            # Plot horizontal line for the segment
            ax.plot(
                [t_start, t_end],
                [current_rate, current_rate],
                color=color,
                linewidth=2,
            )

    if zoom_ms is not None:
        axes[-1].set_xlim(0, zoom_ms)

    axes[-1].set_xlabel("Boot Time (ms)")
    fig.suptitle("Clock Rates and States Over Time", fontsize=16)
    plt.tight_layout(rect=(0, 0.03, 1, 0.97))
    plt.savefig(output_filename)
    print(f"Plot saved to {output_filename}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Plot clock inspect data.")
    parser.add_argument(
        "input_filepath", help="Path to the clock_inspect.txt file"
    )
    parser.add_argument(
        "output_filepath", help="Path to the clock_graphs.png file"
    )
    parser.add_argument(
        "--zoom", type=int, help="Zoom into the first N milliseconds of data."
    )
    args = parser.parse_args()

    clocks_data = parse_inspect_data(args.input_filepath)
    plot_clock_data(clocks_data, args.output_filepath, zoom_ms=args.zoom)
