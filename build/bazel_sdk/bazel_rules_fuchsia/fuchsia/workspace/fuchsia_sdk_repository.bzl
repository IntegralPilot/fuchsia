# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines a WORKSPACE rule for loading a version of the Fuchsia IDK."""

load(
    "//fuchsia/workspace:register_fuchsia_sdk_toolchain.bzl",
    "fuchsia_toolchain_decl",
)
load("//fuchsia/workspace:utils.bzl", "symlink_or_copy", "workspace_path")
load(
    "//fuchsia/workspace/sdk_templates:generate_sdk_build_rules.bzl",
    "generate_sdk_repository",
    "resolve_repository_labels",
)

# Environment variable used to set a local Fuchsia SDK directory. If this
#  variable is set, it should point to
# <FUCHSIA_DIR>/out/<BUILD_DIR> where "fuchsia_sdk" is built.

_LOCAL_FUCHSIA_SDK_DIRECTORY = "LOCAL_FUCHSIA_SDK_DIRECTORY"

# Environment variable used to set a local Fuchsia IDK output directory. If this
# variable is set, it should point to the root of a Fuchsia IDK
# <FUCHSIA_DIR>/out/<BUILD_DIR> where "fuchsia_sdk" is built. In particular we
# will look for
#
#     <LOCAL_FUCHSIA_IDK_DIRECTORY>/"meta/manifest.json"
_LOCAL_FUCHSIA_IDK_DIRECTORY = "LOCAL_FUCHSIA_IDK_DIRECTORY"

def _instantiate_local_path(ctx, manifests):
    local_paths = ctx.attr.local_paths
    for local_path in local_paths:
        # Copies the SDK from a local Fuchsia platform build.
        if local_path[0] == "@":
            # Assume this is a file path inside the repository, e.g. @fuchsia_idk//:BUILD.bazel
            local_sdk = ctx.path(Label(local_path)).dirname
            ctx.report_progress("Copying local IDK from %s" % local_sdk)
        else:
            local_sdk_path = workspace_path(ctx, local_path)
            ctx.report_progress("Copying local SDK from %s" % local_sdk_path)
            local_sdk = ctx.path(local_sdk_path)
        if not local_sdk.exists:
            fail("Cannot find SDK in local directory: %s\n\nPlease build it with\n\n\t\t'fx build //sdk:final_fuchsia_sdk' or similar." % local_sdk)

        manifests.append({"root": "%s/." % local_sdk, "manifest": "meta/manifest.json"})

    # If local_sdk_version_file is specified, make Bazel pick it up as a dep.
    if ctx.attr.local_sdk_version_file:
        ctx.path(ctx.attr.local_sdk_version_file)

def _instantiate_local_env(ctx):
    local_fuchsia_sdk = ctx.os.environ.get(_LOCAL_FUCHSIA_SDK_DIRECTORY)

    # buildifier: disable=print
    print("WARNING: using local SDK from %s" % local_fuchsia_sdk)
    local_sdk = ctx.path(local_fuchsia_sdk)
    ctx.report_progress("Copying local fuchsia_sdk from %s" % local_sdk)
    if not local_sdk.exists:
        fail("Cannot find SDK in local directory: %s\n\nPlease build it with\n\n\t\t'fx build //sdk:final_fuchsia_sdk' or similar or unset variable '%s'." % (local_sdk, _LOCAL_FUCHSIA_SDK_DIRECTORY))
    ctx.symlink(local_sdk, ".")

def _instantiate_local_idk(ctx, manifests):
    local_fuchsia_idk_dir = ctx.os.environ.get(_LOCAL_FUCHSIA_IDK_DIRECTORY)

    # buildifier: disable=print
    print("WARNING: using local IDK from %s" % local_fuchsia_idk_dir)
    ctx.report_progress("Copying local IDK from %s" % local_fuchsia_idk_dir)
    local_idk = ctx.path(local_fuchsia_idk_dir)
    if not local_idk.exists:
        fail("Cannot find IDK in: %s\n" % local_idk)
    manifests.append({"root": "%s/." % local_idk, "manifest": "meta/manifest.json"})

