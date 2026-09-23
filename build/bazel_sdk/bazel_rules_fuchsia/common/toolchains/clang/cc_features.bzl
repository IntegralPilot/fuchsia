# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Clang C++ toolchain feature definitions."""

load("@bazel_tools//tools/build_defs/cc:action_names.bzl", "ACTION_NAMES")
load(
    "@bazel_tools//tools/cpp:cc_toolchain_config_lib.bzl",
    "feature",
    "flag_group",
    "flag_set",
    "with_feature_set",
)

_all_actions = [
    ACTION_NAMES.assemble,
    ACTION_NAMES.preprocess_assemble,
    ACTION_NAMES.c_compile,
    ACTION_NAMES.cpp_compile,
    ACTION_NAMES.cpp_module_compile,
    ACTION_NAMES.objc_compile,
    ACTION_NAMES.objcpp_compile,
    ACTION_NAMES.cpp_header_parsing,
    ACTION_NAMES.clif_match,
]

_all_compile_actions = [
    ACTION_NAMES.assemble,
    ACTION_NAMES.preprocess_assemble,
    ACTION_NAMES.linkstamp_compile,
    ACTION_NAMES.c_compile,
    ACTION_NAMES.cpp_compile,
    ACTION_NAMES.cpp_header_parsing,
    ACTION_NAMES.cpp_module_compile,
    ACTION_NAMES.cpp_module_codegen,
    ACTION_NAMES.lto_backend,
    ACTION_NAMES.clif_match,
]

_all_conly_compile_actions = [
    ACTION_NAMES.c_compile,
    ACTION_NAMES.objc_compile,
]

_all_cpp_compile_actions = [
    ACTION_NAMES.linkstamp_compile,
    ACTION_NAMES.cpp_compile,
    ACTION_NAMES.cpp_header_parsing,
    ACTION_NAMES.cpp_module_compile,
    ACTION_NAMES.cpp_module_codegen,
    ACTION_NAMES.lto_backend,
    ACTION_NAMES.clif_match,
]

_all_link_actions = [
    ACTION_NAMES.cpp_link_executable,
    ACTION_NAMES.cpp_link_dynamic_library,
    ACTION_NAMES.cpp_link_nodeps_dynamic_library,
]

def _make_flag_config(
        *,
        cflags = [],
        conlyflags = [],
        ccflags = [],
        ldflags = [],
        combine_cflags_with_ldflags = True):
    """ Create a struct holding all compiler and linker flags.

    This struct can be seen as a GN config, grouping compiler and linker
    flags together,

    The method _iter_config_flags below can be used to extract the
    corresponding flag_group() lists from an input list of struct values.

    Args:
       cflags (string list, optional): Common C and C++ compiler flags.
       ccflags (string list, optional): C++ only compiler flags.
       conlyflags (string list, optional): C compiler flags (omitted from C++ compilations).
       ldflags (string list, optional): linker flags.

       combine_cflags_with_ldflags (bool, optional): if True (the default), the values
          in cflags will be prepended to ldflags as well.

    Returns:
       A new struct with cflags, ccflags, ldflags keys, and whose values are either None
       of list of Bazel flag_group() values.
    """

    def _flag_group_or_none(flags):
        return flag_group(flags = flags) if len(flags) > 0 else None

    return struct(
        cflags = _flag_group_or_none(cflags),
        conlyflags = _flag_group_or_none(conlyflags),
        ccflags = _flag_group_or_none(ccflags),
        ldflags = _flag_group_or_none(
            (cflags if combine_cflags_with_ldflags else []) + ldflags,
        ),
    )

def _iter_config_flags(flag_name, flag_configs):
    """ Iterate over a list of config structs, and return the values for a given named flag.

    Args:
       flag_name: A flag name (e.g. "cflags", "ccflags", etc).
       flag_configs: A list of structs returned by _make_flag_config.
    Returns:
       A list of flag_group() values from the input configs's flag_name fields.
    """
    return [getattr(f, flag_name) for f in flag_configs if getattr(f, flag_name) != None]

