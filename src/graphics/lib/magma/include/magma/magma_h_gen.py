#!/usr/bin/env fuchsia-vendored-python
# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import re
import sys
import textwrap


def usage():
    print(
        "Usage:\n"
        "  magma_h_gen.py INPUT OUTPUT\n"
        "    INPUT    json file containing the magma interface definition\n"
        "    OUTPUT   destination path for the magma header file to generate\n"
        "  Example: ./magma_h_gen.py ./magma.json ./magma.h\n"
        "  Generates the magma header based on a provided json definition.\n"
        "  Description fields are generated in the Doxygen format."
    )


# Generate a string for a comment with a Doxygen tag, wrapping text as necessary.
def format_comment(type, comment):
    wrap = 100
    prefix_first = "/// \\" + type + " "
    prefix_len = len(prefix_first)
    prefix_rest = "///".ljust(prefix_len)
    lines = textwrap.wrap(comment, wrap - prefix_len)
    formatted = [f"{prefix_first}{lines[0]}"]
    formatted.extend(f"{prefix_rest}{line}" for line in lines[1:])
    return "\n".join(formatted)


# Generate a string for a magma export object, including its comment block.
def format_export(export):
    # Verify no use of "struct" in types - these should use type_t instead.
    for arg in export["arguments"]:
        m = re.search(r"(.*)struct ([^\*]*)(\**)", arg["type"])
        if m:
            print(
                f'Error in {export["name"]}: Argument type "{arg["type"]}" appears to be a struct.'
                f' Did you mean to use "{m.group(1)}{m.group(2)}_t{m.group(3)}"?'
            )
            sys.exit(-1)
    # Leading comment
    lines = [f'///\n{format_comment("brief", export["description"])}']
    lines.extend(
        format_comment("param", f'{arg["name"]} {arg["description"]}')
        for arg in export["arguments"]
    )
    lines.append("///")
    # Function signature
    lines.append(f'MAGMA_EXPORT {export["type"]} {export["name"]}(')
    lines.append(
        ",\n".join(
            f'    {arg["type"]} {arg["name"]}' for arg in export["arguments"]
        )
    )
    if "attributes" in export:
        lines[-1] = f"{lines[-1]})"
        lines.append(f'{export["attributes"]};')
    else:
        lines[-1] = f"{lines[-1]});"
    lines.append("")
    return "\n".join(lines)


# License string for the top of the file.
def license():
    return (
        "// Copyright 2016 The Fuchsia Authors. All rights reserved.\n"
        "// Use of this source code is governed by a BSD-style license that can be\n"
        "// found in the LICENSE file.\n"
    )


def top_level_docs():
    return (
        "// Magma is the driver model for GPUs/NPUs on Fuchsia.\n"
        "//\n"
        '// Magma has two driver components: a hardware-specific library loaded into an "application"’s\n'
        '// address space ("client driver", sometimes known as "Installable client driver" or "ICD");\n'
        "// and a system driver that interfaces with the hardware. Magma is described in detail at [0].\n"
        "//\n"
        "// The Magma APIs are vendor independent. Some drivers may implement only the subset of the\n"
        "// APIs that are relevant. The format of data carried inside commands/command buffers is not\n"
        "// defined. Some APIs are explicitly extensible, such as magma_query, to allow for specific\n"
        "// vendor/driver needs.  Vendor specifics must be contained in a separate definitions file.\n"
        "//\n"
        "// Since client driver implementations may be written in a variety of languages (possibly C),\n"
        "// the Magma bindings have a C interface, and may be used to interact with both Magma and Sysmem.\n"
        "//\n"
        "// The Magma bindings are OS independent so a client driver targeting Magma can easily be built\n"
        "// for Fuchsia or Linux. On Fuchsia the APIs are backed by Zircon and FIDL; for virtualized Linux\n"
        "// they are backed by a virtualization transport (virtmagma). APIs prefixed by 'magma_virt_' are\n"
        "// for virtualization only.  32-bit and 64-bit builds are supported for virtualized clients.\n"
        "//\n"
        "// On Fuchsia the system driver is a separate process, so Magma APIs follow a feed forward design\n"
        "// where possible to allow for efficient pipelining of requests.\n"
        "//\n"
        "// [0] https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0198_magma_api_design\n"
        "//\n"
    )


# Guard macro that goes at the beginning/end of the header (after license).
def guards(begin):
    macro = "LIB_MAGMA_MAGMA_H_"
    if begin:
        return f"#ifndef {macro}\n#define {macro}\n"
    return f"#endif  // {macro}"


# Begin/end C linkage scope.
def externs(begin):
    if begin:
        return '#if defined(__cplusplus)\nextern "C" {\n#endif\n'
    return "#if defined(__cplusplus)\n}\n#endif\n"


# Includes list.
def includes():
    return (
        "#include <lib/magma/magma_common_defs.h> // IWYU pragma: export\n"
        "#include <stdint.h>\n"
    )


# Warning comment about auto-generation.
def genwarning():
    return (
        "// NOTE: DO NOT EDIT THIS FILE!\n"
        "// It is automatically generated by //src/graphics/lib/magma/include/magma/magma_h_gen.py\n"
    )


def genformatoff():
    return "// clang-format off\n"


def ifchange():
    return "// LINT.IfChange"


def thenchange():
    return "// LINT.ThenChange(/sdk/lib/magma_common/include/lib/magma/magma_common_defs.h:version)"


def main():
    if len(sys.argv) != 3:
        usage()
        return 2
    try:
        with open(sys.argv[1], "r") as file:
            with open(sys.argv[2], "w") as dest:
                magma = json.load(file)["magma-interface"]
                lines = [
                    license(),
                    top_level_docs(),
                    genwarning(),
                    genformatoff(),
                    guards(True),
                    includes(),
                    ifchange(),
                    externs(True),
                ]
                lines.extend(format_export(e) for e in magma["exports"])
                lines.append(externs(False))
                lines.append(thenchange())
                lines.append(guards(False))
                lines.append("")
                dest.write("\n".join(lines))
    except Exception as e:
        print(f"Error accessing files: {e}")
        usage()
        return 1


if __name__ == "__main__":
    sys.exit(main())
