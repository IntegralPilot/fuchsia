#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path


def _compute_policy_hash(
    source_path: str,
    handle_unknown: str,
) -> tuple[str, str]:
    """Reads source text and returns (source_text, sha256_hash)."""
    with open(source_path, mode="rt", encoding="utf-8") as f:
        source_content = f.read()

    hasher = hashlib.sha256()
    hasher.update(f"handle_unknown={handle_unknown}\n".encode("utf-8"))
    hasher.update(b"mls=33\n")
    hasher.update(source_content.encode("utf-8"))
    return source_content, hasher.hexdigest()


def _get_fuchsia_dir() -> str:
    """Returns the Fuchsia source root directory."""
    return os.environ.get(
        "FUCHSIA_DIR",
        os.path.abspath(
            os.path.join(
                os.path.dirname(__file__), "..", "..", "..", "..", ".."
            )
        ),
    )


def _find_checkpolicy(explicit_path: str | None) -> str | None:
    """Locates a checkpolicy binary from arguments, environment, in-tree local, or PATH."""
    if (
        explicit_path is not None
        and os.path.isfile(explicit_path)
        and os.access(explicit_path, os.X_OK)
    ):
        return explicit_path
    if (
        (checkpolicy_path := os.environ.get("CHECKPOLICY_PATH")) is not None
        and os.path.isfile(checkpolicy_path)
        and os.access(checkpolicy_path, os.X_OK)
    ):
        return checkpolicy_path
    # Check default //local/checkpolicy in Fuchsia checkout
    fuchsia_dir = _get_fuchsia_dir()
    local_checkpolicy = os.path.join(fuchsia_dir, "local", "checkpolicy")
    if os.path.isfile(local_checkpolicy) and os.access(
        local_checkpolicy, os.X_OK
    ):
        return local_checkpolicy
    return shutil.which("checkpolicy")


def _write_depfile(
    depfile: str | None,
    target: str,
    hash_file: str,
    prebuilt: str,
    source: str,
) -> None:
    if depfile is None:
        return
    deps = []
    if os.path.exists(hash_file):
        deps.append(hash_file)
    if os.path.exists(prebuilt):
        deps.append(prebuilt)
    if os.path.exists(source):
        deps.append(source)
    os.makedirs(os.path.dirname(os.path.abspath(depfile)), exist_ok=True)
    with open(depfile, mode="wt", encoding="utf-8") as f:
        f.write(f"{target}: {' '.join(deps)}\n")


def _to_source_rel(path: str, source_root: str) -> str:
    """Converts a path to be relative to the Fuchsia source root if possible."""
    try:
        return os.path.relpath(os.path.abspath(path), source_root)
    except ValueError:
        return path


def _compile_policy(
    checkpolicy_bin: str,
    source_path: str,
    output_path: str,
    handle_unknown: str,
) -> None:
    """Compiles text policy to binary using checkpolicy."""
    os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
    subprocess.run(
        [
            checkpolicy_bin,
            "--mls",
            "--sort",
            "--optimize",
            "-c",
            "33",
            "--output",
            output_path,
            "--handle-unknown",
            handle_unknown,
            "-t",
            "selinux",
            source_path,
        ],
        check=True,
    )


def _verify_prebuilt(
    policy_name: str,
    output_path: str,
    prebuilt_path: str,
    hash_file_path: str,
    stored_hash: str | None,
    current_hash: str,
    source_root: str,
) -> bool:
    """Compares the compiled binary and source hash against checked-in golden prebuilts."""
    if not os.path.exists(prebuilt_path):
        print("\n" + "=" * 70, file=sys.stderr)
        print(
            f"ERROR: Prebuilt '{_to_source_rel(prebuilt_path, source_root)}' does not exist.\n\n"
            f"To acknowledge this change, run:\n"
            f"  cp {_to_source_rel(output_path, source_root)} {_to_source_rel(prebuilt_path, source_root)}\n"
            f"  cp {_to_source_rel(output_path + '.hash', source_root)} {_to_source_rel(hash_file_path, source_root)}\n\n"
            f"Or, rebuild with `update_goldens=true` set in your GN args (e.g. via `fx args`).",
            file=sys.stderr,
        )
        print("=" * 70 + "\n", file=sys.stderr)
        return False

    with open(output_path, mode="rb") as f_out, open(
        prebuilt_path, mode="rb"
    ) as f_pre:
        candidate_bytes = f_out.read()
        prebuilt_bytes = f_pre.read()
        if candidate_bytes != prebuilt_bytes or stored_hash != current_hash:
            print("\n" + "=" * 70, file=sys.stderr)
            print(
                f"ERROR: SELinux policy '{policy_name}' source fragments have been modified.\n"
                f"The compiled binary policy or hash does not match the checked-in prebuilt:\n"
                f"  Prebuilt:  {_to_source_rel(prebuilt_path, source_root)}\n"
                f"  Hash File: {_to_source_rel(hash_file_path, source_root)}\n\n"
                f"To acknowledge this change, run:\n"
                f"  cp {_to_source_rel(output_path, source_root)} {_to_source_rel(prebuilt_path, source_root)}\n"
                f"  cp {_to_source_rel(output_path + '.hash', source_root)} {_to_source_rel(hash_file_path, source_root)}\n\n"
                f"Or, rebuild with `update_goldens=true` set in your GN args (e.g. via `fx args`).",
                file=sys.stderr,
            )
            print("=" * 70 + "\n", file=sys.stderr)
            return False

    return True


