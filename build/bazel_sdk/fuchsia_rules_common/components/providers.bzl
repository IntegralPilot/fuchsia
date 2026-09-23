# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

FuchsiaComponentInfo = provider(
    doc = "Contains information about a fuchsia component",
    fields = {
        "name": "name of the component",
        "manifest": "A file representing the compiled component manifest file",
        "resources": "any additional resources the component needs",
        "moniker": "The moniker to run the non-driver, non-test, non-session component in",
        "is_driver": "True if this is a driver",
        "is_test": "True if this is a test component",
        "run_tag": "A tag used to identify the component when put in a package to be later used by the run command",
    },
)

FuchsiaComponentManifestInfo = provider(
    "Contains information about a Fuchsia component manifest",
    fields = {
        "compiled_manifest": "A File pointing to the compiled manifest",
        "component_name": "The name of the component",
        "config_package_path": "The path to the generated cvf file",
    },
)

FuchsiaPackagedComponentInfo = provider(
    doc = "Contains information about a fuchsia component that has been included in a package",
    fields = {
        "component_info": "The original FuchsiaComponentInfo provider if this is built locally. Otherwise it will be empty",
        "dest": "The install location for this component in a package (meta/foo.cm)",
    },
)
