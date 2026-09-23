# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common rules and helper functions for extracting and manipulating debug symbols."""

load(
    "@fuchsia_rules_common//:utils.bzl",
    "find_cc_toolchain",
    "flatten",
    "get_target_deps_from_attributes",
    "make_resource_struct",
)
load(
    "@fuchsia_rules_common//debug_symbols:providers.bzl",
    "FuchsiaCollectedDebugSymbolsInfo",
    "FuchsiaCollectedUnstrippedBinariesInfo",
    "FuchsiaDebugSymbolInfo",
    "FuchsiaUnstrippedBinariesInfo",
    "FuchsiaUnstrippedBinaryInfo",
)
load(
    "@fuchsia_rules_common//packages:providers.bzl",
    "FuchsiaPackageResourcesInfo",
)

# The root of the workspace where the build was run.
#
# See https://bazel.build/docs/user-manual#running-executables.
_DEFAULT_SOURCE_SEARCH_ROOT = "BUILD_WORKSPACE_DIRECTORY"

FUCHSIA_DEBUG_SYMBOLS_ATTRS = {
    "_elf_strip_tool": attr.label(
        default = "@fuchsia_rules_common//debug_symbols/tools:elf_strip",
        executable = True,
        cfg = "exec",
    ),
    "_generate_symbols_dir_tool": attr.label(
        default = "@fuchsia_rules_common//debug_symbols/tools:generate_symbols_dir",
        executable = True,
        cfg = "exec",
    ),
    "_cc_toolchain": attr.label(
        default = Label("@bazel_tools//tools/cpp:current_cc_toolchain"),
    ),
}

def strip_resources(ctx, resources, build_id_path = None, source_search_root = _DEFAULT_SOURCE_SEARCH_ROOT):
    """Generate an action to strip resources.

    The generated action will output a single ".build-id" directory that will contain
    symlinks to all unstripped ELF binaries from the given `resources`. This action will
    always generate a directory even if there are no resources to strip.

    In addition, the action will generate a file in the ".build-id" directory named
    ".stamp", which will contain the full names of all of the debug symbols that were
    generated.

    Args:
      ctx: Rule context.
      resources: A list of unstripped input resource_struct() values.
      build_id_path: (optional) A string that will be used when declaring
        the build ID directory. Defaults to `ctx.label.name + "/.build-id"`.
      source_search_root: (optional) Either a string or File value, see
        `FuchsiaDebugSymbolInfo` documentation.

    Returns:
      A pair whose first item is a list of stripped resource_struct() instances,
      and the second item is a `FuchsiaDebugSymbolInfo` provider for the
      corresponding ".build-id" directory that contains a single
      (source_search_root, build_dirs_depset) pair.
    """
    elf_strip_tool = ctx.executable._elf_strip_tool
    generate_symbols_dir_tool = ctx.executable._generate_symbols_dir_tool

    build_id_path = build_id_path or (ctx.label.name + "/.build-id")

    if type(build_id_path) != "string":
        fail("'{}' must be a string but got {}.".format(build_id_path, type(build_id_path)))

    build_id_dir = ctx.actions.declare_directory(build_id_path)

    stripped_resources = []
    all_maybe_elf_files = []
    all_ids_txt = []

    # We need to make sure we have a unique set of inputs. If we have duplicate
    # resources, the `ctx.actions.declare_file` below will fail because it
    # will try to declare the same file twice. We only need to strip the resource
    # once so there is no need to attempt to strip duplicates.
    for r in depset(resources).to_list():
        ids_txt = ctx.actions.declare_file(r.src.path + ".ids_txt")
        all_ids_txt.append(ids_txt)
        all_maybe_elf_files.append(r.src)
        stripped_resources.append(_maybe_process_elf(ctx, r, ids_txt, elf_strip_tool))

    ctx.actions.run(
        executable = generate_symbols_dir_tool,
        arguments = [build_id_dir.path] + [f.path for f in all_ids_txt],
        outputs = [build_id_dir],
        inputs = all_ids_txt + all_maybe_elf_files,
        mnemonic = "GenerateDebugSymbols",
        progress_message = "Generate dir with debug symbols for %s" % ctx.label,
    )

    if type(source_search_root) not in ("File", "string"):
        fail("The 'source_search_root' argument should be a string or a File value, got: %s" % repr(source_search_root))

    return stripped_resources, FuchsiaDebugSymbolInfo(build_id_dirs_mapping = {
        source_search_root: depset([build_id_dir]),
    })

