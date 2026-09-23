#!/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generate the python_editions_defaults.py file."""

import argparse
import tempfile
import subprocess
import sys

from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output", type=Path, required=True, help="Output file path"
    )
    parser.add_argument(
        "--protoc", type=Path, required=True, help="Protoc compiler path"
    )
    parser.add_argument(
        "--descriptor-proto",
        type=Path,
        required=True,
        help="Path to input descriptor.proto file",
    )
    parser.add_argument(
        "--template-file",
        type=Path,
        required=True,
        help="Path to input template file.",
    )
    parser.add_argument("--edition-defaults-minimum", default="PROTO2")
    parser.add_argument("--edition-defaults-maximum", default="2023")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as build_dir:
        # Compile descriptor.proto into a FileDescriptorSet binary proto file.
        # See https://protobuf.dev/programming-guides/techniques/#self-description

        # Generate python_editions_defaults.binpb
        binpb_path = Path(build_dir) / "python_editions_defaults.binpb"

        subprocess.check_call(
            [
                str(args.protoc),
                "--edition_defaults_minimum",
                args.edition_defaults_minimum,
                "--edition_defaults_maximum",
                args.edition_defaults_maximum,
                "--proto_path",
                str(args.descriptor_proto.parent),
                str(args.descriptor_proto),
                "--edition_defaults_out",
                str(binpb_path),
            ]
        )

        # Read template file, then replace DEFAULTS_VALUE with the octal
        # representation of the binary file. The expansion returns the same
        # result than the action created by the embed_edition_defaults() Bazel
        # rule in protobuf/editions/defaults.bzl
        binpb_bytes = binpb_path.read_bytes()
        expansion = ""
        for b in binpb_bytes:
            if b == 34:
                expansion += '\\"'
            elif b >= 32 and b < 127:
                expansion += chr(b)
            elif b == 10:
                expansion += "\\n"
            else:
                expansion += "\\{0:03o}".format(b)

        template_content = args.template_file.read_text()
        key = "DEFAULTS_VALUE"
        pos = template_content.find(key)
        assert (
            pos >= 0
        ), f"Invalid content in {args.template_file}: missing {key}"

        expanded = (
            template_content[:pos]
            + expansion
            + template_content[pos + len(key) :]
        )

        args.output.write_text(expanded)

    return 0


if __name__ == "__main__":
    sys.exit(main())
