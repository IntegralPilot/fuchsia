# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for generating OSS licenses compliance materials."""

def _fuchsia_licenses_review(ctx):
    out_dir = ctx.actions.declare_directory(ctx.label.name + ".unzipped")
    out_zip = ctx.actions.declare_file(ctx.label.name)

    run_inputs = [ctx.file.spdx_input]
    run_arguments = [
        "--spdx_input=%s" % ctx.file.spdx_input.path,
        "--output_dir=%s" % out_dir.path,
        "--output_file=%s" % out_zip.path,
        "--quiet",
    ]

    if ctx.file.classification_input:
        run_inputs.append(ctx.file.classification_input)
        run_arguments.append("--classification_input=%s" % ctx.file.classification_input.path)

    extra_files = ctx.files.extra_files
    if ctx.files.extra_files:
        run_inputs.extend(extra_files)
        run_arguments.append("--extra_files")
        run_arguments.extend([f.path for f in extra_files])

    ctx.actions.run(
        progress_message = "Generating license review material into %s" % out_dir.path,
        inputs = run_inputs,
        outputs = [out_dir, out_zip],
        executable = ctx.executable._generate_licenses_review_tool,
        arguments = run_arguments,
    )

    return [DefaultInfo(files = depset([out_zip]), runfiles = ctx.runfiles([out_zip]))]

fuchsia_licenses_review = rule(
    doc = """
Produces a zip file with \\[name\\] containing license review material.

The file contains:

  + summary.csv
  + licenses.spdx.json
  + classifications.json (optional)
  + extracted_licenses (directory)
    + LicenseRef-1.txt
    + LicenseRef-2.txt
    + LicenseRef-3.txt
    + ...
    + (A txt file with the contents of each extracted license)
  + extra_files (directory)
    + ... (additional files can be added here via extra_files arguments)

The SPDX json conforms with:
https://github.com/spdx/spdx-spec/blob/master/schemas/spdx-schema.json
""",
    implementation = _fuchsia_licenses_review,
    attrs = {
        "spdx_input": attr.label(
            doc = "The output of `fuchsia_licenses_spdx` invocation.",
            allow_single_file = True,
            mandatory = True,
        ),
        "classification_input": attr.label(
            doc = "The output of `fuchsia_licenses_classification` invocation (optional).",
            allow_single_file = True,
        ),
        "extra_files": attr.label_list(
            doc = "Additional files to add to the archive.",
            allow_files = True,
        ),
        "_generate_licenses_review_tool": attr.label(
            executable = True,
            cfg = "exec",
            default = "//fuchsia/tools/licenses:generate_licenses_review",
        ),
    },
)