def _maybe_process_elf(ctx, r, ids_txt, elf_strip_tool):
    cc_toolchain = find_cc_toolchain(ctx)
    stripped = ctx.actions.declare_file(r.src.path + "_stripped")

    ctx.actions.run(
        outputs = [stripped, ids_txt],
        inputs = [r.src],
        tools = cc_toolchain.all_files,
        executable = elf_strip_tool,
        progress_message = "Extracting debug symbols from %s" % r.src,
        mnemonic = "ExtractDebugFromELF",
        arguments = [
            cc_toolchain.objcopy_executable,
            r.src.path,
            stripped.path,
            ids_txt.path,
        ],
    )

    return make_resource_struct(
        src = stripped,
        dest = r.dest,
    )

def merge_debug_symbol_infos(*targets_or_providers):
    """Merges debug symbol infos from targets or providers.

    Finds `FuchsiaDebugSymbolInfo` provider instances in `targets_or_providers`
    and merges them into a single `FuchsiaDebugSymbolInfo` provider instance.

    Args:
        *targets_or_providers: A list whose flattened elements must be
            either a `FuchsiaDebugSymbolInfo` provider instance or a target.
    Returns:
        A new `FuchsiaDebugSymbolInfo` provider instance resulting from merging
        all of the `FuchsiaDebugSymbolInfo` provider instances from the inputs together.
    """

    # { source_search_root -> list[depset[build_id_dir]]}
    source_search_root_map = {}

    for target_or_provider in flatten(targets_or_providers):
        if type(target_or_provider) == "struct":
            provider = target_or_provider
            if hasattr(provider, "build_id_dirs_mapping"):
                # `target_or_provider` *is* a `FuchsiaDebugSymbolInfo` provider instance.
                build_id_dirs_map = provider.build_id_dirs_mapping
            else:
                # `target_or_provider` is a provider instance other than `FuchsiaDebugSymbolInfo`
                # or some other struct. This should not have been included.
                fail("Unexpected provider type or other `struct`: {}".format(repr(provider)))
        elif type(target_or_provider) == "Target":
            target = target_or_provider
            if FuchsiaDebugSymbolInfo in target:
                # `target_or_provider` *has* a `FuchsiaDebugSymbolInfo` provider instance.
                build_id_dirs_map = target[FuchsiaDebugSymbolInfo].build_id_dirs_mapping
            else:
                # `target_or_provider` is a Target but does not have debug
                # symbol info. Skip it.
                continue
        else:
            # `target_or_provider` is not a target or provider and should not have been included.
            fail("Unexpected type '{}' of provider/target value: {}".format(
                type(target_or_provider),
                repr(target_or_provider),
            ))

        for source_search_root, build_id_dirs_depset in build_id_dirs_map.items():
            if source_search_root not in source_search_root_map:
                source_search_root_map[source_search_root] = []
            source_search_root_map[source_search_root].append(build_id_dirs_depset)

    return FuchsiaDebugSymbolInfo(
        build_id_dirs_mapping = {
            source_search_root: depset(transitive = build_id_dirs_depsets)
            for source_search_root, build_id_dirs_depsets in source_search_root_map.items()
        },
    )

def _convert_fuchsia_unstripped_binary_info(binary_info):
    """Converts a `FuchsiaUnstrippedBinaryInfo` provider to a `FuchsiaCollectedUnstrippedBinariesInfo`.

    Args:
        binary_info: A `FuchsiaUnstrippedBinaryInfo` provider instance.
    Returns:
        A `FuchsiaCollectedUnstrippedBinariesInfo` provider instance containing
        info about the unstripped binary.
    """
    source_search_root = getattr(binary_info, "source_search_root", _DEFAULT_SOURCE_SEARCH_ROOT)
    return FuchsiaCollectedUnstrippedBinariesInfo(
        source_search_root_to_unstripped_binaries = {
            source_search_root: depset([
                struct(
                    dest = binary_info.dest,
                    unstripped_file = binary_info.unstripped_file,
                    stripped_file = binary_info.stripped_file,
                ),
            ]),
        },
    )

