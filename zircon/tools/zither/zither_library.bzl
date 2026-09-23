# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Bazel macros for Zither."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@rules_cc//cc:defs.bzl", "cc_library")
load("//build/bazel/rules/rust:rustc_library.bzl", "rustc_library")

# LINT.IfChange

# TODO(https://fxbug.dev/456186319): This should be:
# support_rust = zircon_toolchain == false
support_rust = True

_CLANG_FORMAT = {
    "script": ["@fuchsia_clang//:bin/clang-format"],
    "inputs": ["//:.clang-format"],
    "args": ["--style=file:" + paths.join(".clang-format")],
    # TODO(https://fxbug.dev/427976639): When supporting zither_golden_files(),
    # it may be necessary to add this and allow these definitions to be reused.
    # See the description of `formatter` for `_SUPPORTED_ZITHER_BACKEND_INFO` in
    # zither_library.gni.
    # extensions = [ ".h" ]
}

_RUSTFMT = None
# TODO(https://fxbug.dev/454449781): Determine the correct path and uncomment.
# _RUSTFMT = {
#     "script": ["//prebuilt/third_party/rust/linux-x64/bin:rustfmt"],
#     "inputs": ["//:rustfmt.toml"],
#     "args": ["--config-path=" + paths.join("rustfmt.toml")],
# }

_COMMON_ATTRS = {
    "fidl_ir_json": attr.label(
        doc = "The path to the associated FIDL IR JSON file.",
        allow_single_file = True,
        mandatory = True,
        configurable = False,
    ),
    "testonly": attr.bool(
        default = False,
        configurable = False,
    ),
}

_COMMON_BACKEND_ATTRS = _COMMON_ATTRS | {
    "output_dir": attr.string(
        doc = "The directory for Zither outputs.",
        mandatory = True,
        configurable = False,
    ),
}

_FORMATTER_ATTR = {
    "formatter": attr.string_list_dict(
        doc = "A formatting specification for Zither outputs.",
        configurable = False,
    ),
}

_OUTPUT_NAMESPACE_ATTR = {
    "output_namespace": attr.string(
        doc = "The subdirectory of `output_dir` that has the Zither outputs for entries.",
        mandatory = True,
        configurable = False,
    ),
}

_SUBBACKEND_ATTR = {
    "backend": attr.string(
        doc = "The Zither backend to invoke.",
        mandatory = True,
        configurable = False,
    ),
}

_SOURCE_NAMES_ATTR = {
    "source_names": attr.string_list(
        doc = "The list of the basenames (i.e., stripped of .fidl and .test.fidl extensions) of the source FIDL files.",
        mandatory = True,
        configurable = False,
    ),
}

def _zither_impl(
        name,
        backend,
        fidl_ir_json,
        output_dir,
        generated_files,
        formatter,
        output_namespace_override,
        testonly,
        visibility):
    """Implementation of the _zither() macro."""
    output_manifest = paths.join(output_dir, "outputs.json")

    cmd_parts = [
        "$(execpath //zircon/tools/zither)",
        "-ir $(location %s)" % fidl_ir_json,
        "-backend %s" % backend,
        "-output-manifest $(location %s)" % output_manifest,
        "-output-dir $(RULEDIR)/%s" % output_dir,
        # Bazel runs commands from the source root.
        "-source-dir .",
    ]
    tools = ["//zircon/tools/zither"]

    srcs = [fidl_ir_json]

    if output_namespace_override:
        cmd_parts.append("-output-namespace %s" % output_namespace_override)

    if formatter:
        script = formatter["script"][0]
        cmd_parts.append("-formatter $(execpath %s)" % script)
        tools.append(script)
        if "args" in formatter and formatter["args"]:
            cmd_parts.extend(["-formatter-args"] + formatter["args"])
        if "inputs" in formatter and formatter["inputs"]:
            srcs.extend(formatter["inputs"])

    native.genrule(
        name = name,
        srcs = srcs,
        outs = [output_manifest] + generated_files,
        cmd = " ".join(cmd_parts),
        tools = tools,
        visibility = visibility,
        testonly = testonly,
    )

    # Internal subtarget used to check that a given backend's outputs were
    # fully specified; used for testing.
    output_check_target = name + ".check"
    check_target_stamp = paths.join(output_dir, output_check_target + ".stamp")
    check_outputs = "//zircon/tools/zither/scripts:check_outputs"
    native.genrule(
        name = output_check_target,
        tools = [check_outputs],
        # The manifest must be first
        srcs = [output_manifest] + generated_files,
        outs = [check_target_stamp],
        cmd = "$(location %s) --stamp $@ --manifest $(SRCS)" % check_outputs,
    )

