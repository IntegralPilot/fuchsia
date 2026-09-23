# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common utilities needed by @rules_fuchsia rules."""

def normalize_os(ctx):
    # On osx os.name => "mac os x".
    return ctx.os.name.split(" ")[0]

def normalize_arch(ctx):
    _MAP = {
        "adm64": "x86_64",
        "k8": "x86_64",
        "aarch64": "arm64",
    }
    return _MAP.get(ctx.os.arch, ctx.os.arch)

def symlink_or_copy(ctx, copy_content_strategy, files_to_copy):
    """ Symlink, hardlink or copy files from one location to another, replicating the relative directory structure

    Args:
        ctx: repository context
        copy_content_strategy: "copy" or "symlink"
        files_to_copy: a dict of string to list, where the key is a string with the root path and the value is a list of string paths relative to the root

    """
    unique_files_to_copy = dict([((root, file), None) for root, files in files_to_copy.items() for file in files]).keys()

    if copy_content_strategy == "copy":
        per_dest_dir = {}
        for root, file in unique_files_to_copy:
            dest = ctx.path(file)
            dest_dir = str(dest.dirname)
            origin_file = str(ctx.path("%s/%s" % (root, file)))
            if dest_dir in per_dest_dir:
                for other in per_dest_dir[dest_dir]:
                    if ctx.path(other).basename == dest.basename:
                        fail("File is specified in two different places: in %s and in %s" % (other, origin_file))
                per_dest_dir[dest_dir].append(origin_file)
            else:
                per_dest_dir[dest_dir] = [origin_file]

        host_os = normalize_os(ctx)
        try_hardlink = host_os == "linux"
        for dest_dir in per_dest_dir.keys():
            ctx.execute(["mkdir", "-p", dest_dir], quiet = False)

            # for performance reasons, if a single hardlink operation fails, we switch to regular copy for
            # all the remaining destination directories
            if try_hardlink:
                result = ctx.execute(["cp", "-l", "--no-clobber"] + per_dest_dir[dest_dir] + [dest_dir], quiet = True)
                if result.return_code != 0:
                    try_hardlink = False

            # this is not an "else" intentionally, since we want to execute regular copy if we couldn't hardlink
            if not try_hardlink:
                # Note: -n is the same as --no-clobber but is supported on MacOS.
                command = ["cp", "-n"] + per_dest_dir[dest_dir] + [dest_dir]
                result = ctx.execute(command, quiet = False)
                if result.return_code != 0:
                    fail("Cannot copy files (%s):\n     %s" % (str(result.return_code), " ".join(command)))

    elif copy_content_strategy == "symlink":
        for root, file in unique_files_to_copy:
            ctx.symlink(ctx.path("%s/%s" % (root, file)), ctx.path(file))
    else:
        fail("Invalid value of copy_content_strategy argument: %s" % copy_content_strategy)

def workspace_path(repo_ctx, local_path):
    """Resolve a local path relative to the main workspace directory.

    Args:
      repo_ctx: A repository_ctx instance.

      local_path: Either a Path object, or a relative or absolute path string.
          If absolute, the path is  returned as is. If relative, it is resolved
          relative to the main workspace's directory.

    Returns:
      An absolute path string.
    """
    local_path = str(local_path)
    if local_path.startswith("/"):
        return local_path

    return "%s/%s" % (repo_ctx.workspace_root, local_path)

def fetch_cipd_contents(ctx, cipd_bin, cipd_ensure_file, root = "."):
    """Fetches the contents of a cipd bucket and places them in the root.

    Args:
      ctx: A repository_ctx instance.

      cipd_bin: Either a Path object, or a relative or absolute path string which
          points to a cipd binary. If absolute, the path is  returned as is. If
          relative, it is resolved relative to the main workspace's directory.

      cipd_ensure_file: Either a Path object, or a relative or absolute path string which
          points to a cipd binary. If absolute, the path is  returned as is. If
          relative, it is resolved relative to the main workspace's directory.

      root: A path to where the contents will be installed.
    """
    result = ctx.execute(
        [
            ctx.path(cipd_bin),
            "ensure",
            "-ensure-file",
            ctx.path(cipd_ensure_file),
            "-root",
            root,
            "-max-threads=0",
        ],
    )
    if result.return_code != 0:
        fail("Unable to download cipd content for {}\n{}".format(cipd_ensure_file, result.stderr))

def is_full_bazel_label(s):
    """Checks if a string is a full Bazel label.

    For example:

        * @@myrepo//my/app/main:app_binary
        * @myrepo//my/app/main:app_binary
        * //my/app/main:app_binary

    Args:
      s: A string to check.

    Returns:
      True if the string is a full Bazel label, False otherwise.
    """
    return s.startswith("@") or s.startswith("//")

def is_full_gn_label(s):
    """Checks if a string is a GN path.

    Args:
      s: A string to check.

    Returns:
      True if the string is a full GN label, False otherwise.
    """
    return s.startswith("//")

def abspath_from_full_label_or_repo_relpath(repo_ctx, label_or_relpath):
    """Returns absolute path for input full label or repository relative path.

    Args:
      repo_ctx: A Bazel repository context
      label_or_relpath: A string, it can be either a full Bazel label, or a path
        relative to repository root.

    Returns:
      Absolute path based on input label_or_relpath.
    """
    p = label_or_relpath
    if is_full_bazel_label(label_or_relpath):
        p = str(repo_ctx.path(Label(label_or_relpath)))
    return workspace_path(repo_ctx, p)