def _convert_fuchsia_unstripped_binaries_info(binaries_info):
    """Converts a FuchsiaUnstrippedBinariesInfo provider to a FuchsiaCollectedUnstrippedBinariesInfo.

    Args:
        binaries_info: A FuchsiaUnstrippedBinariesInfo provider instance.
    Returns:
        A FuchsiaCollectedUnstrippedBinariesInfo provider instance containing the info about
        converted unstripped binaries.
    """
    source_search_root_to_binary_info_map = {}
    for binary_info in binaries_info.binaries:
        source_search_root = getattr(binary_info, "source_search_root", _DEFAULT_SOURCE_SEARCH_ROOT)
        binary_info = struct(
            dest = binary_info.dest,
            unstripped_file = binary_info.unstripped_file,
            stripped_file = getattr(binary_info, "stripped_file", None),
        )
        source_search_root_to_binary_info_map.setdefault(source_search_root, []).append(
            binary_info,
        )

    return FuchsiaCollectedUnstrippedBinariesInfo(
        source_search_root_to_unstripped_binaries = {
            source_search_root: depset(binary_infos)
            for source_search_root, binary_infos in source_search_root_to_binary_info_map.items()
        },
    )

def _merge_unstripped_binaries_infos(*targets_or_providers):
    """Merges collected info for unstripped binaries from targets or providers, or lists.

    Finds `FuchsiaCollectedUnstrippedBinariesInfo`, `FuchsiaUnstrippedBinariesInfo`, and
    `FuchsiaUnstrippedBinaryInfo` provider instances in `targets_or_providers` and adds info
    about all the unstripped binaries into a `FuchsiaCollectedUnstrippedBinariesInfo`
    provider instance.

    Handles both provider instances and targets that have them.

    `FuchsiaUnstrippedBinaryInfo` and `FuchsiaUnstrippedBinariesInfo` provider instances are
    converted into `FuchsiaCollectedUnstrippedBinariesInfo` provider instances for the merge.

    Args:
        *targets_or_providers: A list whose flattened elements must be
            `FuchsiaCollectedUnstrippedBinariesInfo`, `FuchsiaUnstrippedBinariesInfo`, or
            `FuchsiaUnstrippedBinaryInfo` provider instances or targets.
    Returns:
        A new `FuchsiaCollectedUnstrippedBinariesInfo` provider instance, merging the content of
        the input arguments.
    """

    # A `Map { source_search_root -> list[depset[struct(dest, unstripped_file, stripped_file)]] }`.
    # This is used to populate the `source_search_root_to_unstripped_binaries`
    # field in the returned provider.
    source_search_root_map = {}

    for target_or_provider in flatten(targets_or_providers):
        if type(target_or_provider) == "struct":
            provider = target_or_provider
            if hasattr(provider, "source_search_root_to_unstripped_binaries"):
                # `target_or_provider` *is* a `FuchsiaCollectedUnstrippedBinariesInfo` provider instance.
                collected_info = provider
            elif hasattr(provider, "binaries"):
                # `target_or_provider` *is* a `FuchsiaUnstrippedBinariesInfo` provider instance.
                collected_info = _convert_fuchsia_unstripped_binaries_info(provider)
            elif hasattr(provider, "unstripped_file") and hasattr(provider, "dest"):
                # `target_or_provider` *is* a `FuchsiaUnstrippedBinaryInfo` provider instance.
                collected_info = _convert_fuchsia_unstripped_binary_info(provider)
            else:
                # `target_or_provider` is a provider instance other than `FuchsiaDebugSymbolInfo`
                # or some other struct. This should not have been included.
                fail("Unexpected provider type or other `struct`: {}".format(repr(provider)))
        elif type(target_or_provider) == "Target":
            target = target_or_provider
            if FuchsiaCollectedUnstrippedBinariesInfo in target:
                # `target_or_provider` *has* a `FuchsiaCollectedUnstrippedBinariesInfo` provider instance.
                collected_info = target[FuchsiaCollectedUnstrippedBinariesInfo]
            elif FuchsiaUnstrippedBinariesInfo in target:
                # `target_or_provider` *has* a `FuchsiaUnstrippedBinariesInfo` provider instance.
                collected_info = _convert_fuchsia_unstripped_binaries_info(
                    target[FuchsiaUnstrippedBinariesInfo],
                )
            elif FuchsiaUnstrippedBinaryInfo in target:
                # `target_or_provider` *has* a `FuchsiaUnstrippedBinaryInfo` provider instance.
                collected_info = _convert_fuchsia_unstripped_binary_info(
                    target[FuchsiaUnstrippedBinaryInfo],
                )
            else:
                # `target_or_provider` is a Target but does not have debug
                # symbol info. Skip it.
                continue
        else:
            # `target_or_provider` is not a target or provider and should not have been included.
            fail("Unexpected type '{}' of provider/target value: {}".format(
                type(target_or_provider),
                repr(target_or_provider),
            ))

        for source_search_root, binary_info_depset in collected_info.source_search_root_to_unstripped_binaries.items():
            if source_search_root not in source_search_root_map:
                source_search_root_map[source_search_root] = []
            source_search_root_map[source_search_root].append(binary_info_depset)

    return FuchsiaCollectedUnstrippedBinariesInfo(
        source_search_root_to_unstripped_binaries = {
            source_search_root: depset(transitive = binary_info_depsets)
            for source_search_root, binary_info_depsets in source_search_root_map.items()
        },
    )