def _get_starlark_runtime_for(ctx, copy_content_strategy):
    """Return a runtime value usable with generate_sdk_build_rules.bzl functions.

    See the technical note at the start of said script describing what the result
    should look like.

    Args:
       ctx: The current repository_ctx value.
       copy_content_strategy: Argument to symlink_or_copy()
    Returns:
       A new struct that can be used as the first argument to most
       functions provided by generate_sdk_build_rules.bzl.
    """

    def _workspace_path(path):
        if path.startswith("@"):
            # This must point to a file inside the directory of interest.
            # e.g. @<repo_name>//:BUILD.bazel --> path to external/repo_name
            # This is necessary for parent_sdk_local_paths handling.
            return ctx.path(Label(path)).dirname

        return ctx.path(workspace_path(ctx, path))

    def _label_to_path(label):
        return ctx.path(Label(label))

    def _file_copier(files_to_copy):
        symlink_or_copy(ctx, copy_content_strategy, files_to_copy)

    def _json_decode(json_str):
        return json.decode(json_str)

    def _value_is_dict(v):
        return type(v) == "dict"

    def _value_is_list(v):
        return type(v) == "list"

    def _value_is_struct(v):
        return type(v) == "struct"

    def _find_repository_files_by_name(file_name):
        ret = ctx.execute(["find", "-L", ".", "-type", "f", "-name", file_name])
        if ret.return_code:
            return []
        return ret.stdout.strip().split("\n")

    def _run_buildifier(buildifier_flags):
        ret = ctx.execute(
            [str(ctx.path(ctx.attr.buildifier))] + buildifier_flags,
            quiet = False,
        )
        return ret.return_code == 0

    return struct(
        ctx = ctx,
        make_struct = struct,
        fail = fail,
        value_is_dict = _value_is_dict,
        value_is_list = _value_is_list,
        value_is_struct = _value_is_struct,
        workspace_path = _workspace_path,
        label_to_path = _label_to_path,
        file_copier = _file_copier,
        json_decode = _json_decode,
        find_repository_files_by_name = _find_repository_files_by_name,
        run_buildifier = _run_buildifier if ctx.attr.buildifier else None,
    )

def _fuchsia_sdk_repository_impl(ctx):
    if _LOCAL_FUCHSIA_SDK_DIRECTORY in ctx.os.environ:
        copy_content_strategy = "copy"
        _instantiate_local_env(ctx)
        return

    # Compute the set of input IDK manifests to use for this repository.
    manifests = []

    if _LOCAL_FUCHSIA_IDK_DIRECTORY in ctx.os.environ:
        copy_content_strategy = "symlink"
        _instantiate_local_idk(ctx, manifests)
    elif ctx.attr.local_paths:
        copy_content_strategy = "symlink"
        _instantiate_local_path(ctx, manifests)
    else:
        fail("The fuchsia sdk no longer supports downloading content via the cipd tool. Please use local_paths or provide a local fuchsia build.")

    runtime = _get_starlark_runtime_for(ctx, copy_content_strategy)

    # Resolve labels early to avoid repository rule restarts.
    resolve_repository_labels(runtime)

    generate_sdk_repository(runtime, manifests)

