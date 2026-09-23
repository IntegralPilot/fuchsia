# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_rules_common//components:component_manifest.bzl",
    "compile_component_manifest",
)
load(
    "@fuchsia_rules_common//components:providers.bzl",
    "FuchsiaComponentManifestInfo",
)

# buildifier: disable=module-docstring
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(
    ":providers.bzl",
    "FuchsiaComponentManifestShardCollectionInfo",
    "FuchsiaComponentManifestShardInfo",
)

_COMMON_CMC_ATTRIBUTES = {
    # This is to get the coverage.shard.cml in the SDK, so it can be merged
    # in when coverage is enabled.
    "_sdk_coverage_shard": attr.label(
        default = "@fuchsia_sdk//pkg/sys/testing:coverage",
    ),
}

def _component_name_from_cml_file(cml_file):
    if not cml_file.basename.endswith(".cml"):
        fail('The component manifest file\'s basename must end with ".cml", but got "{}".'.format(
            cml_file.basename,
        ))
    return cml_file.basename[:-4]

def _fuchsia_component_manifest_shard_collection_impl(ctx):
    return FuchsiaComponentManifestShardCollectionInfo(
        shards = [dep for dep in ctx.attr.deps],
    )

fuchsia_component_manifest_shard_collection = rule(
    doc = """Encapsulates a collection of component manifests and their include paths.

    This rule is not intended to be used directly. Rather, it should be added to the
    fuchsia sdk toolchain to be added as implicit dependencies for all manifests.
""",
    implementation = _fuchsia_component_manifest_shard_collection_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "A list of component manifest shard targets to collect.",
            providers = [[FuchsiaComponentManifestShardInfo]],
        ),
    },
)

def _fuchsia_component_manifest_shard_impl(ctx):
    return [
        FuchsiaComponentManifestShardInfo(
            file = ctx.file.src,
            base_path = ctx.attr.include_path,
        ),
    ]

fuchsia_component_manifest_shard = rule(
    doc = """Encapsulates a component manifest shard from a input file.
""",
    implementation = _fuchsia_component_manifest_shard_impl,
    attrs = {
        "include_path": attr.string(
            doc = "Base path of the shard, used in includepath argument of cmc compile",
            mandatory = True,
        ),
        "src": attr.label(
            doc = "The component manifest shard",
            allow_single_file = [".cml"],
        ),
    },
)

def _compile_component_manifest(ctx, manifest_in, component_name, includes_in):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    if ctx.configuration.coverage_enabled:
        coverage_shard = ctx.attr._sdk_coverage_shard[FuchsiaComponentManifestShardInfo]
        manifest_merged = ctx.actions.declare_file("%s_merged.cml" % _component_name_from_cml_file(manifest_in))
        ctx.actions.run(
            executable = sdk.cmc,
            arguments = [
                "merge",
                "--output",
                manifest_merged.path,
                manifest_in.path,
                coverage_shard.file.path,
            ],
            inputs = [
                manifest_in,
                coverage_shard.file,
            ],
            outputs = [manifest_merged],
            mnemonic = "CmcMergeManifests",
            toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
        )
        manifest_in = manifest_merged

    # use a dict to eliminate duplicate include paths
    include_path_dict = {}
    includes = []
    for dep in includes_in + sdk.cmc_includes[FuchsiaComponentManifestShardCollectionInfo].shards:
        if FuchsiaComponentManifestShardInfo in dep:
            shard = dep[FuchsiaComponentManifestShardInfo]
            includes.append(shard.file)
            include_path_dict[(shard.file.owner.workspace_root or ".") + "/" + shard.base_path] = 1

    include_paths = []
    for w in include_path_dict.keys():
        include_paths.append(w)

    return compile_component_manifest(
        ctx = ctx,
        cmc_tool = sdk.cmc,
        manifest_in = manifest_in,
        component_name = component_name,
        includes = includes,
        include_paths = include_paths,
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
    )

def _fuchsia_component_manifest_impl(ctx):
    if bool(ctx.file.src) == bool(ctx.attr.content):
        fail("Exactly one of 'src' and 'content' must be specified.")

    if ctx.attr.content and not ctx.attr.component_name:
        fail("When 'content' is specified, 'component_name' must also be specified.")

    component_name = ctx.attr.component_name or _component_name_from_cml_file(ctx.file.src)

    if ctx.file.src:
        manifest_in = ctx.file.src
    else:
        manifest_in = ctx.actions.declare_file("%s.cml" % component_name)
        ctx.actions.write(
            output = manifest_in,
            content = ctx.attr.content,
        )

    return _compile_component_manifest(ctx, manifest_in, component_name, ctx.attr.includes)

_fuchsia_component_manifest = rule(
    doc = """Compiles a component manifest from a input file.

This rule will compile an input cml file and output a cm file. The file can,
optionally, include additional cml files but they must be relative to the
src file and included in the includes attribute.

```
{
    include: ["foo.cml", "some_dir/bar.cml"]
}
```

If `component_name` is provided, the generated `.cm` file will inherit that
name. Otherwise, it will keep the same basename as the source file.
""",
    implementation = _fuchsia_component_manifest_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "src": attr.label(
            doc = "The source manifest file to compile. Either it or `content` must be specified.",
            allow_single_file = [".cml"],
        ),
        "content": attr.string(
            doc = "Inline content for the manifest. Either it or `src` must be specified.",
        ),
        "component_name": attr.string(
            doc = "Name of the component for inline manifests. Must be specified when 'content' is specified.",
        ),
        "includes": attr.label_list(
            doc = "A list of dependencies that are included in the `src` cml file.",
            providers = [FuchsiaComponentManifestShardInfo],
        ),
    } | COMPATIBILITY.HOST_ATTRS | _COMMON_CMC_ATTRIBUTES,
)

def fuchsia_component_manifest(*, name, tags = ["manual"], **kwargs):
    _fuchsia_component_manifest(
        name = name,
        tags = tags,
        **kwargs
    )

def _ensure_compiled_component_manifest_impl(ctx):
    if FuchsiaComponentManifestInfo in ctx.attr.dep:
        # This is already a compiled manifest so just return the providers.
        return [
            ctx.attr.dep[DefaultInfo],
            ctx.attr.dep[FuchsiaComponentManifestInfo],
        ]
    else:
        return _compile_component_manifest(
            ctx,
            ctx.file.dep,
            _component_name_from_cml_file(ctx.file.dep),
            [],
        )

ensure_compiled_component_manifest = rule(
    implementation = _ensure_compiled_component_manifest_impl,
    doc = """Checks to see if dep is a compiled manifest or plain file.

    This rule is not intended for general usage but is meant to be used in the
    fuchsia_component macros to ensure that the target that is passed in as the
    manifest gets compiled.
    """,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "dep": attr.label(
            doc = "The dependency to check. This should either be a plain cml file or a fuchsia_component_manifest target.",
            allow_single_file = [".cml", ".cm"],
        ),
    } | COMPATIBILITY.HOST_ATTRS | _COMMON_CMC_ATTRIBUTES,
    provides = [FuchsiaComponentManifestInfo],
)