# A map of rule kind strings to tuples of attribute names for possible dependencies.
# Used by _get_target_deps_from_attributes() below.
_KNOWN_RULE_KINDS_TO_DEP_ATTR_NAMES = {
    "filegroup": ("data", "deps", "srcs"),
    "cc_binary": ("data", "deps", "srcs", "additional_linker_inputs", "dynamic_dep", "link_extra_libs", "malloc", "reexport_deps", "win_def_file"),
    "cc_import": ("data", "deps", "hdrs", "interface_library", "objects", "pic_objects", "pic_static_library", "shared_library", "static_library"),
    "cc_library": ("data", "deps", "srcs", "hdrs", "additional_compiler_inputs", "additional_linker_inputs", "implementation_deps", "linkstamp", "textual_hdrs", "win_def_file"),
    "cc_proto_library": ("deps",),
    "cc_shared_library": ("deps", "additional_linker_inputs", "dynamic_deps", "roots", "win_def_file"),
    "cc_test": ("deps", "srcs", "data", "additional_linker_inputs", "dynamic_deps", "link_extra_libs", "malloc", "reexport_deps", "win_def_file"),
}

def _get_target_deps_from_attributes(rule_attr, rule_kind = None):
    """Retrieves target dependencies from target attributes using known rule patterns.

    Args:
        rule_attr: Target attributes dictionary.
        rule_kind: Optional string rule kind.
    Returns:
        A list of dependent Targets.
    """
    return get_target_deps_from_attributes(rule_attr, rule_kind, known_rule_kinds = _KNOWN_RULE_KINDS_TO_DEP_ATTR_NAMES)

def _fuchsia_collect_unstripped_binaries_aspect_impl(target, aspect_ctx):
    """Aspect implementation to collect unstripped binaries across dependencies.

    Args:
        target: Target being analyzed.
        aspect_ctx: Aspect context.
    Returns:
        A `FuchsiaCollectedUnstrippedBinariesInfo` provider instance.
    """
    return _merge_unstripped_binaries_infos(
        target,
        _get_target_deps_from_attributes(aspect_ctx.rule.attr, aspect_ctx.rule.kind),
    )

_fuchsia_collect_unstripped_binaries_aspect = aspect(
    doc = """Collect FuchsiaUnstrippedBinaryInfo values across a DAG of dependencies,
        and provide a corresponding FuchsiaCollectedUnstrippedBinariesInfo value.""",
    implementation = _fuchsia_collect_unstripped_binaries_aspect_impl,
    attr_aspects = ["*"],
    provides = [FuchsiaCollectedUnstrippedBinariesInfo],
)

