#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import sys


def main():
    parser = argparse.ArgumentParser(
        description="Scaffold Rust driver migration"
    )
    parser.add_argument("--name", required=True, help="Name of the driver")
    parser.add_argument(
        "--dir",
        required=True,
        help="Directory of the driver (relative to fuchsia root)",
    )
    parser.add_argument(
        "--bind-target", required=True, help="Bind target label (e.g., :bind)"
    )

    args = parser.parse_args()

    fuchsia_root = os.environ.get("FUCHSIA_DIR")
    if not fuchsia_root:
        cwd = os.getcwd()
        if os.path.exists(os.path.join(cwd, ".jiri_manifest")):
            fuchsia_root = cwd
        else:
            print(
                "Error: FUCHSIA_DIR environment variable not set and could not detect fuchsia root."
            )
            sys.exit(1)

    driver_dir = os.path.join(fuchsia_root, args.dir)
    if not os.path.exists(driver_dir):
        print(f"Error: Directory {driver_dir} does not exist.")
        sys.exit(1)

    template_dir = os.path.join(
        fuchsia_root, "tools/create/templates/driver-default"
    )
    if not os.path.exists(template_dir):
        print(f"Error: Template directory {template_dir} not found.")
        sys.exit(1)

    camel_case_name = "".join(
        word.capitalize() for word in args.name.split("-")
    )

    # 1. Read and adapt lib.rs template
    lib_tmpl_path = os.path.join(template_dir, "src/lib.rs.tmpl-rust")
    if os.path.exists(lib_tmpl_path):
        with open(lib_tmpl_path, "r") as f:
            lib_content = f.read()

        lib_content = lib_content.replace(
            '{{>copyright comment="//"}}',
            """// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
""",
        )
        lib_content = lib_content.replace(
            "{{pascal_case PROJECT_NAME}}", camel_case_name
        )
        lib_content = lib_content.replace(
            "{{snake_case PROJECT_NAME}}", args.name.replace("-", "_")
        )

        # Fix potential dead code warning for skeleton
        lib_content = lib_content.replace("node: Node,", "_node: Node,")
        lib_content = lib_content.replace(
            "Ok(Self { node })", "Ok(Self { _node: node })"
        )
    else:
        print(f"Warning: {lib_tmpl_path} not found. Using fallback template.")
        lib_content = """// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, Node, driver_register};
use zx::Status;

struct %s {
    _node: Node,
}

driver_register!(%s);

impl Driver for %s {
    const NAME: &str = "%s";

    async fn start(mut context: DriverContext) -> Result<Self, Status> {
        let node = context.take_node()?;
        Ok(%s { _node: node })
    }

    async fn stop(&self) {
    }
}
""" % (
            camel_case_name,
            camel_case_name,
            camel_case_name,
            args.name.replace("-", "_"),
            camel_case_name,
        )

    src_dir = os.path.join(driver_dir, "src")
    os.makedirs(src_dir, exist_ok=True)
    lib_rs = os.path.join(src_dir, "lib.rs")

    if not os.path.exists(lib_rs):
        with open(lib_rs, "w") as f:
            f.write(lib_content)
        print(f"Created {lib_rs}")
    else:
        print(f"Skipped {lib_rs} (already exists)")

    # 2. Create meta/name-rust.cml
    meta_dir = os.path.join(driver_dir, "meta")
    os.makedirs(meta_dir, exist_ok=True)
    cml_file = os.path.join(meta_dir, f"{args.name}-rust.cml")

    bind_target_name = args.bind_target.split(":")[-1]
    cml_content = """{
    include: [
        "inspect/client.shard.cml",
        "syslog/client.shard.cml",
    ],
    program: {
        runner: "driver",
        binary: "driver/%s-rust.so",
        bind: "meta/bind/%s.bindbc",
    },
}
""" % (
        args.name,
        bind_target_name,
    )

    if not os.path.exists(cml_file):
        with open(cml_file, "w") as f:
            f.write(cml_content)
        print(f"Created {cml_file}")
    else:
        print(f"Skipped {cml_file} (already exists)")

    # 3. Read and adapt BUILD.gn template
    build_tmpl_path = os.path.join(template_dir, "BUILD.gn.tmpl-rust")
    if os.path.exists(build_tmpl_path):
        with open(build_tmpl_path, "r") as f:
            build_tmpl = f.read()

        start_idx = build_tmpl.find("fuchsia_rust_driver")
        if start_idx != -1:
            gn_content = build_tmpl[start_idx:]
        else:
            gn_content = build_tmpl

        gn_content = gn_content.replace(
            'fuchsia_rust_driver("driver")',
            f'fuchsia_rust_driver("{args.name}-rust-lib")',
        )
        gn_content = gn_content.replace(
            'output_name = "{{snake_case PROJECT_NAME}}"',
            f'output_name = "{args.name}-rust"',
        )
        gn_content = gn_content.replace(
            'fuchsia_driver_component("component")',
            f'fuchsia_driver_component("{args.name}-rust-component")',
        )
        gn_content = gn_content.replace(
            'component_name = "{{snake_case PROJECT_NAME}}"',
            f'component_name = "{args.name}-rust"',
        )
        gn_content = gn_content.replace(
            'manifest = "meta/{{snake_case PROJECT_NAME}}.cml"',
            f'manifest = "meta/{args.name}-rust.cml"',
        )
        gn_content = gn_content.replace('":driver"', f'":{args.name}-rust-lib"')
        gn_content = gn_content.replace(
            '":driver_test"', f'":{args.name}-rust-lib_test"'
        )
        gn_content = gn_content.replace(
            'fuchsia_driver_package("pkg")',
            f'fuchsia_driver_package("{args.name}-rust")',
        )
        gn_content = gn_content.replace(
            'package_name = "{{snake_case PROJECT_NAME}}"',
            f'package_name = "{args.name}-rust"',
        )
        gn_content = gn_content.replace(
            'driver_components = [ ":component" ]',
            f'driver_components = [ ":{args.name}-rust-component" ]',
        )
        gn_content = gn_content.replace('":bind"', f'"{args.bind_target}"')
        gn_content = gn_content.replace(
            'info = "meta/{{snake_case PROJECT_NAME}}_info.json"',
            f'info = "meta/{args.name}-info.json"',
        )
        gn_content = gn_content.replace(
            'fuchsia_unittest_package("{{snake_case PROJECT_NAME}}-unit-test-pkg")',
            f'fuchsia_unittest_package("{args.name}-rust-test-pkg")',
        )
        gn_content = gn_content.replace(
            'package_name = "{{snake_case PROJECT_NAME}}-unit-test"',
            f'package_name = "{args.name}-rust-test"',
        )

        # Remove tests group from template as we handle it manually
        tests_idx = gn_content.find('group("tests")')
        if tests_idx != -1:
            gn_content = gn_content[:tests_idx]

        # Ensure with_unit_tests is true (it is in template, but just in case)
        if "with_unit_tests = true" not in gn_content:
            gn_content = gn_content.replace(
                "fuchsia_rust_driver",
                "fuchsia_rust_driver\n  with_unit_tests = true",
                1,
            )

    else:
        print(f"Warning: {build_tmpl_path} not found. Using fallback template.")
        gn_content = """
fuchsia_rust_driver("%s-rust-lib") {
  output_name = "%s-rust"
  edition = "2024"
  source_root = "src/lib.rs"
  sources = [ "src/lib.rs" ]
  with_unit_tests = true
  deps = [
    "//sdk/fidl/fuchsia.driver.framework:fuchsia.driver.framework_rust",
    "//sdk/lib/driver/component/rust",
    "//sdk/rust/zx",
    "//src/lib/fidl/rust/fidl",
    "//src/lib/fuchsia-async",
    "//src/lib/fuchsia-component",
    "//third_party/rust_crates:futures",
    "//third_party/rust_crates:log",
  ]
}

fuchsia_unittest_package("%s-rust-test-pkg") {
  package_name = "%s-rust-test"
  deps = [ ":%s-rust-lib_test" ]
}

fuchsia_driver_component("%s-rust-component") {
  component_name = "%s-rust"
  manifest = "meta/%s-rust.cml"
  deps = [
    ":%s-rust-lib",
    "%s",
  ]
}

fuchsia_driver_package("%s-rust") {
  package_name = "%s-rust"
  driver_components = [ ":%s-rust-component" ]
}
""" % (
            args.name,
            args.name,
            args.name,
            args.name,
            args.name,
            args.name,
            args.name,
            args.name,
            args.name,
            args.bind_target,
            args.name,
            args.name,
            args.name,
        )

    build_gn = os.path.join(driver_dir, "BUILD.gn")

    with open(build_gn, "r") as f:
        build_gn_content = f.read()

    if f'fuchsia_rust_driver("{args.name}-rust-lib")' not in build_gn_content:
        with open(build_gn, "a") as f:
            f.write(gn_content)
        print(f"Appended Rust targets to {build_gn}")
    else:
        print(f"Rust targets already exist in {build_gn}")

    # 4. Update tests group
    with open(build_gn, "r") as f:
        content = f.read()

    label = f":{args.name}-rust-test-pkg"
    if 'group("tests")' in content:
        lines = content.splitlines()
        in_tests_group = False
        deps_line_index = -1
        for i, line in enumerate(lines):
            if 'group("tests")' in line:
                in_tests_group = True
            elif in_tests_group and "deps = [" in line:
                deps_line_index = i
                break
            elif in_tests_group and line.strip() == "}":
                in_tests_group = False

        if deps_line_index != -1:
            deps_line = lines[deps_line_index]
            if not any(label in l for l in lines[deps_line_index:]):
                if "]" in deps_line:
                    lines[deps_line_index] = deps_line.replace(
                        "]", f', "{label}" ]'
                    )
                else:
                    lines.insert(deps_line_index + 1, f'    "{label}",')

                content = "\n".join(lines)
                with open(build_gn, "w") as f:
                    f.write(content)
                print(f"Added {label} to existing tests group in {build_gn}")
            else:
                print(f"{label} already in tests group")
        else:
            print(
                f"Warning: Found group('tests') but could not find deps list in {build_gn}"
            )
    else:
        tests_content = (
            """
group("tests") {
  testonly = true
  deps = [ ":%s-rust-test-pkg" ]
}
"""
            % args.name
        )
        with open(build_gn, "a") as f:
            f.write(tests_content)
        print(f"Created tests group in {build_gn}")

    # 4.5 Update local drivers group
    with open(build_gn, "r") as f:
        content = f.read()

    pkg_label = f":{args.name}-rust"
    if 'group("drivers")' in content:
        lines = content.splitlines()
        in_drivers_group = False
        deps_line_index = -1
        for i, line in enumerate(lines):
            if 'group("drivers")' in line:
                in_drivers_group = True
            elif in_drivers_group and "deps = [" in line:
                deps_line_index = i
                break
            elif in_drivers_group and line.strip() == "}":
                in_drivers_group = False

        if deps_line_index != -1:
            deps_line = lines[deps_line_index]
            if not any(pkg_label in l for l in lines[deps_line_index:]):
                if "]" in deps_line:
                    lines[deps_line_index] = deps_line.replace(
                        "]", f', "{pkg_label}" ]'
                    )
                else:
                    lines.insert(deps_line_index + 1, f'    "{pkg_label}",')

                content = "\n".join(lines)
                with open(build_gn, "w") as f:
                    f.write(content)
                print(
                    f"Added {pkg_label} to existing drivers group in {build_gn}"
                )
            else:
                print(f"{pkg_label} already in drivers group")
        else:
            print(
                f"Warning: Found group('drivers') but could not find deps list in {build_gn}"
            )
    else:
        # Scan for existing packages
        import re

        packages = re.findall(r'fuchsia_driver_package\("([^"]+)"\)', content)

        deps_list = [f":{p}" for p in packages]
        rust_label = f":{args.name}-rust"
        if rust_label not in deps_list:
            deps_list.append(rust_label)

        deps_str = ",\n    ".join([f'"{d}"' for d in deps_list])

        drivers_content = (
            """
group("drivers") {
  deps = [
    %s,
  ]
}
"""
            % deps_str
        )
        with open(build_gn, "a") as f:
            f.write(drivers_content)
        print(f"Created drivers group in {build_gn} with packages: {deps_list}")

    # 5. Update parent tests group
    parent_dir = os.path.dirname(driver_dir)
    parent_build_gn = os.path.join(parent_dir, "BUILD.gn")
    if os.path.exists(parent_build_gn):
        with open(parent_build_gn, "r") as f:
            parent_content = f.read()

        if 'group("tests")' in parent_content:
            parent_lines = parent_content.splitlines()
            in_tests_group = False
            deps_line_index = -1
            for i, line in enumerate(parent_lines):
                if 'group("tests")' in line:
                    in_tests_group = True
                elif in_tests_group and "deps = [" in line:
                    deps_line_index = i
                    break
                elif in_tests_group and line.strip() == "}":
                    in_tests_group = False

            if deps_line_index != -1:
                parent_deps_line = parent_lines[deps_line_index]
                dir_name = os.path.basename(args.dir)
                parent_label = f'"{dir_name}:tests"'

                if not any(
                    parent_label in l for l in parent_lines[deps_line_index:]
                ):
                    if "]" in parent_deps_line:
                        parent_lines[
                            deps_line_index
                        ] = parent_deps_line.replace("]", f", {parent_label} ]")
                    else:
                        parent_lines.insert(
                            deps_line_index + 1, f"    {parent_label},"
                        )

                    parent_content = "\n".join(parent_lines)
                    with open(parent_build_gn, "w") as f:
                        f.write(parent_content)
                    print(
                        f"Added {parent_label} to parent tests group in {parent_build_gn}"
                    )
                else:
                    print(f"{parent_label} already in parent tests group")
            else:
                print(
                    f"Warning: Found group('tests') but could not find deps list in {parent_build_gn}"
                )
        else:
            print(
                f"Info: Parent {parent_build_gn} does not have a tests group. You may need to add it manually."
            )

    # 5.5 Update parent drivers group
    if os.path.exists(parent_build_gn):
        with open(parent_build_gn, "r") as f:
            parent_content = f.read()

        if 'group("drivers")' in parent_content:
            parent_lines = parent_content.splitlines()
            in_drivers_group = False
            deps_line_index = -1
            for i, line in enumerate(parent_lines):
                if 'group("drivers")' in line:
                    in_drivers_group = True
                elif in_drivers_group and "deps = [" in line:
                    deps_line_index = i
                    break
                elif in_drivers_group and line.strip() == "}":
                    in_drivers_group = False

            if deps_line_index != -1:
                parent_deps_line = parent_lines[deps_line_index]
                dir_name = os.path.basename(args.dir)
                parent_label = f'"{dir_name}:drivers"'

                if not any(
                    parent_label in l for l in parent_lines[deps_line_index:]
                ):
                    if "]" in parent_deps_line:
                        parent_lines[
                            deps_line_index
                        ] = parent_deps_line.replace("]", f", {parent_label} ]")
                    else:
                        parent_lines.insert(
                            deps_line_index + 1, f"    {parent_label},"
                        )

                    parent_content = "\n".join(parent_lines)
                    with open(parent_build_gn, "w") as f:
                        f.write(parent_content)
                    print(
                        f"Added {parent_label} to parent drivers group in {parent_build_gn}"
                    )
                else:
                    print(f"{parent_label} already in parent drivers group")
            else:
                print(
                    f"Warning: Found group('drivers') but could not find deps list in {parent_build_gn}"
                )
        else:
            print(
                f"Info: Parent {parent_build_gn} does not have a drivers group. You may need to add it manually."
            )

    # 6. Append to all_drivers_list.txt
    all_drivers_path = os.path.join(
        fuchsia_root, "build/drivers/all_drivers_list.txt"
    )
    if os.path.exists(all_drivers_path):
        label = f"//{args.dir}:{args.name}-rust-lib\n"
        with open(all_drivers_path, "r") as f:
            lines = f.readlines()
        if label not in lines:
            with open(all_drivers_path, "a") as f:
                f.write(label)
            print(f"Added {label.strip()} to {all_drivers_path}")
        else:
            print(f"Label {label.strip()} already in {all_drivers_path}")
    else:
        print(f"Warning: {all_drivers_path} not found. Could not add label.")

    # 7. Format files
    print("Formatting files...")
    files_to_format = [
        os.path.join(driver_dir, "src/lib.rs"),
        os.path.join(driver_dir, f"meta/{args.name}-rust.cml"),
        os.path.join(driver_dir, "BUILD.gn"),
    ]
    # Filter out non-existent files
    files_to_format = [f for f in files_to_format if os.path.exists(f)]

    if files_to_format:
        import subprocess

        # Use relative paths from fuchsia_root for the command
        rel_files = [os.path.relpath(f, fuchsia_root) for f in files_to_format]
        files_str = ",".join(rel_files)
        try:
            subprocess.run(
                f"fx format-code --files={files_str}",
                shell=True,
                cwd=fuchsia_root,
                check=True,
            )
            print(f"Formatted files: {files_str}")
        except subprocess.CalledProcessError as e:
            print(f"Warning: fx format-code failed: {e}")


if __name__ == "__main__":
    main()
