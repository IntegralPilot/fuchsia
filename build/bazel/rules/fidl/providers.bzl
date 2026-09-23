# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

visibility("private")

FidlLibraryInfo = provider(
    "Contains information about a FIDL library",
    fields = {
        "name": "Name of the FIDL library",
        "ir": "Path to the JSON file with the library's intermediate representation",
        "srcs_depset": "Depset of FIDL source files.",
        "libraries_file": "Path to the .libraries file, in which each line contains a library's source files separated by spaces.",
    },
)