def _find_and_process_unstripped_binaries_impl(ctx):
    all_collected_unstripped_binaries_info = _merge_unstripped_binaries_infos(ctx.attr.deps)

    prebuilt_resources = []

    # list[resource_struct]
    generated_resources = []

    # list[FuchsiaDebugSymbolInfo] covering the symbols of all stripped binaries.
    stripped_debug_symbol_infos = []

    for source_search_root, unstripped_depset in all_collected_unstripped_binaries_info.source_search_root_to_unstripped_binaries.items():
        resources_to_strip = []

        for unstripped in unstripped_depset.to_list():
            if unstripped.stripped_file != None:
                prebuilt_resources.append(
                    make_resource_struct(dest = unstripped.dest, src = unstripped.stripped_file),
                )
            else:
                resources_to_strip.append(
                    make_resource_struct(dest = unstripped.dest, src = unstripped.unstripped_file),
                )

        if not resources_to_strip:
            continue

        stripped_resources, debug_symbol_info = strip_resources(
            ctx,
            resources_to_strip,
            source_search_root = source_search_root,
        )
        generated_resources.extend(stripped_resources)
        stripped_debug_symbol_infos.append(debug_symbol_info)

    outputs = depset(
        direct = [r.src for r in generated_resources],
        # `strip_resources` creates a `FuchsiaDebugSymbolInfo` containing a mapping with a single
        # (key, value) pair. The value is a `depset` containing the generated ".build-id" directory.
        transitive = [debug_symbol_info.build_id_dirs_mapping.values()[0] for debug_symbol_info in stripped_debug_symbol_infos],
    )

    result = [
        DefaultInfo(files = outputs),
        FuchsiaPackageResourcesInfo(resources = prebuilt_resources + generated_resources),
        merge_debug_symbol_infos(stripped_debug_symbol_infos),
        all_collected_unstripped_binaries_info,  # A `FuchsiaCollectedUnstrippedBinariesInfo` provider instance.
    ]
    return result

find_and_process_unstripped_binaries = rule(
    doc = """Collects unstripped binary info from a DAG of targets.

        Find all targets providing `FuchsiaUnstrippedBinaryInfo` from the DAG of
        dependencies starting from `deps`.

        Then generate actions to strip those that need it, plus other actions to
        generate a ".build-id" directory populated with symlinks to the original
        unstripped files.

        Returns a `FuchsiaPackageResourcesInfo` provider to list all stripped binaries
        and their installation path (as used by `fuchsia_package()`).

        Returns a `FuchsiaDebugSymbolInfo` provider to list the ".build-id" directories
        and the corresponding source search roots.

        Returns a `FuchsiaCollectedUnstrippedBinariesInfo` provider listing
        information about all the collected files.
        """,
    implementation = _find_and_process_unstripped_binaries_impl,
    toolchains = ["@bazel_tools//tools/cpp:toolchain_type"],
    provides = [
        DefaultInfo,
        FuchsiaPackageResourcesInfo,
        FuchsiaDebugSymbolInfo,
        FuchsiaCollectedUnstrippedBinariesInfo,
    ],
    attrs = {
        "deps": attr.label_list(
            doc = "A list of roots for the DAG of dependencies to scan.",
            mandatory = True,
            aspects = [_fuchsia_collect_unstripped_binaries_aspect],
        ),
    } | FUCHSIA_DEBUG_SYMBOLS_ATTRS,
)

# Debug Symbols Collection aspect & helpers

