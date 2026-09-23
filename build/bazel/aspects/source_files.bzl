# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Aspects related to collecting source input paths for the Fuchsia build system."""

load("//build/bazel/aspects:utils.bzl", "get_target_deps_from_attributes")

FuchsiaSourcesInfo = provider(
    doc = "A provider for a set of source files",
    fields = {
        "sources": "a depset[File] to input source files.",
    },
)

def _should_exclude_file_path(path):
    """Return True if a given file path should be excluded from the source files list."""

    # Artifacts have a file path starting with "bazel-out/" so filter them out.
    if path.startswith("bazel-out/"):
        return True

    # The aspect collects license() target inputs. By default these targets use
    # a `license_text = "LICENSE"` attribute, but a few third-party Rust crates have
    # different license file names (e.g. `LICENCE` instead of `LICENSE`).
    # We should ignore these paths to avoid creating Ninja no-op failures.
    if "third_party/rust_crates/vendor" in path and path.endswith("/LICENSE"):
        return True

    return False

# First, an aspect to collect source file information.
# This simply gets all file inputs for each target and its dependencies,
# whether they are artifacts or source files must be sorted out by the caller.
def _collect_source_files_aspect_impl(target, aspect_ctx):
    direct_files = []

    # aspect_ctx.rule.files is a struct that whose keys match all the label attributes
    # of the rule that point directly to input files. Use dir() to walk over all of them.
    for field in dir(aspect_ctx.rule.files):
        files = getattr(aspect_ctx.rule.files, field, [])

        # Artifacts have a file path starting with "bazel-out/" so filter them out.
        direct_files.extend([file for file in files if not _should_exclude_file_path(file.path)])

    transitive_depsets = []

    # Propagate through common dependency attributes
    for dep in get_target_deps_from_attributes(aspect_ctx.rule.attr):
        if FuchsiaSourcesInfo in dep:
            transitive_depsets.append(dep[FuchsiaSourcesInfo].sources)

    return [FuchsiaSourcesInfo(
        sources = depset(direct_files, transitive = transitive_depsets),
    )]

collect_source_files_aspect = aspect(
    implementation = _collect_source_files_aspect_impl,
    attr_aspects = ["*"],
    provides = [FuchsiaSourcesInfo],
)

# Second, a non-propagating aspect that requires the first one and will use
# the result to generate a text file containing all paths collected by the first one.
def _generate_source_files_list_impl(target, actx):
    if FuchsiaSourcesInfo not in target:
        fail("No FuchsiaSourcesInfo in %s" % target.label)

    sources = sorted([source.path for source in target[FuchsiaSourcesInfo].sources.to_list()])
    output = actx.actions.declare_file("%s.fuchsia_source_files.json" % target.label.name)

    # LINT.IfChange(source_files_list_schema)
    content_json = {
        "label": str(target.label),
        "sources": sources,
    }
    # LINT.ThenChange(//build/bazel/scripts/bazel_action_impl.py:source_files_list_schema)

    actx.actions.write(output, json.encode_indent(content_json, indent = "  "))

    # There is no way to get the path of the output file using cquery, because
    # that command ignores aspect-generated providers.
    # See https://github.com/bazelbuild/bazel/issues/22528
    #
    # To work around this, use print() here to print the execroot-related path
    # to stderr, and ensure the caller can process this line to extract the file's
    # location.
    # LINT.IfChange(source_files_list_path_prefix)
    print("FUCHSIA_SOURCES_MANIFEST_PATH=%s" % output.path)
    # LINT.ThenChange(//build/bazel/scripts/bazel_action_impl.py:source_files_list_path_prefix)

    return [
        OutputGroupInfo(
            fuchsia_sources_manifest = depset([output]),
        ),
    ]

generate_source_files_manifest = aspect(
    doc = """Generate a manifest file describing source files for a given set of targets.

Unfortunately, the Bazel stderr output must be filtered to extract the output
file's location, relative to the Bazel execroot. Example usage from the
command-line (this works with --config=quiet):

    bazel build <config-args> <target> \
        --output_groups=+fuchsia_sources_manifest \
        --aspects=//build/bazel/aspects:source_files.bzl%generate_source_files_manifest \
        2>&1 | grep '^DEBUG:.*FUCHSIA_SOURCES_MANIFEST_PATH=' | \
        sed -e 's|.*FUCHSIA_SOURCES_MANIFEST_PATH||'
""",
    implementation = _generate_source_files_list_impl,
    # This aspect does not traverse, so no attr_aspects definition here.
    # Ensure that collect_debug_symbols_manifest_aspect is run first.
    requires = [collect_source_files_aspect],
    # Ensure that the result of collect_debug_symbols_manifest_aspect is available.
    required_aspect_providers = [FuchsiaSourcesInfo],
    provides = [OutputGroupInfo],
)
