#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Extracts the copyright header from a .proto file and prepends it to the generated Python
bindings.

protoc's Python generator does not automatically carry over copyright headers from the
source .proto file into the generated bindings. This script extracts the header manually
(converting `//` comments into `#` comments) and appends a `# type: ignore` directive
before wrapping the generated bindings, so that checked-in goldens contain the proper
copyright header and type-checkers skip them.
"""

import os
import sys


def main(argv):
    assert len(argv) == 3, "Incorrect number of arguments"

    candidate_path, output_path, proto_path = argv

    header = ""

    if os.path.exists(proto_path):
        with open(proto_path, "r") as f:
            for line in f:
                if line.startswith("//"):
                    header += "#" + line[2:]
                elif not line.strip():
                    header += line
                else:
                    break

    if "Copyright" not in header:
        # Default fallback.
        header = (
            "# Copyright 2026 The Fuchsia Authors. All rights reserved.\n"
            "# Use of this source code is governed by a BSD-style license that can be\n"
            "# found in the LICENSE file."
        )

    # Add a `# type: ignore` directive so that type checkers like mypy skip the
    # generated bindings, since protoc's generated Python output often fails
    # strict type checking.
    header = header.rstrip() + "\n\n# type: ignore\n\n"

    with open(output_path, "w") as out_f, open(candidate_path, "r") as in_f:
        out_f.write(header)
        out_f.write(in_f.read())


if __name__ == "__main__":
    main(sys.argv[1:])