def _read_stored_hash(hash_file: str) -> str | None:
    if os.path.exists(hash_file):
        with open(hash_file, mode="rt", encoding="utf-8") as f:
            return f.read().strip()
    return None


def _report_missing_checkpolicy(
    policy_name: str, stored_hash: str | None, current_hash: str
) -> None:
    print("\n" + "=" * 70, file=sys.stderr)
    print(
        f"ERROR: SELinux policy '{policy_name}' source fragments have changed,",
        file=sys.stderr,
    )
    print(
        "but 'checkpolicy' is not available in the build environment.",
        file=sys.stderr,
    )
    print(f"Checked-in hash: {stored_hash}", file=sys.stderr)
    print(f"Calculated hash: {current_hash}", file=sys.stderr)
    print(
        "\nPlease place a valid checkpolicy executable at //local/checkpolicy or",
        file=sys.stderr,
    )
    print(
        f"revert local changes to '{policy_name}' fragments.",
        file=sys.stderr,
    )
    print("=" * 70 + "\n", file=sys.stderr)


def _bless_prebuilt(
    output_path: str, prebuilt_path: str, hash_file_path: str, policy_name: str
) -> None:
    os.makedirs(os.path.dirname(prebuilt_path), exist_ok=True)
    shutil.copyfile(output_path, prebuilt_path)
    shutil.copyfile(output_path + ".hash", hash_file_path)
    print(f"Blessed '{policy_name}': updated prebuilt and hash.")


def _record_success(
    stamp: str | None,
    depfile: str | None,
    target: str,
    hash_file: str,
    prebuilt: str,
    source: str,
) -> None:
    if stamp is not None:
        Path(stamp).touch()
    _write_depfile(depfile, target, hash_file, prebuilt, source)


def _parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Verify or compile an SELinux policy."
    )
    parser.add_argument(
        "--policy-name", required=True, help="Name of the policy"
    )
    parser.add_argument(
        "--source", required=True, help="Path to input policy .conf file"
    )
    parser.add_argument(
        "--prebuilt", required=True, help="Path to checked-in prebuilt binary"
    )
    parser.add_argument(
        "--hash-file", required=True, help="Path to checked-in source hash file"
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Path to output binary policy in out-dir",
    )
    parser.add_argument(
        "--handle-unknown",
        default="deny",
        choices=["allow", "deny", "reject"],
        help="handle_unknown setting",
    )
    parser.add_argument("--checkpolicy", help="Path to checkpolicy executable")
    parser.add_argument(
        "--bless",
        action="store_true",
        help="Update checked-in prebuilt and hash in-place",
    )
    parser.add_argument(
        "--stamp", help="Optional stamp file to write on success"
    )
    parser.add_argument("--depfile", help="Path at which to write the depfile")
    return parser.parse_args(argv)


def main(argv: Sequence[str]) -> int:
    args = _parse_args(argv)
    target = args.stamp if args.stamp is not None else args.output

    # 1. Compute current source hash and read stored hash
    _, current_hash = _compute_policy_hash(args.source, args.handle_unknown)
    stored_hash = _read_stored_hash(args.hash_file)

    # 2. Fast path: sources unchanged & prebuilt exists
    if (
        stored_hash is not None
        and stored_hash == current_hash
        and os.path.exists(args.prebuilt)
    ):
        os.makedirs(os.path.dirname(args.output), exist_ok=True)
        shutil.copyfile(args.prebuilt, args.output)
        _record_success(
            args.stamp,
            args.depfile,
            target,
            args.hash_file,
            args.prebuilt,
            args.source,
        )
        return 0

    # 3. Sources changed or prebuilt missing: Look for checkpolicy
    checkpolicy_bin = _find_checkpolicy(args.checkpolicy)
    if checkpolicy_bin is None:
        _report_missing_checkpolicy(args.policy_name, stored_hash, current_hash)
        return 1

    # 4. Compile candidate binary in out-dir
    _compile_policy(
        checkpolicy_bin,
        args.source,
        args.output,
        args.handle_unknown,
    )

    # Write candidate hash file in out-dir for manual copy workflows
    with open(args.output + ".hash", mode="wt", encoding="utf-8") as f:
        f.write(f"{current_hash}\n")

    # 5. Bless or verify
    if args.bless:
        _bless_prebuilt(
            args.output, args.prebuilt, args.hash_file, args.policy_name
        )
    else:
        source_root = _get_fuchsia_dir()
        if not _verify_prebuilt(
            args.policy_name,
            args.output,
            args.prebuilt,
            args.hash_file,
            stored_hash,
            current_hash,
            source_root,
        ):
            return 1

    _record_success(
        args.stamp,
        args.depfile,
        target,
        args.hash_file,
        args.prebuilt,
        args.source,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
