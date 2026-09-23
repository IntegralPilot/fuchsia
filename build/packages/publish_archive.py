#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import shutil
import subprocess
import tempfile
import zipfile


def zip_dir(dir, zip_file):
    for root, _dirs, files in os.walk(dir):
        for file in files:
            path = os.path.join(root, file)
            zip_file.write(path, os.path.relpath(path, dir))


# rmtree manually removes all subdirectories and files instead of using
# shutil.rmtree, to avoid registering spurious reads on stale
# subdirectories. See https://fxbug.dev/42153728.
def rmtree(dir):
    if not os.path.exists(dir):
        return
    for root, dirs, files in os.walk(dir, topdown=False):
        for file in files:
            os.unlink(os.path.join(root, file))
        for dir in dirs:
            full_path = os.path.join(root, dir)
            if os.path.islink(full_path):
                os.unlink(full_path)
            else:
                os.rmdir(full_path)


def prepare_dirs(repo_dir):
    path = os.path.join(repo_dir, "repository")
    os.makedirs(path)

    return {"root": repo_dir, "repository": path}


# `package-tool repository publish` expects the following inputs in its in/out directory:
# - `keys/{snapshot|targets|timestamp}.json` containing private metadata keys;
# - `repository/{{version-num}}.root.json` containing versioned root metadata;
# - `repository/root.json` containing default root metadata.
def prepare_publish(args, dirs):
    for root_metadata_path in args.root_metadata:
        shutil.copy(root_metadata_path, dirs["repository"])
    shutil.copy(
        args.default_root_metadata,
        "{}/{}".format(dirs["repository"], "root.json"),
    )


def package_tool_publish(args, dirs, depfile):
    cmd_args = [
        args.package_tool,
        "repository",
        "publish",
        "--trusted-keys",
        args.trusted_keys,
        "--trusted-root",
        "{}/{}".format(dirs["repository"], "root.json"),
        "--package-list",
        args.input,
        "--depfile",
        args.depfile,
    ]

    if args.delivery_blob_type:
        cmd_args.extend(["--delivery-blob-type", args.delivery_blob_type])

    cmd_args.append(dirs["root"])

    subprocess.run(cmd_args, check=True)


def main(args):
    with tempfile.TemporaryDirectory(
        dir=os.path.dirname(args.output)
    ) as gendir:
        dirs = prepare_dirs(gendir)

        # Prepare for `package-tool repository publish` and gather deps associated with preparations.
        prepare_publish(args, dirs)

        depfile = os.path.join(gendir, "deps")

        # Invoke `package-tool repository publish` and gather deps associated with invocation.
        package_tool_publish(args, dirs, depfile)

        # Output repository directory to zip file.
        with zipfile.ZipFile(
            args.output, "w", zipfile.ZIP_DEFLATED
        ) as zip_file:
            zip_dir(dirs["repository"], zip_file)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        "Creates a zip archive of the TUF repository output by `package-tool repository publish`"
    )
    parser.add_argument(
        "--package-tool",
        help="path to the package-tool executable",
        required=True,
    )
    parser.add_argument(
        "--trusted-keys",
        help="path to a keys directory to be consumed by `package-tool repository publish`",
    )
    parser.add_argument(
        "--root-metadata",
        help="path to a root metadata file to be used in the TUF repository",
        action="append",
    )
    parser.add_argument(
        "--default-root-metadata",
        help="path to the default TUF root metadata file",
        required=True,
    )
    parser.add_argument(
        "--input",
        help="path to `package-tool repository publish` file input",
        required=True,
    )
    parser.add_argument(
        "--delivery-blob-type", help="the type of delivery blob to generate"
    )
    parser.add_argument("--depfile", help="generate a depfile", required=True)
    parser.add_argument("--output", help="path output zip file", required=True)
    main(parser.parse_args())
