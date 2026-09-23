# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import asyncio
import sys

from daemon.daemon import Daemon


def main() -> None:
    parser = argparse.ArgumentParser(description="zxdb daemon")
    parser.add_argument("--port", type=int, help="Port for DAP server")
    parser.add_argument(
        "--ready-fd", type=int, help="File descriptor to signal readiness"
    )
    parsed_args = parser.parse_args()

    daemon = Daemon(
        port=parsed_args.port,
        ready_fd=parsed_args.ready_fd,
    )
    sys.exit(asyncio.run(daemon.run()))


if __name__ == "__main__":
    main()
