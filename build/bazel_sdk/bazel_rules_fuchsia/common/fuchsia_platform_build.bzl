# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Misc utilities that are related to the Fuchsia platform build only."""

load(
    "//common:repository_utils.bzl",
    "get_fuchsia_host_arch",
    "get_fuchsia_host_os",
)
load(
    "//common:toolchains/clang/clang_utils.bzl",
    "to_clang_target_tuple",
)

def _path_relativize(path_from, path_to):
    """Give a relative path from absolute input paths.

    Needed because bazel_skylib's relativize() doesn't work
    when path_from is not a parent of path_to.

    Args:
        path_from: Start path, must be absolute.
        path_to: Destination path, must be absolute.
    Return:
        Relative path from path_from to path_to. This
        will be "." if they are the same path.
    """
    if not path_from.startswith("/"):
        fail("Path is not absolute: %s" % path_from)
    if not path_to.startswith("/"):
        fail("Path is not absolute: %s" % path_to)

    from_segments = path_from.split("/")[1:]
    to_segments = path_to.split("/")[1:]

    # Compute common prefix length.
    from_len = len(from_segments)
    to_len = len(to_segments)
    common_len = min(from_len, to_len)
    for n in range(common_len):
        if from_segments[n] != to_segments[n]:
            common_len = n
            break

    segments = [".."] * (from_len - common_len)
    segments += to_segments[common_len:]
    if segments:
        return "/".join(segments)
    else:
        return "."  # They are the same path.

def _get_fuchsia_source_dir_relative_to_workspace(repo_ctx):
    fuchsia_source_dir = repo_ctx.attr.fuchsia_source_dir
    if fuchsia_source_dir.startswith("/"):
        # Fuchsia source dir is absolute. Compute its relative path.
        # from the workspace root.
        return _path_relativize(str(repo_ctx.workspace_root), fuchsia_source_dir)
    elif fuchsia_source_dir:
        # Fuchsia source dir is already relative to the workspace root.
        return fuchsia_source_dir
    else:
        # Fuchsia source dir is empty (in-tree Fuchsia build). Since the workspace
        # file contains symlinks to the same file under the Fuchsia source directory,
        # resolve the real path of $WORKSPACE/BUILD.bazel then take its parent as the
        # result, then relativize that.
        fuchsia_source_dir = str((repo_ctx.workspace_root.get_child("BUILD.bazel")).realpath.dirname)
        return _path_relativize(str(repo_ctx.workspace_root), fuchsia_source_dir)

def _get_fuchsia_source_path(repo_ctx, relative_path):
    """Return the path to a file, given its relative path from the Fuchsia source directory.

    Args:
       repo_ctx: repository context
       relative_path: A path, relative to the fuchsia source directory.

    Returns:
       A new absolute repo_ctx.path() object pointing to the file.
    """
    fuchsia_source_dir = repo_ctx.attr.fuchsia_source_dir
    if fuchsia_source_dir.startswith("/"):
        # Fuchsia source dir is absolute. This can happen when defining
        # it through the LOCAL_FUCHSIA_PLATFORM_BUILD environment variable.
        result = "%s/%s" % (fuchsia_source_dir, relative_path)
    elif fuchsia_source_dir:
        # Fuchsia source dir is relative to the workspace root (e.g. used by Bazel SDK test).
        result = "%s/%s/%s" % (repo_ctx.workspace_root, fuchsia_source_dir, relative_path)
    else:
        # Fuchsia source dir is empty (in-tree Fuchsia build), just use
        # a path relative to the Bazel workspace directory, as it mimics
        # the Fuchsia source directory anyway.
        result = "%s/%s" % (repo_ctx.workspace_root, relative_path)

    result = str(repo_ctx.path(result).realpath)
    return result