fuchsia_sdk_repository = repository_rule(
    doc = """
Loads a particular version of the Fuchsia IDK.
""",
    implementation = _fuchsia_sdk_repository_impl,
    environ = [_LOCAL_FUCHSIA_SDK_DIRECTORY],
    configure = True,
    attrs = {
        "parent_sdk": attr.label(
            doc =
                """
                If specified, libraries in current SDK that also exist in the parent SDK will always resolve to the parent. In practice,
                this means that a library defined in the current SDK that is also defined in parent_sdk will be ignored in the current SDK,
                and references to it will be replaced with @<parent_sdk>//<library>. This is useful when SDKs are layered, for example an
                internal SDK and a public SDK.
                """,
            mandatory = False,
        ),
        "parent_sdk_local_paths": attr.string_list(
            doc =
                """
                If parent_sdk is specified, parent_sdk_local_paths has to contain the same values as the local_paths attribute of the parent SDK.
                This is required because Bazel does not have a way to evaluate the existence of a Label, so we process the metadata of the parent
                SDK again when using layered SDKs.
                TODO: look for a better approach if this is limiting or causing performance issues.
                """,
            mandatory = False,
        ),
        "local_paths": attr.string_list(
            doc = "Paths to local SDK directories.",
        ),
        "local_sdk_version_file": attr.label(
            doc = "An optional file used to mark the version of the SDK pointed to by local_paths.",
            allow_single_file = True,
        ),
        "buildifier": attr.label(
            doc = "An optional label to the buildifier tool, used to reformat all generated Bazel files.",
            allow_single_file = True,
        ),
        "visibility_templates": attr.string_list_dict(
            doc = """Allows for the addition of additional visibility lists.

            This attribute lets the caller specify a set of additional visibility
            parameters to be passed into the templates when we generate the repositry.
            For a list of keys refer to the generate_sdk_build_rules.bzl file.

            When you pass in the labels you must specify the fully qualified name of
            the label to ensure that we reference the correct labels. For example,
            if you are wanting to refer to a target within the root repository you should
            use a path like @@//foo:__pkg__. If you were to pass in this value as
            //foo:__pkg__ then it will be passed into the template in a way that makes
            bazel think it belongs to the repository you are generating.

            The values in this list can contain a wildcard ("*") expanded out to include
            all of the directories at that location but there are restrictions to the
            usage.
              - The expansion will only work on repos that are name "@@".
              - The expansion only supports a single "*"
              - The expansion will check that a BUILD.bazel file exists at the location
                  that is being expanded and will add it if true but will not check that
                  the BUILD.bazel file contains the target that is being expanded.

            Examples of the wildcard expansion are:
              - @@//src/*/foo:bar
              - @@//src/*:bar
              - @@//src/*/foo
            """,
        ),
    },
)

def _fuchsia_sdk_repository_ext(ctx):
    local_paths = None
    local_sdk_version_file = None
    buildifier = None
    visibility_templates = None

    for mod in ctx.modules:
        # only the root module can set tags, and only one
        # of version() or local() tag can be used.
        if mod.is_root and len(mod.tags.local) > 0:
            local_paths = []
            for p in mod.tags.local:
                if p.path:
                    local_paths.append(p.path)

                if local_sdk_version_file and p.local_sdk_version_file:
                    fail("local_sdk_version_file set multiple times, got:\n{}\n{}".format(
                        local_sdk_version_file,
                        p.local_sdk_version_file,
                    ))
                local_sdk_version_file = p.local_sdk_version_file

                if buildifier and p.buildifier:
                    fail("buildifier set multiple times, got:\n{}\n{}".format(
                        buildifier,
                        p.buildifier,
                    ))
                buildifier = p.buildifier

                if visibility_templates and p.visibility_templates:
                    fail("visibility_templates set multiple times, got:\n{}\n{}".format(
                        visibility_templates,
                        p.visibility_templates,
                    ))
                visibility_templates = p.visibility_templates

    fuchsia_sdk_repository(
        name = "fuchsia_sdk",
        local_paths = local_paths,
        local_sdk_version_file = local_sdk_version_file,
        buildifier = buildifier,
        visibility_templates = visibility_templates,
    )

    fuchsia_toolchain_decl(
        name = "fuchsia_sdk_toolchain_decl",
        toolchain_path = "@fuchsia_sdk//:fuchsia_toolchain_info",
    )

# A tag used to specify a local Fuchsia SDK repository.
_local_tag = tag_class(
    attrs = {
        "path": attr.string(
            doc = "Path to local SDK directory, relative to the workspace root",
            mandatory = True,
        ),
        "local_sdk_version_file": attr.label(
            doc = "An optional file used to mark the version of the SDK pointed to by local_paths.",
            allow_single_file = True,
        ),
        "buildifier": attr.label(
            doc = "An optional label to the buildifier tool, used to reformat all generated Bazel files.",
            allow_single_file = True,
        ),
        "visibility_templates": attr.string_list_dict(
            doc = "Allows for the addition of additional visibility lists. See details in fuchsia_sdk_repository.",
        ),
    },
)

fuchsia_sdk_ext = module_extension(
    implementation = _fuchsia_sdk_repository_ext,
    tag_classes = {"local": _local_tag},
)