def transform_collected_debug_symbols_infos(*targets):
    """Transforms a list of targets processed by an aspect into a `FuchsiaDebugSymbolInfo`.

    Given a list of targets that have had the `fuchsia_collect_all_debug_symbols_infos_aspect`
    run against them, collect all the debug symbols into a single `FuchsiaDebugSymbolInfo`.

    Args:
      *targets: A list of targets. It is ok to pass a list that contains None values

    Returns:
      A `FuchsiaDebugSymbolInfo` provider instance.
    """
    valid_targets = []
    for target_or_list in targets:
        for t in (target_or_list if type(target_or_list) == "list" else [target_or_list]):
            if t and FuchsiaCollectedDebugSymbolsInfo in t:
                valid_targets.append(t)

    return merge_debug_symbol_infos(
        flatten([
            t[FuchsiaCollectedDebugSymbolsInfo].collected_symbols.to_list()
            for t in valid_targets
        ]),
    )

def _fuchsia_collect_all_debug_symbols_infos_aspect_impl(target, ctx):
    return FuchsiaCollectedDebugSymbolsInfo(
        collected_symbols = depset(
            direct = [target[FuchsiaDebugSymbolInfo]] if FuchsiaDebugSymbolInfo in target else [],
            transitive = [t[FuchsiaCollectedDebugSymbolsInfo].collected_symbols for t in get_target_deps_from_attributes(
                ctx.rule.attr,
                ctx.rule.kind,
            ) if FuchsiaCollectedDebugSymbolsInfo in t],
        ),
    )

fuchsia_collect_all_debug_symbols_infos_aspect = aspect(
    doc = """Collects all of the `FuchsiaDebugSymbolInfo` providers in the graph.

    Walks the dependency tree finding all of the targets that expose the
    `FuchsiaDebugSymbolInfo` provider and collect them into a single
    `FuchsiaCollectedDebugSymbolsInfo` provider instance.

    To convert the collected resources back into a `FuchsiaDebugSymbolInfo`
    object, call the `transform_collected_debug_symbols_infos()` function with
    the top level target(s) as the argument.
    """,
    implementation = _fuchsia_collect_all_debug_symbols_infos_aspect_impl,
    attr_aspects = ["*"],
)

def _fuchsia_unstripped_binary_impl(ctx):
    return [
        FuchsiaUnstrippedBinaryInfo(
            dest = ctx.attr.dest,
            unstripped_file = ctx.file.unstripped_file,
            stripped_file = ctx.file.stripped_file if ctx.attr.stripped_file else None,
            source_search_root = ctx.attr.source_search_root,
        ),
    ]

fuchsia_unstripped_binary = rule(
    doc = "A rule that provides FuchsiaUnstrippedBinaryInfo wrapping the given files.",
    implementation = _fuchsia_unstripped_binary_impl,
    provides = [FuchsiaUnstrippedBinaryInfo],
    attrs = {
        "dest": attr.string(
            doc = "Installation location in Fuchsia package for the stripped binary.",
            mandatory = True,
        ),
        "unstripped_file": attr.label(
            doc = "Unstripped ELF binary file",
            mandatory = True,
            allow_single_file = True,
        ),
        "stripped_file": attr.label(
            doc = "Optional stripped ELF binary file, if available as prebuilt.",
            mandatory = False,
            allow_single_file = True,
        ),
        "source_search_root": attr.label(
            doc = "Optional label to source directory or file inside source directory.",
            mandatory = False,
            allow_single_file = True,
        ),
    },
)

def _fuchsia_debug_symbols_impl(ctx):
    return [
        FuchsiaDebugSymbolInfo(build_id_dirs_mapping = {
            ctx.file.source_search_root: depset(ctx.files.build_id_dirs),
        }),
    ]

# NOTE: fuchsia_debug_symbols() is part of the public Bazel SDK API.
# If changes to the implementation are needed for in-tree use cases, they
# should be done in a way that doesn't break this public API, or a separate
# internal rule (e.g., fx_debug_symbols()) should be introduced instead.
fuchsia_debug_symbols = rule(
    doc = """Rule-based constructor for FuchsiaDebugSymbolInfo.""",
    implementation = _fuchsia_debug_symbols_impl,
    attrs = {
        "build_id_dirs": attr.label_list(
            doc = "A list of build-id directory files.",
            allow_files = True,
            mandatory = True,
        ),
        "source_search_root": attr.label(
            doc = "A label to a file in the directory where sources can be found.",
            allow_single_file = True,
            mandatory = True,
        ),
    },
)
