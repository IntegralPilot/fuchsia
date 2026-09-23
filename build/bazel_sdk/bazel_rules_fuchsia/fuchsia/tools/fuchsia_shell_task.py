#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import subprocess
from shutil import which

from fuchsia.tools.fuchsia_task_lib import *


class FuchsiaShellTask(FuchsiaTask):
    def try_resolve(self, executable: str) -> str:
        result = Path(which(executable) or "").resolve()
        return (
            str(result)
            if result.is_file() and os.access(result, os.X_OK)
            else executable
        )

    def run(self, parser: ScopedArgumentParser) -> None:
        executable, *arguments = parser.get_default_arguments()
        command = [self.try_resolve(executable), *arguments]
        try:
            # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
            # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
            # inheritance which can confuse child Python processes.
            env = dict(os.environ)
            env.pop("RUNFILES_DIR", None)
            env.pop("RUNFILES_MANIFEST_FILE", None)

            subprocess.check_call(" ".join(command), shell=True, env=env)
        except subprocess.SubprocessError:
            raise TaskExecutionException(f"Shell task {command} failed.")


if __name__ == "__main__":
    FuchsiaShellTask.main()