def _get_ninja_output_dir(repo_ctx):
    """Compute the Ninja output directory used by this workspace.

    Args:
        repo_ctx: the repository context.
    Returns:
        a string containing the path to the ninja output directory,
        relative to the current workspace root.
    """

    if repo_ctx.attr.fuchsia_source_dir:
        # If repo_ctx.attr.fuchsia_dir is not empty, this repository
        # rule is not used in the Fuchsia in-tree build (e.g. running the
        # Bazel SDK test suite), and the Ninja output directory should not
        # be a concern.
        return "COULD_NOT_FIND_NINJA_OUTPUT_DIRECTORY"

    # LINT.IfChange(bazel_topdir_config_file)
    # The config file at this location contains the location of the TOPDIR
    # used by //build/regenerator.py, e.g. `gen/build/bazel`. This code assumes
    # that the workspace is one of its sub-directories, so compute a
    # back-tracking path prefix by counting the number of path fragments
    # in it.
    config_path = repo_ctx.workspace_root.get_child("build/bazel/config/bazel_top_dir")

    # LINT.ThenChange(//build/bazel/bazel_workspace.gni:bazel_topdir_config_file)
    top_dir = repo_ctx.read(config_path).strip()
    num_fragments = len(top_dir.split("/"))
    result = ".."
    for _ in range(num_fragments):
        result += "/.."

    ninja_output_dir_path = repo_ctx.path(result).realpath
    if not ninja_output_dir_path.exists:
        fail("Could not find Ninja build directory: %s" % ninja_output_dir_path)

    return result

def _cfg_values_to_dict(values_text):
    """Parse comma-separated key=value pairs into a dictionary."""
    values = {}
    for var in values_text.split(","):
        k, _, v = var.partition("=")
        values[k] = v
    return values

def _get_rbe_config(repo_ctx):
    """Compute RBE-related configuration.

    Args:
      repo_ctx: Repository context.
    Returns:
      A struct with fields describing RBE-related config information.
    """
    rewrapper_config_path = _get_fuchsia_source_path(
        repo_ctx,
        "build/rbe/fuchsia-rewrapper.cfg",
    )

    reproxy_config_path = _get_fuchsia_source_path(
        repo_ctx,
        "build/rbe/fuchsia-reproxy.cfg",
    )

    instance_prefix = "instance="
    platform_prefix = "platform="

    platform_values = {}
    for line in repo_ctx.read(rewrapper_config_path).splitlines():
        line = line.strip()
        if line.startswith(platform_prefix):
            # After "platform=", expect comma-separated key=value pairs.
            platform_values = _cfg_values_to_dict(line.removeprefix(platform_prefix))

    container_image = platform_values.get("container-image")
    pool_name = platform_values.get("Pool")

    instance_name = None
    for line in repo_ctx.read(reproxy_config_path).splitlines():
        line = line.strip()
        if line.startswith(instance_prefix):
            instance_name = line[len(instance_prefix):]

    if not instance_name:
        fail("ERROR: Missing instance name from %s" % reproxy_config_path)

    if not container_image:
        fail("ERROR: Missing container image name from %s" % rewrapper_config_path)

    return struct(
        instance_name = instance_name,
        container_image = container_image,
        pool_name = pool_name,
    )

def _get_formatted_starlark_dict(dict_value, margin):
    """Convert dictionary into formatted Starlak expression.

    Args:
        dict_value: (dict) dictionary value.
        margin: (string) A string of spaces to use as the current left-margin
           for the location where the value will be inserted.
    Returns:
        A string holding a formatted Starlark expression for the input
        dictionary. If not empty, this will use multiple lines (one per key)
        with indentation of |len(margin) + 4| spaces.
    """
    return json.encode_indent(dict_value, prefix = margin, indent = margin + "   ")

