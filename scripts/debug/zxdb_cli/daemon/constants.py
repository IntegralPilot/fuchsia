# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from pathlib import Path
from typing import Final

UDS_PATH: Final[Path] = Path("/tmp/fx-debug-daemon.sock")
DEFAULT_DAP_PORT: Final[int] = 15678
