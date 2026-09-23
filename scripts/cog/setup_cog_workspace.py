#!/usr/bin/env python3
# allow-non-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""This script is used to set up a cog-based workspace for Fuchsia development."""

import argparse
import logging
import os
import shlex
import sys

import logger
import preflight
import workspace


def _parse_args() -> argparse.Namespace:
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser(
        description="Set up a cog-based workspace for Fuchsia development."
    )
    parser.add_argument(
        "--no-snapshot",
        dest="snapshot",
        action="store_false",
        help="Disable snapshotting and initialize this workspace from scratch.",
    )
    logger.add_args(parser, default_log_level=logging.INFO)
    return parser.parse_args()


def main() -> int:
    """Main function to set up the cog workspace."""
    args = _parse_args()

    if not args.color:
        os.environ["NO_COLOR"] = "1"

    logger.init_logger(
        log_level=args.log_level,
        colors=args.color,
        enable_status_updates=args.enable_status_updates,
    )
    logger.log_debug(f"===== main ===== {shlex.join(sys.argv)}")

    try:
        if preflight.check_all(require_grpc_cli=args.snapshot):
            logger.log_info("All preflight checks passed!")
        else:
            return 1

        logger.emit_status("Creating workspace instance...")
        ws = workspace.Workspace()

        logger.log_debug(f"Found repository: {ws.repo_dir}")
        logger.log_debug(
            f"CartFS mount point: {ws.cartfs_instance.mount_point}"
        )

        with ws.lock():
            if not ws.has_cartfs_dir:
                ws.init_cartfs_workspace(args.snapshot)

            if ws.is_checkout_uptodate():
                logger.log_info(
                    "Workspace is up to date, no CartFS workspace sync is needed."
                )
                return 0

            ws.checkout_cartfs_to_cog_revisions()

        return 0
    except Exception:
        logger.log_exception("An unexpected error occurred:")
        logger.log_warn(
            f"To file a bug, please attach `{logger.get_log_path()}` to http://go/fuchsia-cog-bug"
        )
        return 1
    except KeyboardInterrupt:
        logger.log_error(
            "Workspace setup cancelled by user (KeyboardInterrupt)."
        )
        return 130


if __name__ == "__main__":
    sys.exit(main())