def _fuchsia_build_config_repository_impl(repo_ctx):
    host_os = get_fuchsia_host_os(repo_ctx)
    host_arch = get_fuchsia_host_arch(repo_ctx)
    host_target_triple = to_clang_target_tuple(host_os, host_arch)
    host_tag = "%s-%s" % (host_os, host_arch)
    host_tag_alt = "%s_%s" % (host_os, host_arch)

    host_os_constraint = "@platforms//os:" + host_os

    host_cpu_constraint = "@platforms//cpu:" + {
        "x64": "x86_64",
        "arm64": "aarch64",
    }.get(host_arch, host_arch)

    rbe_config = _get_rbe_config(repo_ctx)

    ninja_output_dir = _get_ninja_output_dir(repo_ctx)

    fuchsia_source_dir = _get_fuchsia_source_dir_relative_to_workspace(repo_ctx)

    defs_content = '''# Auto-generated DO NOT EDIT

"""Global information about this build's configuration."""

build_config = struct(
    # The host operating system, using Fuchsia conventions
    host_os = "{host_os}",

    # The host CPU architecture, using Fuchsia conventions.
    host_arch = "{host_arch}",

    # The host tag, used to separate prebuilts in the Fuchsia source tree
    # (e.g. 'linux-x64')
    host_tag = "{host_tag}",

    # The host tag, using underscores instead of dashes.
    host_tag_alt = "{host_tag_alt}",

    # The GCC/Clang target triple for the host operating system.
    # (e.g. x86_64-unknown-gnu-linux or aarcg64-apple-darwin).
    host_target_triple = "{host_target_triple}",

    # The Bazel platform os constraint value for the host
    # (e.g. '@platforms//os:linux')
    host_platform_os_constraint = "{host_os_constraint}",

    # The Bazel platform cpu constraint value for the host
    # (e.g. '@platforms//cpu:x86_64' or '@platforms//cpu:aarch64')
    host_platform_cpu_constraint = "{host_cpu_constraint}",

    # The RBE instance name for remote build configuration. Empty if disabled.
    rbe_instance_name = "{rbe_instance_name}",

    # The RBE container image for remote build configuration. Empty if disabled.
    rbe_container_image = "{rbe_container_image}",

    # The RBE default RBE pool for remote build configuration, e.g. "default".
    rbe_pool_name = "{rbe_pool_name}",

    # The path to the Ninja output directory, relative to the current
    # workspace root.
    ninja_output_dir = "{ninja_output_dir}",

    # The path to the Fuchsia source directory, relative to the current
    # workspace root.
    fuchsia_source_dir = "{fuchsia_source_dir}",
)
'''.format(
        host_os = host_os,
        host_arch = host_arch,
        host_target_triple = host_target_triple,
        host_tag = host_tag,
        host_tag_alt = host_tag_alt,
        host_os_constraint = host_os_constraint,
        host_cpu_constraint = host_cpu_constraint,
        rbe_instance_name = rbe_config.instance_name,
        rbe_container_image = rbe_config.container_image,
        rbe_pool_name = rbe_config.pool_name,
        ninja_output_dir = ninja_output_dir,
        fuchsia_source_dir = fuchsia_source_dir,
    )

    repo_ctx.file("WORKSPACE.bazel", "")
    repo_ctx.file("defs.bzl", defs_content)

    # Generate a BUILD.bazel file that contains a platform definition using the
    # right rbe_container_image execution property for linux-x64.
    if host_tag == "linux-x64":
        exec_properties = {
            "container-image": rbe_config.container_image,
            "OSFamily": "Linux",
            "Pool": rbe_config.pool_name,
        }
    else:
        exec_properties = {}

    # Create symlinks used to locate host prebuilts without an explicit
    # fuchsia_host_tag in their path, making the top-level MODULE.bazel easier
    # to write.
    prebuilt_host_subdirs = ["go", "llvm", "python3"]
    prebuilt_host_tools = ["ninja", "gn", "buildifier"]
    prebuilt_bin_host_tools = ["jq"]  # Tools that live in an additional "bin" subdirectory.

    # LINT.IfChange
    gn_args = json.decode(
        repo_ctx.read("{}/fuchsia_build_generated/args.json".format(repo_ctx.workspace_root)),
    )

    # LINT.ThenChange(//build/bazel/scripts/workspace_utils.py)

    # In case users set a custom clang prefix in GN, respect that config.
    if "clang_prefix" in gn_args:
        clang_prefix = gn_args["clang_prefix"]

        # GN clang_prefix can be GN labels, which are relative to workspace root.
        if clang_prefix.startswith("//"):
            clang_prefix = "{}/{}".format(repo_ctx.workspace_root, clang_prefix[2:])

        # GN clang_prefix points to the `bin` directory under clang dir.
        repo_ctx.symlink(repo_ctx.path(clang_prefix).dirname, "host_prebuilts/clang")
    else:
        prebuilt_host_subdirs.append("clang")

    # In case users set a custom rustc prefix in GN, respect that config.
    if "rustc_prefix" in gn_args:
        rustc_prefix = gn_args["rustc_prefix"]

        # GN rustc_prefix can be GN labels, which are relative to workspace root.
        if rustc_prefix.startswith("//"):
            rustc_prefix = "{}/{}".format(repo_ctx.workspace_root, rustc_prefix[2:])

        # GN rustc_prefix points to the root directory under rust dir.
        repo_ctx.symlink(repo_ctx.path(rustc_prefix), "host_prebuilts/rust")
    else:
        prebuilt_host_subdirs.append("rust")

    for subdir in prebuilt_host_subdirs:
        repo_ctx.symlink("{}/prebuilt/third_party/{}/{}".format(repo_ctx.workspace_root, subdir, host_tag), "host_prebuilts/{}".format(subdir))
    for tool in prebuilt_host_tools:
        repo_ctx.symlink("{}/prebuilt/third_party/{}/{}/{}".format(repo_ctx.workspace_root, tool, host_tag, tool), "host_prebuilts/{}".format(tool))
    for tool in prebuilt_bin_host_tools:
        repo_ctx.symlink("{}/prebuilt/third_party/{}/{}/bin/{}".format(repo_ctx.workspace_root, tool, host_tag, tool), "host_prebuilts/{}".format(tool))

    exported_files_list = ["host_prebuilts/{}".format(tool) for tool in prebuilt_host_tools + prebuilt_bin_host_tools]

    # Create a symlink to the Fuchsia prebuilt python3 binary (unlike host_prebuilts/python3
    # which points to a directory). This avoids the heavy machinery associated with py_binary()
    # such as toolchain resolution and middle-man scripts, but is only usable from
    # repository rules, or target rules that run locally without a sandbox.
    repo_ctx.symlink("{}/prebuilt/third_party/python3/{}/bin/python3".format(repo_ctx.workspace_root, host_tag), "host_prebuilts/python3_bin_no_sandbox")
    exported_files_list.append("host_prebuilts/python3_bin_no_sandbox")

    # TODO: invoke `create_rbe_exec_properties_dict` to validate keys
    # https://github.com/bazelbuild/bazel-toolchains/blob/master/rules/exec_properties/README.md
    build_bazel_content = '''# Auto-generated DO NOT EDIT

"""A host platform() with optional support for remote builds."""

exports_files(
    {exported_files}
)

platform(
    name = "host",
    constraint_values = [
        "{host_os_constraint}",
        "{host_cpu_constraint}",
    ],
    exec_properties = {exec_properties_str},
)
'''.format(
        exported_files = repr(exported_files_list),
        host_os_constraint = host_os_constraint,
        host_cpu_constraint = host_cpu_constraint,
        exec_properties_str = _get_formatted_starlark_dict(exec_properties, "    "),
    )
    repo_ctx.file("BUILD.bazel", build_bazel_content)

