#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Processes the zip archive of syscall markdown and adds toc and readme."""

import argparse
import io
import shutil
import sys
import zipfile

README_HEADER = """# Zircon System Calls

[Life of a Fuchsia syscall](/docs/concepts/kernel/life_of_a_syscall.md)

"""

TOC_HEADER = """# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Please, read https://fuchsia.dev/fuchsia-src/contribute/docs/documentation-navigation-toc
# before making changes to this file, and add a member of the fuchsia.dev
# team as reviewer.
toc:
"""

# TODO(b/306180541): Move syscall categories closer to source .fidl files.

# These are the sections for the system calls in order of appearance.
README_SECTIONS = [
    ("Handles", "handle"),
    ("Objects", "object"),
    ("Threads", "thread"),
    ("Thread Sampling", "sampler"),
    ("IO Buffers", "iob"),
    ("Processes", "process"),
    ("Jobs", "job"),
    ("Tasks (Thread, Process, or Job)", "task"),
    ("Profiles", "profile"),
    ("Exceptions", "exception"),
    ("Channels", "channel"),
    ("Sockets", "socket"),
    ("Streams", "stream"),
    ("Fifos", "fifo"),
    ("Events, Event Pairs, and Counters", "event"),
    ("Ports", "port"),
    ("Futexes", "futex"),
    ("Virtual Memory Objects (VMOs)", "vmo"),
    ("Virtual Memory Address Regions (VMARs)", "vmar"),
    ("Userspace Pagers", "pager"),
    ("Cryptographically Secure RNG", "cprng"),
    ("Time", "time"),
    ("Timers", "timer"),
    ("Message Signaled Interrupts (MSIs)", "msi"),
    ("Hypervisor guests", "guest"),
    ("Virtual CPUs", "vcpu"),
    ("Global system information", "global"),
    ("Debug Logging", "debug"),
    ("Multi-function", "multi-function"),
    ("System", "system"),
    ("Driver", "driver"),
    ("Tracing", "tracing"),
    ("Memory Barriers", "membarrier"),
    ("Restricted Mode (Work in progress)", "restricted"),
]


def gen_readme_and_toc(data: list[dict[str, str]], reference_root: str) -> None:
    readme = io.StringIO()
    toc = io.StringIO()
    print(README_HEADER, file=readme)

    print(TOC_HEADER, file=toc)
    print('- title: "Overview"', file=toc)
    print(f"  path: {reference_root}/README.md", file=toc)

    topic_map = dict()
    # build a map of prefixes from the topics
    for entry in data:
        if entry["topic"] not in topic_map:
            topic_map[entry["topic"]] = [entry]
        else:
            topic_map[entry["topic"]].append(entry)

    for title, topic in README_SECTIONS:
        entries = topic_map[topic]
        entries.sort(key=lambda x: x["filename"])
        del topic_map[topic]
        print(f"## {title}\n", file=readme)
        print(f'- title: "{title}"', file=toc)
        print(f"  section:", file=toc)

        for entry in entries:
            label = entry["filename"][:-3]
            filename = entry["filename"]
            summary = entry["summary"]
            print(f"* [{label}]({filename}) - {summary}", file=readme)
            print(f'  - title: "zx_{label}"', file=toc)
            print(f"    path: {reference_root}/{filename}", file=toc)
        print("", file=readme)

    if topic_map:
        raise ValueError(f"Uncategorized topics: {topic_map}")

    return (readme.getvalue(), toc.getvalue())


def get_topic(filename: str) -> str:
    topic = filename.split("_")[0]

    # There are some special cases
    if topic == "vmar" and filename == "vmar_unmap_handle_close_thread_exit.md":
        topic = "multi-function"
    elif topic in ["eventpair", "counter"]:
        topic = "event"
    elif topic == "system":
        if filename == "system_get_event.md":
            topic = "event"
        if filename in [
            "system_get_dcache_line_size.md",
            "system_get_features.md",
            "system_get_num_cpus.md",
            "system_get_page_size.md",
            "system_get_physmem.md",
            "system_get_version_string.md",
        ]:
            topic = "global"
    elif topic in [
        "bti",
        "cache",
        "interrupt",
        "iommu",
        "ioports",
        "pmt",
        "resource",
        "smc",
    ]:
        topic = "driver"
    elif topic in ["clock", "deadline", "nanosleep.md", "ticks"]:
        topic = "time"
    elif topic in ["ktrace"]:
        topic = "tracing"
    elif topic in ["debuglog"]:
        topic = "debug"

    return topic


def read_summary(input: bytes) -> str:
    """Read the first line of the Summary section."""
    lines = input.decode().splitlines()
    saw_summary = False
    for l in lines:
        if l == "## Summary":
            saw_summary = True
        elif saw_summary and l:
            return l
    return ""


def gen_toc_readme(inzip: str, outzip: str, reference_root: str) -> None:
    """Generates _toc.yaml and README.md for the files in inzip."""

    # Copy the zip
    shutil.copy2(inzip, outzip)

    data = []
    # Build the list of topic, summary, path tuples
    with zipfile.ZipFile(inzip) as zf:
        for m in zf.infolist():
            if not m.is_dir():
                filename = m.filename
                topic = get_topic(filename)
                contents = zf.read(m)
                summary = read_summary(contents)
                data.append(
                    {"topic": topic, "summary": summary, "filename": filename}
                )
    readme_contents, toc_contents = gen_readme_and_toc(data, reference_root)

    with zipfile.ZipFile(outzip, "a") as zf:
        zf.writestr("README.md", readme_contents)
        zf.writestr("_toc.yaml", toc_contents)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,  # Prepend helpdoc with this file's docstring.
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--input",
        type=str,
        required=True,
        help="Output location where generated docs should go",
    )
    parser.add_argument(
        "--output",
        type=str,
        required=True,
        help="zipfile path for final output",
    )
    parser.add_argument(
        "--reference-root",
        type=str,
        required=True,
        help="root path for reference directory. This is prepended to the filename paths to build the local URL for the toc.",
    )

    args = parser.parse_args()

    gen_toc_readme(args.input, args.output, args.reference_root)

    return 0


if __name__ == "__main__":
    sys.exit(main())
