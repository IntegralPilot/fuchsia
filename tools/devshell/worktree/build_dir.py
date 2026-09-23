# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import re
import shutil
from enum import Enum
from pathlib import Path


class BuildStatus(Enum):
    CONFIGURED = "Configured"
    BUILT = "Built"
    DIRTY = "Dirty"
    FAILED = "Build Failed"
    NOT_CONFIGURED = "Not Configured"
    UNKNOWN = "Unknown"


class BuildDir:
    def __init__(self, path: Path):
        self.path = Path(path).resolve()
        self.args_gn = self.path / "args.gn"
        self.ninja_stamp = self.path / "last_ninja_build_success.stamp"

    def exists(self) -> bool:
        return self.path.exists() and self.path.is_dir()

    def get_build_config(self) -> str:
        if not self.args_gn.exists():
            return "Not Configured"

        product = "unknown"
        board = "unknown"

        try:
            content = self.args_gn.read_text()
            product_match = re.search(
                r'build_info_product\s*=\s*"([^"]+)"', content
            )
            if product_match:
                product = product_match.group(1)
            else:
                pm = re.search(r'import\("//products/([^/]+)\.gni"\)', content)
                if not pm:
                    pm = re.search(
                        r'import\("//vendor/[^/]+/products/([^/]+)\.gni"\)',
                        content,
                    )
                if pm:
                    product = pm.group(1)

            board_match = re.search(
                r'build_info_board\s*=\s*"([^"]+)"', content
            )
            if board_match:
                board = board_match.group(1)
            else:
                bm = re.search(r'import\("//boards/([^/]+)\.gni"\)', content)
                if not bm:
                    bm = re.search(
                        r'import\("//vendor/[^/]+/boards/([^/]+)\.gni"\)',
                        content,
                    )
                if bm:
                    board = bm.group(1)
        except Exception:
            pass

        return f"{product}.{board}"

    def get_build_status(self) -> BuildStatus:
        if not self.args_gn.exists():
            return BuildStatus.NOT_CONFIGURED

        ninja_errors = self.path / ".ninja_errors.json"
        if ninja_errors.exists():
            try:
                if (
                    not self.ninja_stamp.exists()
                    or ninja_errors.stat().st_mtime
                    > self.ninja_stamp.stat().st_mtime
                ):
                    content = json.loads(ninja_errors.read_text())
                    if content.get("failures"):
                        return BuildStatus.FAILED
            except Exception:
                pass

        if not self.ninja_stamp.exists():
            return BuildStatus.CONFIGURED

        try:
            args_mtime = self.args_gn.stat().st_mtime
            stamp_mtime = self.ninja_stamp.stat().st_mtime
            if args_mtime > stamp_mtime:
                return BuildStatus.DIRTY
            return BuildStatus.BUILT
        except OSError:
            return BuildStatus.UNKNOWN

    def get_build_time_ago_sec(self) -> float | None:
        ninja_log = self.path / ".ninja_log"
        if not ninja_log.exists():
            return None

        try:
            import time

            mtime = ninja_log.stat().st_mtime
            diff = time.time() - mtime
            return max(0.0, diff)
        except OSError:
            return None

    def backup_args(self) -> None:
        args_gn_ref = self.path / "args.gn.ref"
        if self.args_gn.exists():
            shutil.copy2(self.args_gn, args_gn_ref)

    def restore_args(self) -> None:
        args_gn_ref = self.path / "args.gn.ref"
        if args_gn_ref.exists():
            shutil.copy2(args_gn_ref, self.args_gn)
            os.remove(args_gn_ref)
