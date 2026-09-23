# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("./common.star", "FORMATTER_MSG", "cipd_platform_name", "get_fuchsia_dir", "os_exec")

def _python_format(ctx):
    """Formats Python code using autoflake, isort and black on a Python code base.

    Args:
      ctx: A ctx instance.
    """
    py_files = ctx.scm.affected_files(glob = ["*.py", "!third_party", "!*_pb2.py"])
    if not py_files:
        return

    # Format tools make conflicting code style changes. To ensure consistent formatting:
    # 1. Run Autoflake to remove unused imports and variables.
    # 2. Run isort to sort imports on the autoflake formatted code.
    # 3. Run black on the isort formatted code to enforce its code style guidelines.
    fuchsia_dir = get_fuchsia_dir(ctx)
    platform = cipd_platform_name(ctx)
    python_bin = "%s/prebuilt/third_party/python3/%s/bin/python3" % (
        fuchsia_dir,
        platform,
    )

    autoflake_path = "%s/third_party/pylibs/autoflake/main.py" % fuchsia_dir
    isort_path = "%s/third_party/pylibs/isort/main.py" % fuchsia_dir
    black_path = "%s/prebuilt/third_party/black/%s/black" % (
        fuchsia_dir,
        platform,
    )
    pyproject_toml = "%s/pyproject.toml" % fuchsia_dir

    procs = []
    for f in py_files:
        procs.append(
            (
                f,
                os_exec(
                    ctx,
                    [
                        python_bin,
                        "scripts/shac/python_format.py",
                        "--python",
                        python_bin,
                        "--autoflake",
                        autoflake_path,
                        "--isort",
                        isort_path,
                        "--black",
                        black_path,
                        "--pyproject-toml",
                        pyproject_toml,
                        f,
                    ],
                    raise_on_failure = False,
                ),
            ),
        )

    errors = []
    for filepath, proc in procs:
        original = str(ctx.io.read_file(filepath))
        res = proc.wait()
        if res.retcode != 0:
            errors.append("python_format failed on %s:\n%s" % (filepath, res.stderr))
            continue
        formatted = res.stdout
        if formatted != original:
            ctx.emit.finding(
                level = "error",
                message = FORMATTER_MSG,
                filepath = filepath,
                replacements = [formatted],
            )

    if errors:
        fail("\n".join(errors))

def _py_shebangs(ctx):
    """Validates that all Python script shebangs specify the vendored Python interpeter.

    Scripts can opt out of this by adding a comment with
    "allow-non-vendored-python" in a line after the shebang.
    """
    ignore_paths = (
        "build/bazel/",
        "build/bazel_sdk/",
        "infra/",
        "integration/",
        "vendor/",
        "third_party/",
    )
    for path in ctx.scm.affected_files(glob = "*.py"):
        if path.startswith(ignore_paths):
            continue
        lines = str(ctx.io.read_file(path, 4096)).splitlines()
        if not lines:
            continue
        first_line = lines[0]
        want_shebang = "#!/usr/bin/env fuchsia-vendored-python"
        if first_line.startswith("#!") and first_line != want_shebang:
            if len(lines) > 1 and lines[1].startswith("# allow-non-vendored-python"):
                continue
            ctx.emit.finding(
                level = "warning",
                message = "Use fuchsia-vendored-python in shebangs for Python scripts.",
                filepath = path,
                line = 1,
                replacements = [want_shebang + "\n"],
            )

def register_python_checks():
    shac.register_check(shac.check(_python_format, formatter = True))
    shac.register_check(_py_shebangs)
