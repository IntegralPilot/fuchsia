# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Creates a fuchsia_package_resource for devicetree visitor."""

load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("@rules_cc//cc:cc_shared_library.bzl", "cc_shared_library")
load(":fuchsia_cc.bzl", "fuchsia_cc")

def fuchsia_devicetree_visitor(name, output_name = None, additional_linker_inputs = [], user_link_flags = [], **kwargs):
    """Creates a fuchsia_package_resource for devicetree visitor.

    Args:
        name: the target name
        output_name: (optional) the name of the .so to be included into the package. If excluded will default to <name>.so
        user_link_flags: this will be propagaed to cc_shared_library for any additional flags that you may want to pass to the linker
        additional_linker_inputs: this will be propagaed to cc_shared_library for any additional files that you may want to pass to the linker, for example, linker scripts
        **kwargs: The arguments to forward to cc_library
    """
    lib_name = "{}_lib".format(name)
    visibility = kwargs.pop("visibility", None)

    cc_library(
        name = lib_name,
        visibility = visibility,
        **kwargs
    )
    additional_linker_inputs = kwargs.pop("additional_linker_inputs", [])
    user_link_flags = kwargs.pop("user_link_flags", [])

    static_kwargs = dict(kwargs)
    static_defines = static_kwargs.get("defines", [])
    if "FDF_DEVICETREE_STATIC_VISITOR" not in static_defines:
        static_defines.append("FDF_DEVICETREE_STATIC_VISITOR")
    static_kwargs["defines"] = static_defines

    cc_library(
        name = "{}_static".format(name),
        visibility = visibility,
        **static_kwargs
    )

    # Compute the label of the linker script.
    #
    # NOTE: A value of //fuchsia/private:driver.ld will not work here, as it will
    # be interpreted as a package 'fucshia' or the project's workspace itself.
    visitor_ld_target = "@rules_fuchsia//fuchsia/private:visitor.ld"

    user_link_flags.extend([
        "-Wl,--version-script",
        "$(location %s)" % visitor_ld_target,
    ])
    additional_linker_inputs.append(visitor_ld_target)

    shared_lib_name = "{}_shared_lib".format(name)
    cc_shared_library(
        name = shared_lib_name,
        deps = [
            ":" + lib_name,
        ],
        additional_linker_inputs = additional_linker_inputs,
        user_link_flags = user_link_flags,
    )

    resource_name = name
    bin_name = "{}.so".format(output_name or name)
    fuchsia_cc(
        name = resource_name,
        bin_name = bin_name,
        install_root = "lib/visitors/",
        native_target = shared_lib_name,
        visibility = visibility,
    )
