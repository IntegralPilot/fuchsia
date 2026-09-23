"""Implementation of bind_library rule"""

# A Bind library.
#
# Parameters
#
#   srcs
#     List of source files.
#
#   deps
#     List of other bind_library targets included by the library.

load(":providers.bzl", "FuchsiaBindLibraryInfo")

def _get_transitive_srcs(srcs, deps):
    return depset(
        srcs,
        transitive = [dep[FuchsiaBindLibraryInfo].transitive_sources for dep in deps],
    )

def _bind_library_impl(context):
    # In-tree bind libraries have exactly 1 source file.
    if len(context.files.srcs) != 1:
        fail("There must be exactly 1 item in 'srcs'.")

    trans_srcs = _get_transitive_srcs(context.files.srcs, context.attr.deps)
    return [
        FuchsiaBindLibraryInfo(name = context.attr.name, transitive_sources = trans_srcs),
        DefaultInfo(files = depset(context.files.srcs)),
    ]

fuchsia_bind_library = rule(
    implementation = _bind_library_impl,
    attrs = {
        "srcs": attr.label_list(
            doc = "The list of bind library source files",
            mandatory = True,
            allow_files = True,
            allow_empty = False,
        ),
        "deps": attr.label_list(
            doc = "The list of bind libraries this library depends on",
            mandatory = False,
            providers = [FuchsiaBindLibraryInfo],
        ),
    },
)
