# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""fuchsia_archivist_pipeline_test() rule."""

load(":fuchsia_component.bzl", "fuchsia_test_component")
load(":fuchsia_component_manifest.bzl", "fuchsia_component_manifest")
load(":fuchsia_package.bzl", "fuchsia_test_package")

CML = """{{
    "program": {{
        "runner": "inspect_test_runner",
        "accessor": "ALL",
        "timeout_seconds": "60",
        "cases": [
          "bootstrap/archivist:root/fuchsia.inspect.Health:status WHERE [s] s == 'OK'",
          "{filtering_enabled_selector}",
          "{count_selector}",
          {selectors}
        ]
    }},
    "capabilities": [
        {{ "protocol": "fuchsia.test.Suite" }}
    ],
    "expose": [
        {{
            "protocol": "fuchsia.test.Suite",
            "from": "self"
        }}
    ]
}}
"""

FILTERING_ENABLED_SELECTOR = "bootstrap/archivist:root/pipelines/{pipeline_name}:filtering_enabled WHERE [s] {value}"

# This selector checks that the number of selectors we see configured are greater or equal to the
# number of selector files we got. This should be a good enough signal to know that the pipeline is
# configured correctly. Further checking would require parsing the selector files, deduping, etc.
COUNT_SELECTOR = "bootstrap/archivist:root/pipelines/{pipeline_name}:selector_count WHERE [s] s >= {count}"

CONFIG_FILE_SELECTOR = "\"bootstrap/archivist:root/pipelines/{pipeline_name}/config_files/{file_name}\""

def _file_to_selector(pipeline_name, file):
    """
    Creates an archivist selector for the given pipeline and selector config file.

    Args:
        pipeline_name: The name of the privacy pipeline.
        file: A file in that pipeline.

    Returns:
        A selector to verify that config health in the archivist inspect.
    """
    if file.extension != "cfg":
        return None
    file_name = file.basename.removesuffix("." + file.extension)
    return CONFIG_FILE_SELECTOR.format(
        pipeline_name = pipeline_name,
        file_name = file_name,
    )

def _fuchsia_archivist_pipeline_test_manifest_impl(ctx):
    selectors = []
    for target in ctx.attr.inspect:
        for file in target.files.to_list():
            selector = _file_to_selector(ctx.attr.pipeline_name, file)
            if selector:
                selectors.append(selector)
    count = len(selectors)
    selectors = ",".join(selectors)

    count_selector = COUNT_SELECTOR.format(
        pipeline_name = ctx.attr.pipeline_name,
        count = count,
    )

    value = "Not(s)" if ctx.attr.expect_disabled else "s"
    filtering_enabled_selector = FILTERING_ENABLED_SELECTOR.format(
        pipeline_name = ctx.attr.pipeline_name,
        value = value,
    )

    cml = CML.format(
        selectors = selectors,
        count_selector = count_selector,
        filtering_enabled_selector = filtering_enabled_selector,
    )

    manifest = ctx.actions.declare_file(ctx.attr.component_name + ".cml")
    ctx.actions.write(
        output = manifest,
        content = cml,
    )
    return [DefaultInfo(files = depset([manifest]))]

# This is public so that unit tests can access it.
fuchsia_archivist_pipeline_test_manifest = rule(
    doc = """Constructs a component manifest for a archive pipeline test.""",
    implementation = _fuchsia_archivist_pipeline_test_manifest_impl,
    attrs = {
        "pipeline_name": attr.string(
            doc = "Name of the pipeline",
            mandatory = True,
        ),
        "component_name": attr.string(
            doc = "Name of the component",
            mandatory = True,
        ),
        "inspect": attr.label_list(
            doc = "List of inspect files to test",
            mandatory = True,
            allow_empty = True,
            allow_files = True,
        ),
        "expect_disabled": attr.bool(
            doc = "If set, expect the pipeline to be disabled",
            default = False,
        ),
    },
)

# buildifier: disable=function-docstring
def fuchsia_archivist_pipeline_test(name, pipeline_name, inspect = [], expect_disabled = False):
    cml_name = name + "_cml"
    fuchsia_archivist_pipeline_test_manifest(
        name = cml_name,
        pipeline_name = pipeline_name,
        component_name = name,
        inspect = inspect,
        expect_disabled = expect_disabled,
    )

    cm_name = name + "_cm"
    fuchsia_component_manifest(
        name = cm_name,
        src = ":" + cml_name,
    )

    component_name = name + "_component"
    fuchsia_test_component(
        name = component_name,
        component_name = component_name,
        manifest = ":" + cm_name,
    )

    fuchsia_test_package(
        name = name,
        test_components = [":" + component_name],
        fuchsia_api_level = "HEAD",
    )
