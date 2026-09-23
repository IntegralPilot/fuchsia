# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""file-making utilities."""

# buildifier: disable=bzl-visibility
load("@rules_fuchsia//fuchsia/private:providers.bzl", "FuchsiaComponentManifestInfo")

def _make_file_impl(ctx):
    f = ctx.actions.declare_file(ctx.attr.filename)
    ctx.actions.write(f, ctx.attr.content)
    return DefaultInfo(files = depset([f]))

make_file = rule(
    implementation = _make_file_impl,
    doc = """A simple rule for making a file.

    This could be achieved with a genrule that cats to a file but this provides
    a simpler interface.""",
    attrs = {
        "filename": attr.string(),
        "content": attr.string(),
    },
)

def _make_fake_component_manifest_impl(ctx):
    manifest_out = ctx.actions.declare_file("meta/{}.cm".format(ctx.attr.component_name))
    ctx.actions.write(manifest_out, content = "", is_executable = False)
    return [
        DefaultInfo(files = depset([manifest_out])),
        FuchsiaComponentManifestInfo(
            compiled_manifest = manifest_out,
            component_name = ctx.attr.component_name,
            config_package_path = "meta/%s.cvf" % ctx.attr.component_name,
        ),
    ]

make_fake_component_manifest = rule(
    implementation = _make_fake_component_manifest_impl,
    attrs = {
        "component_name": attr.string(mandatory = True),
    },
)
