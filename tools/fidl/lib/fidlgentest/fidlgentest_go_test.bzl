"""A macro for running go tests that use the fidlgentest library."""

load("//build/bazel/rules/host_tests:host_go_test.bzl", "host_go_test")
load("//build/bazel/rules/host_tests:host_test_data.bzl", "host_test_data_files")

def fidlgentest_go_test(
        name,
        embed,
        target_compatible_with,
        deps = [],
        data = [],
        test_args = [],
        visibility = None,
        **kwargs):
    """Declares a host Go test that uses the fidlgentest library.

    The fidlgentest library runs fidlc at runtime, so tests using it must make
    fidlc available and pass its path as an argument to the test binary. This
    macro takes care of that.

    Args: Same as host_go_test().
    """
    _fidlc_target = "//tools/fidl/fidlc"
    test_data_name = name + "_fidlc_data"

    host_test_data_files(
        name = test_data_name,
        srcs = [_fidlc_target],
    )

    if embed != [":fidlgentest"]:
        deps = deps[:]
        deps.append("//tools/fidl/lib/fidlgentest")

    test_args = test_args[:]
    test_args.extend([
        "--fidlc",
        "./fidlc",
    ])

    test_data = kwargs.pop("test_data", []) + [":" + test_data_name]

    host_go_test(
        name = name,
        embed = embed,
        test_args = test_args,
        test_data = test_data,
        deps = deps,
        target_compatible_with = target_compatible_with,
        visibility = visibility,
        **kwargs
    )
