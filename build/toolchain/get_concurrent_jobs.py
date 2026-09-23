#!/usr/bin/env fuchsia-vendored-python
# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script computes the number of concurrent links we want to run in the
# build as a function of the machine.

from __future__ import print_function

import argparse
import json
import multiprocessing
import os
import re
import subprocess
import sys

UNITS = {"B": 1, "KB": 2**10, "MB": 2**20, "GB": 2**30, "TB": 2**40}


def parse_size(string: str) -> int:
    i = next(i for (i, c) in enumerate(string) if not c.isdigit())
    number = string[:i].strip()
    unit = string[i:].strip()
    return int(float(number) * UNITS[unit])


def parse_job(value: str) -> tuple[str, int]:
    (k, v) = value.split("=", 1)
    return (k, parse_size(v))


def get_total_memory() -> float:
    if sys.platform.startswith("linux"):
        if os.path.exists("/proc/meminfo"):
            with open("/proc/meminfo") as meminfo:
                memtotal_re = re.compile(r"^MemTotal:\s*(\d*)\s*kB")
                for line in meminfo:
                    match = memtotal_re.match(line)
                    if match:
                        return float(match.group(1)) * 2**10
    elif sys.platform == "darwin":
        try:
            return int(subprocess.check_output(["sysctl", "-n", "hw.memsize"]))
        except Exception:
            return 0
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--memory-per-job", type=parse_job, default=[], nargs="*"
    )
    parser.add_argument("--reserve-memory", type=parse_size, default=0)
    args = parser.parse_args()

    mem_total_bytes = max(0, get_total_memory() - args.reserve_memory)
    try:
        cpu_cap = multiprocessing.cpu_count()
    except:
        cpu_cap = 1

    concurrent_jobs = {}
    for job, memory_per_job in args.memory_per_job:
        num_concurrent_jobs = int(max(1, mem_total_bytes / memory_per_job))
        concurrent_jobs[job] = min(num_concurrent_jobs, cpu_cap)

    print(json.dumps(concurrent_jobs))

    return 0


if __name__ == "__main__":
    sys.exit(main())