def _iter_cflags(flag_configs):
    return _iter_config_flags("cflags", flag_configs)

def _iter_ccflags(flag_configs):
    return _iter_config_flags("ccflags", flag_configs)

def _iter_conlyflags(flag_configs):
    return _iter_config_flags("conlyflags", flag_configs)

def _iter_ldflags(flag_configs):
    return _iter_config_flags("ldflags", flag_configs)

def _apply_if(feature):
    """Generate `with_features` value that matches a given feature name.

    This is useful to define flag_set() values that are only applied when a
    specific feature is enabled. Example usage:

        flag_set(
            actions = _all_compile_actions,
            flag_groups = [ ... ],
            with_features = _apply_if("dbg"),
        ),

    Args:
        feature (string): Feature flag name.
    Returns:
        A list of one with_feature_set() value matching |feature|.
    """
    return [with_feature_set(
        features = [feature],
    )]

# A global struct providing various constant flag configs that
# do not depend on either the host or target os/cpu values.
_flag_configs = struct(
    color_diagnostics = _make_flag_config(
        cflags = ["-fcolor-diagnostics"],
        ldflags = ["-Wl,--color-diagnostics"],
    ),
    pic = _make_flag_config(
        cflags = ["-fPIC"],
    ),
    language_cxx17 = _make_flag_config(
        ccflags = ["-std=c++17"],
    ),
    language_cxx20 = _make_flag_config(
        ccflags = ["-std=c++20"],
    ),
    no_frame_pointers = _make_flag_config(
        cflags = ["-fomit-frame-pointer"],
    ),
    linker_gc = _make_flag_config(
        cflags = [
            "-fdata-sections",
            "-ffunction-sections",
        ],
        ldflags = ["-Wl,--gc-sections"],
    ),
    optimize_none = _make_flag_config(
        cflags = ["-O0"],
    ),
    optimize_debug = _make_flag_config(
        cflags = ["-Og"],
    ),
    optimize_default = _make_flag_config(
        cflags = ["-O2"],
    ),
    optimize_size = _make_flag_config(
        cflags = ["-Os"],
        ldflags = ["-Wl,-O2"],
    ),
    # LINT.IfChange
    debuginfo = _make_flag_config(
        cflags = [
            "-g3",
            "-gdwarf-5",
            "-gz=zstd",
            "-Xclang",
            "-debug-info-kind=constructor",
        ],
        ldflags = [
            "-g3",
            "-gdwarf-5",
            "-gz=zstd",
        ],
        combine_cflags_with_ldflags = False,
    ),
    # LINT.ThenChange(//build/bazel/debug_symbols/README.md)

    # LINT.IfChange(default_warnings)
    default_warnings = _make_flag_config(
        cflags = [
            # TODO(https://fxbug.dev/542707525): Suppress unused variable warnings in libc++ containers.
            "-D_LIBCPP_DISABLE_UNUSED_STRUCT_WARNINGS",
            "-Wall",
            "-Wextra",
            "-Wextra-semi",
            "-Wnewline-eof",
            # TODO(b/315062126) Some in-tree builds are failing because we
            # are shadowing variables.
            #"-Wshadow",
            "-Wstrict-prototypes",
            "-Wwrite-strings",
            "-Wno-sign-conversion",
            "-Wno-unused-parameter",
            "-Wnonportable-system-include-path",

            # This is still needed for -Wmaybe-uninitialized which is added as a pragma in source code.
            # Clang hasn't implemented this yet.
            "-Wno-unknown-warning-option",
            "-Wno-missing-field-initializers",

            # TODO(https://fxbug.dev/477786942): Fix violations and enable the following checks.
            # "-Wconversion",
            # "-Wimplicit-fallthrough",

            # TODO(https://fxbug.dev/500111548): Disable "-Wunused-but-set-global" until
            # all instances are fixed.
            "-Wno-unused-but-set-global",

            # TODO(https://fxbug.dev/534361287): Clean up unused templates in first-party code.
            # This is currently applied globally to unblock the toolchain update.
            # Eventually, this should be removed from default_warnings and only applied
            # to third_party targets.
            "-Wno-unused-template",
        ],
    ),
    # LINT.ThenChange(//build/config/BUILD.gn:default_warnings)
    werror = _make_flag_config(
        cflags = [
            "-Werror",
            "-Wa,--fatal-warnings",
        ],
    ),
    no_exceptions = _make_flag_config(
        ccflags = ["-fno-exceptions"],
        ldflags = ["-fno-exceptions"],
    ),
    no_rtti = _make_flag_config(
        ccflags = ["-fno-rtti"],
        ldflags = ["-fno-rtti"],
    ),
    symbol_visibility_hidden = _make_flag_config(
        cflags = ["-fvisibility=hidden"],
        ccflags = ["-fvisibility-inlines-hidden"],
        combine_cflags_with_ldflags = False,
    ),
    release = _make_flag_config(
        cflags = ["-DNDEBUG=1"],
        combine_cflags_with_ldflags = False,
    ),
    link_zircon = _make_flag_config(
        ldflags = ["-lzircon"],
    ),
    driver_mode = _make_flag_config(
        ldflags = ["--driver-mode=g++"],
    ),
    symbol_no_undefined = _make_flag_config(
        ldflags = ["-Wl,--no-undefined"],
    ),
    lto = _make_flag_config(
        cflags = [
            "-flto",
            "-fwhole-program-vtables",
            "-mllvm",
            "-wholeprogramdevirt-branch-funnel-threshold=0",
        ],
        ldflags = [
            "-flto",
            "-fwhole-program-vtables",
            "-Wl,-mllvm,--wholeprogramdevirt-branch-funnel-threshold=0",
        ],
        combine_cflags_with_ldflags = False,
    ),
    icf = _make_flag_config(
        ldflags = ["-Wl,--icf=all"],
    ),
    ffp_contract_off = _make_flag_config(
        cflags = ["-ffp-contract=off"],
        combine_cflags_with_ldflags = False,
    ),
    auto_var_init = _make_flag_config(
        cflags = ["-ftrivial-auto-var-init=pattern"],
        combine_cflags_with_ldflags = False,
    ),
    relpath_debug_info = _make_flag_config(
        # Relativize paths to source files and linker inputs to avoid
        # leaking absolute paths, and ensure consistency
        # between local and remote compiling/linking.
        cflags = [
            "-ffile-compilation-dir=.",
            "-no-canonical-prefixes",
        ],
        ccflags = [
            "-ffile-compilation-dir=.",
            "-no-canonical-prefixes",
        ],
        ldflags = [
            "-no-canonical-prefixes",
        ],
        combine_cflags_with_ldflags = False,
    ),
    thread_safety_annotations = _make_flag_config(
        cflags = [
            "-Wthread-safety",

            # TODO(https://fxbug.dev/42085252): Clang is catching instances of these in the kernel and drivers.
            # Temporarily disable them for now to facilitate the roll then come back and
            # fix them.
            "-Wno-unknown-warning-option",
            "-Wno-thread-safety-reference-return",
            "-D_LIBCPP_ENABLE_THREAD_SAFETY_ANNOTATIONS=1",
        ],
        combine_cflags_with_ldflags = False,
    ),
)

