#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import datetime
import os
import re
import subprocess
import tempfile

# All other paths are relative to here (main changes to this directory on startup).
ROOT_PATH = os.path.join(os.path.dirname(__file__), "..", "..")

RUSTFMT_PATH = "prebuilt/third_party/rust/linux-x64/bin/rustfmt"
BINDGEN_PATH = "prebuilt/third_party/rust_bindgen/linux-x64/bindgen"
CLANG_PATH = "prebuilt/third_party/clang/linux-x64/bin/clang"
FX_PATH = "scripts/fx"

FUCHSIA_NOTICE_HEADER = (
    """// Copyright %d The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

"""
    % datetime.datetime.now().year
)

# TODO(wintermelons): Remove allow(unused_unsafe) once upstream bug is fixed
# https://github.com/rust-lang/rust-bindgen/issues/3355
GENERATED_FILE_HEADER = """#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::undocumented_unsafe_blocks, reason = "bindgen-generated code")]
#![allow(unused_unsafe)]
"""

# Replacements to add to these defined by the user.
BASE_REPLACEMENTS = [
    # Remove alignment field that use u8
    (re.compile(r"pub _bitfield_align_[0-9]*: \[u8; 0\],\n"), ""),
]

# The `HEAD` API level.
#
# NOTE(hjfreyer): It's not totally clear that the value of HEAD is the
# right value here. If some parts of the C structures are missing,
# we may want to investigate setting this to a different value like
# `PLATFORM`.
FUCHSIA_API_LEVEL_HEAD = 4292870144


