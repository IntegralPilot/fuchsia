# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A wrapper around cc_binary to be used for drivers targeting Fuchsia."""

load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("@rules_cc//cc:cc_shared_library.bzl", "cc_shared_library")
load("//fuchsia/private:fuchsia_cc.bzl", "fuchsia_cc")

def fuchsia_cc_driver(
        name,
        srcs = [],
        output_name = None,
        deps = [],
        **kwargs):
    """Creates a binary driver which targets Fuchsia.

    Wraps a cc_shared_library rule and provides appropriate defaults.

    This method currently just simply ensures that libc++ is statically linked.
    In the future it will ensure drivers are correctly versioned and carry the
    appropriate package resources.

    If srcs are included the macro will create a cc_library and put all of the
    deps into that library. If srcs are not provided then the deps will be added
    directly to the cc_shared_library.

    Args:
        name: the target name
        srcs: (optional) the sources to include in the driver. If not provided,
          the user should provide a cc_library in the deps.
        output_name: (optional) the name of the .so to build. If excluded,
          will default to lib<name>.so
        deps: The deps for the driver
        **kwargs: The arguments to forward to cc_library
    """

    # Remove this value because we want to set it on our own. If we don't
    # remove it the fuchsia_wrap_cc_binary to fail with an unknown attribute.
    kwargs.pop("linkshared", None)

    # Compute the label of the private linker script.
    #
    # NOTE: A value of //fuchsia/private:driver.ld will not work here, as it will
    # be interpreted as a package 'fucshia' or the project's workspace itself.
    driver_ld_target = "@rules_fuchsia//fuchsia/private:driver.ld"

    shared_lib_name = (output_name or "lib{}".format(name)).removesuffix(".so") + ".so"

    # Ensure we are packaging the lib/libdriver_runtime.so
    # NOTE: @rules_fuchsia should not depend on @fuchsia_sdk directly. Instead a
    # toolchain should be used to add one layer of indirection.
    deps.append(
        "@fuchsia_sdk//pkg/driver_runtime_shared_lib",
    )

    user_link_flags = [
        # We need to run our own linker script to limit the symbols that are exported
        # and to make the driver framework symbols global.
        "-Wl,--version-script",
        "$(location %s)" % driver_ld_target,
    ]

    # maintain backwards compatibility with cc_binary
    user_link_flags.extend(kwargs.pop("linkopts", []))

    # collect any flags that the user passed in
    user_link_flags.extend(kwargs.pop("user_link_flags", []))

    # If a user includes srcs then we need to create a cc_library and put all of
    # the deps in that target. Otherwise, the user has provided their own
    # cc_library as a dep in which case we can
    if len(srcs) > 0:
        cc_library(
            name = name + "_srcs",
            srcs = srcs,
            deps = deps,
            # pull out the kwargs
            defines = kwargs.pop("defines", None),
            **kwargs
        )
        deps.append(":" + name + "_srcs")

        # only include the cc_library in the deps of the cc_shared_library. If
        # we don't do this then the linker will error out with a duplicate
        # symbols error.
        shared_library_deps = [":" + name + "_srcs"]
    else:
        shared_library_deps = deps

    features = kwargs.pop("features", []) + select({
        "@rules_fuchsia//fuchsia/select:dynamic_cpp_standard_library_for_drivers_enabled": [],
        "//conditions:default": ["static_cpp_standard_library"],
    })

    cc_shared_library_name = name + "_cc_shared_library"
    cc_shared_library(
        name = cc_shared_library_name,
        deps = shared_library_deps,
        additional_linker_inputs = [driver_ld_target],
        user_link_flags = user_link_flags,
        shared_lib_name = shared_lib_name,
        features = features,
        visibility = ["//visibility:private"],
        **kwargs
    )

    # To forward to fuchsia_wrap_cc_binary. If we don't do this here we end up
    # with duplicate entries in cc_binary which will cause a failure.
    visibility = kwargs.pop("visibility", None)
    tags = kwargs.pop("tags", None)
    testonly = kwargs.pop("testonly", None)

    driver_data = [
        "@fuchsia_sdk//pkg/sysroot:dist",
        "@fuchsia_sdk//clang:sanitizer_resources",
    ] + select({
        "@rules_fuchsia//fuchsia/select:dynamic_cpp_standard_library_for_drivers_enabled": ["@fuchsia_sdk//clang:libcxx_resources"],
        "//conditions:default": [],
    })

    fuchsia_cc(
        name = name,
        bin_name = shared_lib_name,
        install_root = "driver/",
        native_target = cc_shared_library_name,
        data = driver_data,
        deps = deps,
        visibility = visibility,
        testonly = testonly,
        features = features,
        tags = tags,
        # TODO(352586714) Enable this check when we understand why the symbols are getting
        # pulled in.
        # restricted_symbols = "@rules_fuchsia//fuchsia/private:driver_restricted_symbols.txt",
    )