fuchsia_build_config_repository = repository_rule(
    implementation = _fuchsia_build_config_repository_impl,
    doc = "A repository rule used to create an external repository that " +
          "contains a defs.bzl file exposing information about the current " +
          "build configuration for the Fuchsia platform build. Only the Fuchsia " +
          "in-tree build, and the Bazel SDK test suite should use this repository rule.",
    attrs = {
        "fuchsia_source_dir": attr.string(
            mandatory = False,
            doc = "Path to the Fuchsia source directory, relative to the " +
                  "current workspace. Must be empty for the in-tree Fuchsia workspace",
        ),
    },
)

### BzlMod support.

def _fuchsia_build_config_ext_impl(module_ctx):
    # See //build/bazel_sdk/tests/README.md: LOCAL_FUCHSIA_PLATFORM_BUILD
    # can be defined to point to the in-tree Ninja build directory when
    # running the Bazel SDK test suite.
    fuchsia_source_dir = module_ctx.os.environ.get("LOCAL_FUCHSIA_PLATFORM_BUILD", "")
    if fuchsia_source_dir:
        # Assume the build directory is under {FUCHSIA_SOURCE_DIR}/out/<some_name>
        fuchsia_source_dir += "/../.."

    for mod in module_ctx.modules:
        if not mod.is_root:
            continue
        for d in mod.tags.local:
            if d.fuchsia_source_dir and not fuchsia_source_dir:
                # The local() tag is used by //build/bazel_sdk/tests/MODULE.bazel
                # to point to the Fuchsia source directory.
                fuchsia_source_dir = d.fuchsia_source_dir

    fuchsia_build_config_repository(
        name = "fuchsia_build_config",
        fuchsia_source_dir = fuchsia_source_dir,
    )

_local_tag = tag_class(
    attrs = {
        "fuchsia_source_dir": attr.string(
            mandatory = True,
            doc = "Path to the Fuchsia source directory, relative to the " +
                  "current workspace.",
        ),
    },
)

fuchsia_build_config_ext = module_extension(
    implementation = _fuchsia_build_config_ext_impl,
    tag_classes = {"local": _local_tag},
)