class Bindgen:
    def __init__(self):
        # Clang: Compilation target (`--target`)
        self.clang_target = "x86_64-unknown-linux-gnu"
        # Use the given PREFIX before raw types instead of ::std::os::raw.
        self.c_types_prefix = ""
        # Notice header to place at the beginning of the file.
        self.notice_header = FUCHSIA_NOTICE_HEADER
        # Whether to generate !#[allow(...)] directives.
        self.generate_allows = True
        # Additional raw lines of Rust code to add to the beginning of the generated output.
        self.raw_lines = ""
        # Whether to generate explicit padding fields in structs.
        self.explicit_padding = True
        # Whether size_t must be converted to usize
        self.size_t_is_usize = True
        # Mark types as an an opaque blob of bytes with a size and alignment.
        self.opaque_types = []
        # Clang: Include directories (`-I`)
        self.include_dirs = []
        # Generate implementations for standard traits when not auto-derivable (`--impl-foo`)
        self.std_impls = []
        # Standard derivations (`--with-derive-foo`)
        self.std_derives = []
        # Add extra traits to derive on generated structs/unions.
        # Only applies to `pub struct`/`pub union` that already have a #[derive()] line.
        self.auto_derive_traits = []
        # Pairs of (regex, str) replacements to apply to generated output.
        self.replacements = BASE_REPLACEMENTS
        # Do not generate bindings for given functions or methods.
        self.ignore_functions = False
        # Allowlist all the free-standing functions matching regexes.
        # Other non-allowlisted functions will not be generated.
        self.function_allowlist = []
        # Allowlist all the free-standing variables matching regexes.
        # Other non-allowlisted variables will not be generated.
        self.var_allowlist = []
        # Allowlist all the free-standing types matching regexes.
        # Other non-allowlisted types will not be generated.
        self.type_allowlist = []
        # Mark functions as hidden, to omit them from generated code.
        self.function_blocklist = []
        # Mark variables as hidden, to omit them from generated code.
        self.var_blocklist = []
        # Mark types as hidden, to omit them from generated code.
        self.type_blocklist = []
        # Avoid deriving/implementing Debug for types matching regexes.
        self.no_debug_types = []
        # Avoid deriving/implementing Copy for types matching regexes.
        self.no_copy_types = []
        # Avoid deriving/implementing Default for types matching regexes.
        self.no_default_types = []
        # Use types from Rust core instead of std.
        self.use_core = False
        # Whether to wrap unsafe operations in unsafe blocks.
        self.wrap_unsafe_ops = True
        # Additional flags to pass directly to bindgen.
        self.additional_bindgen_flags = []
        # Clang: Enable standard #include directories for the C++ standard library
        self.enable_stdlib_include_dirs = True
        # Clang: Define `__Fuchsia_API_level__`.
        self.fuchsia_api_level = ""
        # Clang: Additional command line flags to pass to clang.
        self.additional_clang_flags = []

    def set_auto_derive_traits(self, traits_map):
        self.auto_derive_traits = [(re.compile(x[0]), x[1]) for x in traits_map]

    def set_replacements(self, replacements):
        self.replacements = [
            (re.compile(x[0]), x[1]) for x in replacements
        ] + BASE_REPLACEMENTS

    def run_bindgen(self, input_file, output_file):
        # Bindgen arguments.
        raw_lines = self.notice_header
        raw_lines += f"// LINT.IfChange\n\n"
        if self.generate_allows:
            raw_lines += GENERATED_FILE_HEADER
        raw_lines += self.raw_lines

        args = [
            BINDGEN_PATH,
            "--no-layout-tests",
            "--raw-line",
            raw_lines,
            "-o",
            output_file,
        ]

        if self.explicit_padding:
            args += ["--explicit-padding"]

        if self.ignore_functions:
            args.append("--ignore-functions")

        if self.c_types_prefix:
            args.append("--ctypes-prefix=" + self.c_types_prefix)

        if self.use_core:
            args.append("--use-core")

        if not self.size_t_is_usize:
            args.append("--no-size_t-is-usize")

        if self.wrap_unsafe_ops:
            args.append("--wrap-unsafe-ops")

        args += ["--allowlist-function=" + x for x in self.function_allowlist]
        args += ["--allowlist-var=" + x for x in self.var_allowlist]
        args += ["--allowlist-type=" + x for x in self.type_allowlist]
        args += ["--blocklist-function=" + x for x in self.function_blocklist]
        args += ["--blocklist-var=" + x for x in self.var_blocklist]
        args += ["--blocklist-type=" + x for x in self.type_blocklist]
        args += ["--opaque-type=" + x for x in self.opaque_types]
        args += ["--no-debug=" + x for x in self.no_debug_types]
        args += ["--no-copy=" + x for x in self.no_copy_types]
        args += ["--no-default=" + x for x in self.no_default_types]
        args += ["--impl-" + x for x in self.std_impls]
        args += ["--with-derive-" + x for x in self.std_derives]

        args += self.additional_bindgen_flags

        args += [input_file]

        with tempfile.NamedTemporaryFile() as depfile:
            args += [
                "--depfile",
                depfile.name,
            ]

            # Clang arguments (after the "--").
            args += [
                "--",
                "-target",
                self.clang_target,
                "-DIS_BINDGEN=1",
            ]

            clang_resource_dir = (
                subprocess.check_output([CLANG_PATH, "-print-resource-dir"])
                .decode("utf-8")
                .strip()
            )
            args += ["-resource-dir", clang_resource_dir]

            if not self.enable_stdlib_include_dirs:
                args += ["-nostdlibinc"]

            if self.fuchsia_api_level:
                args += [f"-D__Fuchsia_API_level__={self.fuchsia_api_level}"]

            for i in self.include_dirs:
                args += ["-I", i]
            args += ["-I", "."]

            args += self.additional_clang_flags

            subprocess.check_call(
                args,
                env={"RUSTFMT": os.path.abspath(RUSTFMT_PATH)},
            )

            depfile_contents = depfile.read().decode("utf-8")
            # The colon delimits the output file from all the input files.
            all_input_files = depfile_contents.split(": ")[1]
            all_input_files = all_input_files.split(" ")
            fuchsia_input_files = []
            for path in all_input_files:
                if not path.startswith("/"):
                    if path.startswith("./"):
                        clean_path = path[2:]
                    else:
                        clean_path = path
                    fuchsia_input_files.append("//" + clean_path)

            if len(fuchsia_input_files) > 1:
                # go/ifthisthenthatlint#disjunction
                ifttt_matcher = (
                    "\n//    " + " |\n//    ".join(fuchsia_input_files) + "\n//"
                )
            else:
                ifttt_matcher = fuchsia_input_files[0]

        with open(output_file, "a") as f:
            f.write(f"\n// LINT.ThenChange({ifttt_matcher})\n")

    def get_auto_derive_traits(self, line):
        """Returns true if the given line defines a Rust structure with a name
        matching any of the types we need to add FromBytes."""
        if not (
            line.startswith("pub struct ") or line.startswith("pub union ")
        ):
            return None

        # The third word (after the "pub struct") is the type name.
        split = re.split(r"[ <\(]", line)
        if len(split) < 3:
            return None
        type_name = split[2]

        results = set()
        for t, traits in self.auto_derive_traits:
            if t.match(type_name):
                results.update(traits)
        if len(results) == 0:
            return None
        else:
            return results

    def post_process_rust_file(self, rust_file_name):
        with open(rust_file_name, "r+") as source_file:
            input_lines = source_file.readlines()
            output_lines = []
            for line in input_lines:
                extra_traits = self.get_auto_derive_traits(line)
                if extra_traits:
                    # Parse existing traits, if any.
                    traits = set(extra_traits)
                    if len(output_lines) > 0 and output_lines[-1].startswith(
                        "#[derive(",
                    ):
                        traits.update(output_lines[-1][9:-3].split(", "))
                        output_lines.pop()
                    output_lines.append(
                        "#[derive(" + ", ".join(sorted(traits)) + ")]\n",
                    )
                output_lines.append(line)

            text = "".join(output_lines)
            for regexp, replacement in self.replacements:
                text = regexp.sub(replacement, text)

            source_file.seek(0)
            source_file.truncate()
            source_file.write(text)

    def run(self, input_file, rust_file):
        os.chdir(ROOT_PATH)

        self.run_bindgen(input_file, rust_file)
        self.post_process_rust_file(rust_file)

        subprocess.check_call([FX_PATH, "format-code", "--files=" + rust_file])