#
## Begin feature definitions
#

# This is a special feature in that Bazel will put all of these flags first
def get_default_compile_flags_feature(
        clang_info,
        target_os,
        sysroot = ""):
    """Compute the special "default_compile_flags" feature().

    This feature is special because Bazel will place all its flags before all
    others in corresponding actions.

    Args:
       clang_info: A ClangInfo provider value.
       target_os: Target OS, following Fuchsia conventions.
       sysroot: Optional path to sysroot to use.

    Returns:
       A new feature() value.
    """
    is_host = clang_info.fuchsia_host_os == target_os

    default_cflags = []
    default_conlyflags = []
    default_ccflags = []
    default_ldflags = []

    if sysroot:
        default_cflags.append(
            "--sysroot={}".format(sysroot),
        )
        default_ldflags.append(
            "--sysroot={}".format(sysroot),
        )

    default_system_flags = _make_flag_config(
        cflags = default_cflags,
        conlyflags = default_conlyflags,
        ccflags = default_ccflags,
        ldflags = default_ldflags,
        combine_cflags_with_ldflags = False,
    )

    return feature(
        name = "default_compile_flags",
        flag_sets = [
            # These are cflags that will be added to all builds
            flag_set(
                actions = _all_compile_actions,
                flag_groups = _iter_cflags(
                    [
                        default_system_flags,
                        _flag_configs.color_diagnostics,
                        _flag_configs.pic,
                        _flag_configs.linker_gc,
                        _flag_configs.no_frame_pointers,
                        _flag_configs.debuginfo,
                        _flag_configs.default_warnings,
                        _flag_configs.symbol_visibility_hidden,
                        _flag_configs.ffp_contract_off,
                        _flag_configs.auto_var_init,
                        _flag_configs.thread_safety_annotations,
                        _flag_configs.relpath_debug_info,
                    ] + (
                        [
                            # TODO(b/430020292): Re-enable werror for
                            # strict-prototypes on host when we have a better
                            # way to address Go SDK compilation failure.
                            _make_flag_config(
                                cflags = ["-Wno-strict-prototypes"],
                            ),
                        ] if is_host else [_flag_configs.werror]
                    ),
                ),
            ),
            # These are conlyflags that will be added to all builds
            flag_set(
                actions = _all_conly_compile_actions,
                flag_groups = _iter_conlyflags([
                    default_system_flags,
                ]),
            ),
            # These are ccflags that will be added to all builds
            flag_set(
                actions = _all_cpp_compile_actions,
                flag_groups = _iter_ccflags([
                    default_system_flags,
                    _flag_configs.language_cxx20,
                    _flag_configs.no_exceptions,
                    _flag_configs.no_rtti,
                    _flag_configs.symbol_visibility_hidden,
                    _flag_configs.relpath_debug_info,
                ]),
            ),
            # These are cflags that will be added to dbg builds
            flag_set(
                actions = _all_compile_actions,
                flag_groups = _iter_cflags([
                    _flag_configs.optimize_debug,
                ]),
                with_features = _apply_if("dbg"),
            ),
            # These are cflags that will be added to opt builds
            flag_set(
                actions = _all_compile_actions,
                flag_groups = _iter_cflags([
                    _flag_configs.optimize_size,
                    _flag_configs.release,
                    # TODO(b/299545705) turn on LTO for all opt builds
                    # _flag_configs.lto,
                ]),
                with_features = _apply_if("opt"),
            ),

            # Begin link Actions:
            # Note: The link actions must be added to the 'default_compile_flags' feature.
            # Bazel will move all of these to the top of the command linke which makes it
            # possible for users to override certain flags.

            # These are ldflags that are applied to all builds
            flag_set(
                actions = _all_link_actions,
                flag_groups = _iter_ldflags([
                    default_system_flags,
                    _flag_configs.driver_mode,
                    _flag_configs.color_diagnostics,
                    _flag_configs.no_frame_pointers,
                    _flag_configs.linker_gc,
                    _flag_configs.debuginfo,
                    _flag_configs.no_exceptions,
                    _flag_configs.no_rtti,
                    _flag_configs.pic,
                    _flag_configs.icf,
                    _flag_configs.relpath_debug_info,
                ]) + (
                    _iter_ldflags([
                        _flag_configs.link_zircon,
                    ]) if target_os == "fuchsia" else []
                ),
            ),
            # This is subtle, normally all binaries (executables and
            # shared libraries) should be linked with --no-undefined.
            #
            # However, on host platforms, sanitizer runtimes are normally
            # linked statically to executables, and are *not* linked to
            # shared libraries, expecting symbols like __asan_init() to
            # be provided by the executable itself.
            #
            # This implies that shared library links cannot use --no-undefined
            # when sanitizers are used, as linking them would always fail
            # otherwise. The two following flag_set() deal with this special
            # case.

            # Apply --no-undefined unconditionally to executables.
            flag_set(
                actions = [ACTION_NAMES.cpp_link_executable],
                flag_groups = _iter_ldflags([
                    _flag_configs.symbol_no_undefined,
                ]),
            ),
            # Apply --no-undefined to shared libraries only when no
            # clang sanitizers are used on the host.
            flag_set(
                actions = [
                    ACTION_NAMES.cpp_link_dynamic_library,
                    ACTION_NAMES.cpp_link_nodeps_dynamic_library,
                ],
                flag_groups = _iter_ldflags([
                    _flag_configs.symbol_no_undefined,
                ]),
                with_features = [
                    with_feature_set(
                        not_features = ["clang_sanitizer"],
                    ),
                ] if is_host else [],
            ),
            # See https://fxbug.dev/542560456: Linking Rust binaries when
            # Asan is enabled fails surprisingly with undefined symbols
            # __cxa_begin_catch, std::terminate() and __gxx_personality_v0
            # referenced from the host libclang_rt.asan_cxx.a library.
            #
            # These symbols are provided by libc++ which is normally an
            # implicit dependency to all link actions when --driver-mode=g++
            # is used. For some reason, this doesn't work, so add an explicit
            # -lc++ here to work-around this.
            flag_set(
                actions = [
                    ACTION_NAMES.cpp_link_executable,
                    ACTION_NAMES.cpp_link_dynamic_library,
                    ACTION_NAMES.cpp_link_nodeps_dynamic_library,
                ],
                flag_groups = [flag_group(flags = ["-lc++"])] if is_host else [],
                with_features = [
                    with_feature_set(
                        features = ["clang_sanitizer"],
                    ),
                ],
            ),
            # These are ldflags that will be added to dbg builds
            flag_set(
                actions = _all_link_actions,
                flag_groups = _iter_ldflags([
                    _flag_configs.optimize_debug,
                ]),
                with_features = _apply_if("dbg"),
            ),
            # These are ldflags that will be added to opt builds
            flag_set(
                actions = _all_link_actions,
                flag_groups = _iter_ldflags([
                    _flag_configs.optimize_size,
                    # TODO(b/299545705) turn on LTO for all opt builds
                    # _flag_configs.lto,
                ]),
                with_features = _apply_if("opt"),
            ),
        ],
        enabled = True,
        implies = [
            # LINT.IfChange(target_system_name)
            "target_system_name",
            # LINT.ThenChange(toolchain_utils.bzl)
        ],
    )

def get_fuchsia_api_level_feature(api_level):
    """Creates a feature that adds the target Fuchsia API level flag.

    Args:
       api_level: The integer representation of the target Fuchsia API level.

    Returns:
       A feature() object.
    """
    return feature(
        name = "fuchsia_api_level",
        enabled = True,
        flag_sets = [
            flag_set(
                actions = _all_compile_actions,
                flag_groups = [
                    flag_group(
                        flags = ["-ffuchsia-api-level={}".format(api_level)],
                    ),
                ],
            ),
        ],
    )

action_names = struct(
    all_actions = _all_actions,
    all_compile_actions = _all_compile_actions,
    all_conly_compile_actions = _all_conly_compile_actions,
    all_cpp_compile_actions = _all_cpp_compile_actions,
    all_link_actions = _all_link_actions,
)
