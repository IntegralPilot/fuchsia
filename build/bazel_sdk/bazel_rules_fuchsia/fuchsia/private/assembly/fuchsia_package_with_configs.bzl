# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for tying a package to its configs for assembly."""

load("@fuchsia_rules_common//assembly:providers.bzl", "FuchsiaAssembledPackageInfo")
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load(":providers.bzl", "FuchsiaConfigDataInfo")

def _fuchsia_package_with_configs_impl(ctx):
    configs = []
    files = []
    for config_file in ctx.attr.configs:
        f = config_file.files.to_list()[0]
        configs.append(FuchsiaConfigDataInfo(
            source = f,
            destination = ctx.attr.configs[config_file],
        ))
        files.append(f)
    package = ctx.attr.package[FuchsiaPackageInfo]
    files.extend(ctx.files.package)

    return [
        DefaultInfo(files = depset(files)),
        FuchsiaAssembledPackageInfo(
            package = package,
            configs = configs,
            files = files,
            build_id_dirs = package.build_id_dirs,
        ),
    ]

fuchsia_package_with_configs = rule(
    doc = """Declares a target to attach configs to package for assembly.""",
    implementation = _fuchsia_package_with_configs_impl,
    provides = [FuchsiaAssembledPackageInfo],
    attrs = {
        "package": attr.label(
            doc = "The package to attach configs to.",
            providers = [FuchsiaPackageInfo],
            mandatory = True,
        ),
        "configs": attr.label_keyed_string_dict(
            doc = "Config-datas that are attached to the package. It's a dictionary of source files to destination string.",
            allow_files = True,
        ),
    },
)