_zither = macro(
    doc = "Internal Zither invocation helper template used by `zither_library()`.",
    implementation = _zither_impl,
    attrs = {
        "generated_files": attr.output_list(
            doc = "The expected set of Zither outputs, which necessarily must start with \"$output_dir/\".",
            mandatory = True,
        ),
        "output_namespace_override": attr.string(
            doc = "If set, overrides the default convention for the Zither backend's output namespace.",
            configurable = False,
        ),
    } | _COMMON_BACKEND_ATTRS | _FORMATTER_ATTR | _SUBBACKEND_ATTR,
)

#
# Internal language library helper templates used by `zither_library()`.
#

def _zither_c_family_library_impl(
        name,
        source_names,
        output_dir,
        output_namespace,
        backend,
        fidl_ir_json,
        formatter,
        output_namespace_override,
        testonly,
        visibility):
    """Implementation of the _zither_c_family_library() macro."""
    zither_target = name + ".gen"

    # Work around Bazel symbolic macro naming limitations by ignoring
    # `output_dir` and crafting a subdirectory name that meets the requirements.
    #
    # A note about output file locations (using `zbi` as an example):
    #   In Bazel, there is no `target_gen_dir`. fidl_library() currently outputs
    #   all its files to `bazel-bin/sdk/fidl/zbi/`, which is the package output
    #   directory.
    #   The GN implementation passed `fidl_gen_dir` as `output_dir` to cause FIDL
    #   backends to put their files in a subdirectory of the gen dir.
    #   If we were to approximate that, Zither would output its files within
    #   `bazel-bin/sdk/fidl/zbi/zither`,
    #   `bazel-bin/sdk/fidl/zbi/gen/zbi/zither`, or similar.
    #
    # Bazel symbolic macros require that output files' paths begin with the
    # macro's `name` beginning starting from the package output directory. Thus,
    # the `output_dir` cannot be used as it was in GN.
    #
    # Instead we must use a directory name based on the `zither_target`, which
    # generates the files. Since we want a directory of files and `/` is not
    # a valid separator for Bazel symbolic macro naming purposes, we must add
    # a valid suffix to the directory name. (This is ".out" below.)
    #
    # As a result, instead of all of the Zither backends being within
    # `bazel-bin/sdk/fidl/zbi/zither/`, they are peers such as
    # `bazel-bin/sdk/fidl/zbi/zbi_zither.c.gen.out/`.
    # The files generated in directories such as
    # `bazel-bin/sdk/fidl/zbi/zbi_zither.c.gen.out/zbi/c/`.
    #
    # The following shows how this path is composed:
    #   bazel-bin/sdk/fidl/zbi/zbi_zither.c.gen.out/zbi/c/
    #   package:  ^^^^^^^^^^^^
    #   output_dir override:   ^^^^^^^^^^^^^^^^^^^^
    #   prefix_parts:                               ^^^^
    #   library name:                                    ^^^
    #   suffix_parts                                         ^^^^^^
    #
    # TODO(https://fxbug.dev/454449781): Consider removing the `output_dir`
    # attribute (and `fidl_gen_dir`).
    output_dir = zither_target + ".out"

    generated_files = [paths.join(output_dir, output_namespace, name + ".h") for name in source_names]
    if backend == "c":
        generated_files.append(paths.join(output_dir, "README.md"))

    _zither(
        name = zither_target,
        backend = backend,
        fidl_ir_json = fidl_ir_json,
        output_dir = output_dir,
        generated_files = generated_files,
        formatter = formatter,
        output_namespace_override = output_namespace if output_namespace_override else None,
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    # TODO(https://fxbug.dev/456186319): This may need to be a wrapper when
    # supporting kernel builds. In GN, it was a `library_headers()`, which adds
    # `public_deps += [ "//zircon/system/public" ]`.
    cc_library(
        name = name,
        hdrs = generated_files,
        include_prefix = output_dir,
        # TODO(https://fxbug.dev/454449781): Figure out how to make this target depend on the check target.
        # implementation_deps = [":" + zither_target + ".check"],
        testonly = testonly,
        visibility = visibility,
    )

def _zither_rust_library_impl(
        name,
        source_names,
        output_dir,
        output_namespace,
        backend,
        fidl_ir_json,
        formatter,
        testonly,
        visibility):
    """Implementation of the _zither_rust_library() macro."""
    zither_target = name + ".gen"

    # Work around Bazel symbolic macro naming limitations. See _zither_c_family_library_impl().
    output_dir = paths.join(zither_target + ".out", output_dir)

    if backend == "rust_syscall":
        crate_name = "zx_sys"
        crate_root = paths.join(output_dir, output_namespace, "src", "definitions.rs")
        crate_deps = ["//sdk/rust/zx-types"]
        generated_files = [crate_root]
    else:
        generated_files = [paths.join(
            output_dir,
            output_namespace,
            "src",
            name.replace("-", "_") + ".rs",
        ) for name in source_names]
        crate_name = output_namespace
        crate_root = paths.join(output_dir, output_namespace, "src", "lib.rs")
        crate_deps = [
            "//third_party/rust_crates/vendor:bitflags",
            "//third_party/rust_crates/vendor:zerocopy",
        ]
        generated_files.append(crate_root)

    _zither(
        name = zither_target,
        backend = backend,
        fidl_ir_json = fidl_ir_json,
        output_dir = output_dir,
        generated_files = generated_files,
        formatter = formatter,
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    # TODO(https://fxbug.dev/454449781): It may be necesary to do this per the GN template:
    # Namespace by target so that there is no rlib collision between fidl()
    # targets in the same file.
    rustc_library(
        name = name,
        crate_name = crate_name,
        crate_root = crate_root,
        srcs = generated_files,
        # TODO(https://fxbug.dev/454449781): Figure out how to make this target depend on the check target.
        # deps = crate_deps + [":" + zither_target + ".check"],
        deps = crate_deps,
        edition = "2024",
        testonly = testonly,
        visibility = visibility,
    )

def _zither_zircon_ifs_file_impl(
        name,
        output_dir,
        fidl_ir_json,
        testonly,
        visibility):
    """Implementation of the _zither_zircon_ifs_file() macro."""
    zither_target = name + ".gen"

    # Work around Bazel symbolic macro naming limitations. See _zither_c_family_library_impl().
    output_dir = paths.join(zither_target + ".out", output_dir)

    ifs_file = paths.join(output_dir, "zircon.ifs")
    json_file = paths.join(output_dir, "libzircon.json")
    generated_files = [
        ifs_file,
        json_file,
    ]

    _zither(
        name = zither_target,
        backend = "zircon_ifs",
        fidl_ir_json = fidl_ir_json,
        output_dir = output_dir,
        generated_files = generated_files,
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    # TODO(https://fxbug.dev/456186319): Implement equivalent of metadata in GN implementation.
    native.filegroup(
        name = name,
        srcs = [":" + zither_target, ":" + zither_target + ".check"],
        testonly = testonly,
        visibility = visibility,
    )

def _zither_kernel_sources_impl(
        name,
        output_dir,
        output_namespace,
        fidl_ir_json,
        formatter,
        testonly,
        visibility):
    """Implementation of the _zither_kernel_sources() macro."""
    zither_target = name + ".gen"

    # Work around Bazel symbolic macro naming limitations. See _zither_c_family_library_impl().
    output_dir = paths.join(zither_target + ".out", output_dir)

    """Implementation of the _zither_kernel_sources() macro."""
    generated_files = [
        paths.join(output_dir, output_namespace, "category.inc"),
        paths.join(output_dir, output_namespace, "kernel.inc"),
        paths.join(output_dir, output_namespace, "kernel-wrappers.inc"),
        paths.join(output_dir, output_namespace, "syscall_sigs.rs"),
        paths.join(output_dir, output_namespace, "syscalls.inc"),
        paths.join(output_dir, output_namespace, "zx-syscall-numbers.h"),
    ]

    _zither(
        name = zither_target,
        backend = "kernel",
        fidl_ir_json = fidl_ir_json,
        output_dir = output_dir,
        generated_files = generated_files,
        formatter = formatter,
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    # TODO(https://fxbug.dev/456186319): This may need to be a wrapper when
    # supporting kernel builds. In GN, it was a `library_headers()`, which adds
    # `public_deps += [ "//zircon/system/public" ]`.
    cc_library(
        name = name,
        hdrs = generated_files,
        include_prefix = output_dir,
        # TODO(https://fxbug.dev/454449781): Figure out how to make this target depend on the check target.
        # implementation_deps = [":" + zither_target + ".check"],
        testonly = testonly,
        visibility = visibility,
    )

def _zither_legacy_syscall_cdecl_sources_impl(
        name,
        output_dir,
        output_namespace,
        fidl_ir_json,
        formatter,
        testonly,
        visibility):
    """Implementation of the _zither_legacy_syscall_cdecl_sources() macro."""

    # TODO(https://fxbug.dev/456186319): Implement when supporting kernel builds.
    pass

def _zither_syscall_docs_impl(
        name,
        output_dir,
        fidl_ir_json,
        testonly,
        visibility):
    """Implementation of the _zither_syscall_docs() macro."""
    zither_target = name + ".gen"

    # Work around Bazel symbolic macro naming limitations. See _zither_c_family_library_impl().
    output_dir = paths.join(zither_target + ".out", output_dir)

    _zither(
        name = zither_target,
        backend = "syscall_docs",
        fidl_ir_json = fidl_ir_json,
        output_dir = output_dir,
        # The set of generated files is not statically known and is instead
        # dynamically determined from zither's output manifest, outputs.json.
        # Note: Building `zbi_zither.syscall_docs.gen.check` will fail.
        generated_files = [],
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    native.filegroup(
        name = name,
        # We purposefully do not depend on `":" + zither_target + ".check"` as
        # we do not statically know the set of backend outputs in GN.
        srcs = [":" + zither_target],
        testonly = testonly,
        visibility = visibility,
    )

_zither_c_family_library = macro(
    doc = "Internal C family library helper() macro.",
    implementation = _zither_c_family_library_impl,
    attrs = {
        "output_namespace_override": attr.bool(
            default = False,
        ),
    } | _COMMON_BACKEND_ATTRS | _FORMATTER_ATTR | _OUTPUT_NAMESPACE_ATTR | _SUBBACKEND_ATTR | _SOURCE_NAMES_ATTR,
)

_zither_rust_library = macro(
    doc = "Internal Rust library helper macro.",
    implementation = _zither_rust_library_impl,
    attrs = _COMMON_BACKEND_ATTRS | _FORMATTER_ATTR | _OUTPUT_NAMESPACE_ATTR | _SUBBACKEND_ATTR | _SOURCE_NAMES_ATTR,
)

_zither_zircon_ifs_file = macro(
    doc = "Internal Zircon IFS file helper macro.",
    implementation = _zither_zircon_ifs_file_impl,
    attrs = _COMMON_BACKEND_ATTRS,
)

_zither_kernel_sources = macro(
    doc = "Internal kernel sources helper macro.",
    implementation = _zither_kernel_sources_impl,
    attrs = _COMMON_BACKEND_ATTRS | _FORMATTER_ATTR | _OUTPUT_NAMESPACE_ATTR,
)

_zither_legacy_syscall_cdecl_sources = macro(
    doc = "Internal legacy syscall C decl sources helper macro.",
    implementation = _zither_legacy_syscall_cdecl_sources_impl,
    attrs = _COMMON_BACKEND_ATTRS | _FORMATTER_ATTR | _OUTPUT_NAMESPACE_ATTR,
)

_zither_syscall_docs = macro(
    doc = "Internal syscall docs helper macro.",
    implementation = _zither_syscall_docs_impl,
    attrs = _COMMON_BACKEND_ATTRS,
)

# Information on supported Zither backends.
#
# Each backend dictionary contains the following:
#
#  * output_namespace
#    - Required: Describes the output subdirectory of the output directory
#      passed to zither in which the backend artifacts will be written. This
#      subdirectory has backend-specific significance as a C include path,
#      Go package name, etc. Given a FIDL library name `library_name` and
#      referring to this dictionary as `output_namespace_info`, one can
#      reconstruct this path as follows:
#      ```
#      output_namespace_info = {
#        prefix_parts = []
#        suffix_parts = []
#        part_separator = "/"
#        forward_variables_from(backend_info.output_namespace, "*")
#        parts = prefix_parts
#        if (defined(library_name_separator)) {
#         parts += [ string_replace(library_name, ".", library_name_separator) ]
#        }
#        path += suffix_parts
#        path = string_join(part_separator, parts)
#      }
#      output_namespace = output_namespace_info.path
#      ```
#    - Type: dictionary
#
#    The dictionary contains the following:
#      * prefix_parts
#        - Optional: The path parts that prefix the output subdirectory.
#        - Type: list(string)
#        - Default: []
#
#      * library_name_separator
#        - Optional: The separator with which the '.'-separated tokens of the
#          FIDL library name should be joined in the output subdirectory
#          namespace (in between the prefix and suffix parts ). A value of "."
#          will just use the library name unchanged as path token.
#        - Type: string
#
#      * suffix_parts
#        - Optional: The path parts that suffix the output subdirectory.
#        - Type: list(string)
#        - Default: []
#
#      * part_separator
#        - Optional: The path part separator.
#        - Type: string
#        - Default: "/"
#
#  * formatter
#    - Optional: A formatting specification for Zither outputs. The shape and
#      semantics of this parameter are identical to the `formatter` parameter
#      of `golden_files()`. While `formatter.extensions` is not consumed by
#      Zither - it makes sure to only format the appropriate files - it is
#      consumed in zither_golden_files() for the formatting of goldens outside
#      of Zither.
#    - Type: dictionary
#
# TODO(https://fxbug.dev/42172629, https://fxbug.dev/42175173): "cpp"
_SUPPORTED_ZITHER_BACKEND_INFO = {
    # C data layout bindings.
    "c": {
        "output_namespace": {
            "prefix_parts": [],
            "library_name_separator": ".",
            "suffix_parts": [
                "c",
            ],
            "supports_override": True,
        },
        "formatter": _CLANG_FORMAT,
        "_library_template": _zither_c_family_library,
    },
    # Assembly data layout bindings.
    "asm": {
        "output_namespace": {
            "prefix_parts": [],
            "library_name_separator": ".",
            "suffix_parts": [
                "asm",
            ],
            "supports_override": True,
        },
        "formatter": _CLANG_FORMAT,
        "_library_template": _zither_c_family_library,
    },
    # Syscall text ABI bindings.
    "zircon_ifs": {
        "output_namespace": {},
        "_library_template": _zither_zircon_ifs_file,
    },
    # Internal kernel bindings.
    "kernel": {
        "formatter": _CLANG_FORMAT,
        "output_namespace": {
            "prefix_parts": [
                "lib",
                "syscalls",
            ],
        },
        "_library_template": _zither_kernel_sources,
    },
    # Legacy C syscall declarations.
    "legacy_syscall_cdecl": {
        "formatter": _CLANG_FORMAT,
        "output_namespace": {
            "prefix_parts": [
                "zircon",
                "syscalls",
                "gen",
            ],
        },
        "_library_template": _zither_legacy_syscall_cdecl_sources,
    },
    # Syscall documentation markdown.
    "syscall_docs": {
        "output_namespace": {},
        "_library_template": _zither_syscall_docs,
    },
}

_SUPPORTED_ZITHER_BACKEND_INFO.update({
    # Rust data layout bindings.
    "rust": {
        "output_namespace": {
            "prefix_parts": [],
            "library_name_separator": "-",
            "part_separator": "-",
        },
        "formatter": _RUSTFMT,
        "_library_template": _zither_rust_library,
    },
    # Thin Rust FFI syscall wrappers.
    "rust_syscall": {
        "output_namespace": {
            "library_name_separator": "-",
        },
        "formatter": _RUSTFMT,
        "_library_template": _zither_rust_library,
    },
} if support_rust else {})

def _zither_library_impl(
        name,
        library_name,
        srcs,
        fidl_gen_dir,
        fidl_ir_json,
        backend_overrides,
        testonly,
        visibility):
    """Implementation of the zither_library() macro."""
    source_names = []
    for src in srcs:
        # Strip any .fidl or .test.fidl extensions.
        s_name = src.name
        if "." in s_name:
            s_name = s_name.rsplit(".", 1)[0]
        if "." in s_name and s_name.rsplit(".", 1)[1] == "test":
            s_name = s_name.rsplit(".", 1)[0]

        # Ignore overview.fidl files, which do not contribute declarations:
        #
        # See https://fuchsia.dev/fuchsia-src/development/languages/fidl/guides/style#library-overview.
        if s_name != "overview":
            source_names.append(s_name)

    for backend, info in _SUPPORTED_ZITHER_BACKEND_INFO.items():
        output_namespace_info = info["output_namespace"]
        parts = output_namespace_info.get("prefix_parts", [])[:]
        separator = output_namespace_info.get("library_name_separator")
        if separator:
            parts.append(library_name.replace(".", separator))
        parts.extend(output_namespace_info.get("suffix_parts", []))
        output_namespace = output_namespace_info.get("part_separator", "/").join(parts)

        if backend in backend_overrides:
            output_namespace = backend_overrides[backend]

        kwargs = {
            "name": name + "." + backend,
            "output_dir": paths.join(fidl_gen_dir, backend),
            "fidl_ir_json": fidl_ir_json,
            "testonly": testonly,
            "visibility": visibility,
        }

        if backend not in ["syscall_docs", "zircon_ifs"]:
            kwargs["formatter"] = info.get("formatter")
        if backend in ["c", "asm", "rust", "rust_syscall"]:
            kwargs["source_names"] = source_names
        if backend in ["c", "asm", "kernel", "legacy_syscall_cdecl", "rust", "rust_syscall"]:
            kwargs["output_namespace"] = output_namespace
        if backend in ["c", "asm", "rust", "rust_syscall"]:
            kwargs["backend"] = backend
        if backend in ["c", "asm"]:
            kwargs["output_namespace_override"] = output_namespace_info.get("supports_override", False)

        info["_library_template"](**kwargs)

zither_library = macro(
    doc = """Defines a full set of per-backend targets for a Zither library.
`zither_library()` is meant to be instantiated within `fidl_library()`. It
consumes FIDL source and defines the relevant language library targets that
collect the bindings of the various supported Zither backends. These backends
are defined in `_SUPPORTED_ZITHER_BACKEND_INFO` and the details of their
bindings can be found in //zircon/tools/zither/README.md. The associated backend library
subtargets are as follows where `${output_namespace}` is as described above in
`_SUPPORTED_ZITHER_BACKEND_INFO`:

Subtargets:
 * ${target_name}.${backend_name}
   Each supported Zither backend, corresponding to a named entry of
   `_SUPPORTED_ZITHER_BACKEND_INFO`, yields a subtarget for generating the
   associated bindings. For more information about a given backend's set of
   bindings see //zircon/tools/zither/backends/${backend_name}/README.md.
""",
    implementation = _zither_library_impl,
    # TODO(https://fxbug.dev/454449781): Support overrides for Zither backends
    # as described below from the GN template.
    #
    # Parameters - supplied directly via `fidl() { zither = {...} }`:
    #
    #  * ${backend_name}
    #    - Optional: Additional backend-specific parameters
    #    - Type: scope
    #
    #    Each scope contains:
    #    * output_namespace
    #      - Optional: This is the namespace/layout under which backend outputs are
    #        generated within the specified output directory. By default, this is
    #        backend-specific. The value can determine the name of the resulting
    #        library/package/crate/etc. (when a function of source layout), as well
    #        as the 'include' namespace of the headers generated by a C family
    #        backend. A backend only supports this parameter if
    #        `supported_zither_backend_info["$backend_name"].output_namespace.supports_overrides`
    #        is defined and true.
    #      - Type: string or relative path
    attrs = {
        "library_name": attr.string(
            doc = "The name of the FIDL library.",
            mandatory = True,
            configurable = False,
        ),
        "srcs": attr.label_list(
            doc = "The input FIDL sources, comprising one library necessarily of the name $target_name.",
            allow_files = True,
            mandatory = True,
            configurable = False,
        ),
        "fidl_gen_dir": attr.string(
            doc = "The directory under which Zither outputs should be generated.",
            mandatory = True,
            configurable = False,
        ),
        "backend_overrides": attr.string_dict(
            doc = "A dictionary mapping backend names to their output_namespace override.",
            default = {},
            configurable = False,
        ),
    } | _COMMON_ATTRS,
)

# LINT.ThenChange(zither_library.gni)
