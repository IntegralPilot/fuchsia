#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""fuchsia_fortunes.py prints a random fortune each time.

Example usage: run this script once per ssh login.
"""
import random
import sys
from typing import Sequence

# Guidelines:
# Keep entries short, typically one or two lines.
# Use links.
_FORTUNE_SET = [
    # sections by topic:
    # General
    "Learn general productivity tips from go/engfortunes.",
    "Learn Fuchsia productivity tricks one tidbit at a time: go/fuchsia-fortunes.",
    # Build
    """Looking for 'fx build' support? go/fuchsia-build-chat""",
    """Slow build?  File a report at go/fx-build-slow.
Include a link to data from 'fx report-last-build'.""",
    """Accelerate builds with remote execution and caching:
https://fuchsia.dev/internal/intree/concepts/remote-builds""",
    """Profile memory and network usage of builds with 'fx build-profile enable'""",
    """Use `fx -i` to automatically re-build/test when a file changes.""",
    """Set `NINJA_STATUS_MAX_COMMANDS=N` in your environment to show the N longest-running build actions.
Set `NINJA_STATUS_REFRESH_MILLIS=t` to change the refresh rate.""",
    """Set `FX_BUILD_RBE_STATS=1` in your environment to remote execution statistics after each build.""",
    """Publish build results to ResultStore (go/fxbtx) by setting in args.gn:
  bazel_upload_build_events = \"resultstore\" """,
    # fx
    """Help with fx commands: fx help COMMAND""",
    """Build service authentication errors?  Troubleshoot with `fx rbe auth`.""",
    """`fx repro BBID` prints instructions on how to reproduce a build from infra.""",
    """`fx format-code` reformats changed code.  Do this before `jiri upload`.""",
    """`fx test-remote` runs tests in infra with locally built artifacts.
    go/fx-test-remote-docs for more details.""",
    """`fx doc-preview` (Googler only) lets you preview documentation on fuchsia.dev.""",
    # ffx
    # editors
    # Infra
    """go/fuchsia-builders lists all builders in Fuchsia infra.""",
    """go/tq-cq-q shows the number of CQ attempts in progress.""",
    """go/fuchsia-rbe-weather shows the RBE backend load.""",
    """General infra requests: go/fuchsia-infra-bug""",
    """Need a new third-party package in CIPD?  go/fuchsia-new-3pp""",
    """Request open-source code license reviews at go/osrbugs""",
    # Gerrit
    # jiri
    # testing
    # debugging
    # add more helpful entries here...
]


def main(argv: Sequence[str]) -> int:
    print(random.choice(_FORTUNE_SET))
    return 0


if __name__ == "__main__":
    main(sys.argv[1:])
