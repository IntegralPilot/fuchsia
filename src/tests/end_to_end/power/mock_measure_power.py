#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import argparse
import csv
import signal
import time
from typing import Any


class MockMeasurePower:
    def __init__(self, csv_out: str) -> None:
        self.csv_out = csv_out
        self.keep_going = True

    def stop_now(self, *unused_args: Any) -> None:
        self.keep_going = False

    def start(self) -> None:
        signal.signal(signal.SIGINT, self.stop_now)
        signal.signal(signal.SIGTERM, self.stop_now)
        with open(self.csv_out, "w", newline="") as csvfile:
            fieldnames = ["Timestamp", "Current", "Voltage"]
            writer = csv.DictWriter(csvfile, fieldnames=fieldnames)
            writer.writeheader()
            csvfile.flush()
            i = 0
            while self.keep_going:
                writer.writerow(
                    {
                        "Timestamp": time.time_ns(),
                        "Current": 100 + (i % 10),
                        "Voltage": 12,
                    }
                )
                i += 1
                csvfile.flush()
                time.sleep(1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        prog="mock_measure_power",
        description="Mocks measurepower binary provided by infra for local execution",
    )

    parser.add_argument(
        "-format", "--out_format", help="ignored, we only support csv"
    )
    parser.add_argument(
        "-out", "--csv_out", help="defines the file to output csv to"
    )
    args = parser.parse_args()

    app = MockMeasurePower(args.csv_out)
    app.start()
