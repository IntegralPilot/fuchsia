# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines SHAC checks for FIDL library migration from GN to Bazel.

These checks are only needed until the migration from GN to Bazel is complete.
"""

load("./common.star", "os_exec")

# FIDL library migration checks:
# 1. All the attributes in BUILD.bazel should have a mapped attribute in BUILD.gn.
# 2. All the attributes in BUILD.gn should be mapped to attributes in BUILD.bazel.
# 3. The copyright years in new created BUILD.bazel should be current year.
# 4. All migrated targets should be added to the `deps` list of `bazel2gn_verifications` group.
# 5. Migrated targets should be added to the targets_lists in `//sdk/fidl/category_lists.bzl`, according to its `category` and `stable` attributes.
# 6. The "//build/tools/bazel2gn/bazel_migration.gni" should be imported in the BUILD.gn of migrated libraries.
# 7. In the BUILD.bazel, the "fidl_library" should be loaded from "//build/bazel/rules/fidl:fidl_library.bzl".
# 8. visibility attribute should exist in both BUILD.bazel and BUILD.gn.
# 9. target name in BUILD.bazel should be the same as in BUILD.gn.
# 10. Attributes in BUILD.bazel and BUILD.gn should have the same values.

# Verified by changing copyright year to 2025.
# The Gerrit SHAC does report the expected warning. It confirms:
# 1. Current year can be obtained correctly.
# 2. The BUILD.bazel file is read correctly.
# 3. The copyright year checking works as expected.
def _check_copyright_year(ctx, path, content):
    """Checks if the copyright year of a newly created BUILD.bazel file is current year."""

    # Get current year.
    date_res = os_exec(ctx, ["/bin/date", "+%Y %m"]).wait()
    date_parts = date_res.stdout.strip().split()
    current_year = date_parts[0]

    # Check Item 3
    # The first line should start with "# Copyright".
    lines = str(content).splitlines()
    if len(lines) > 0:
        line = lines[0]
        if not line.startswith("# Copyright"):
            ctx.emit.finding(
                message = "First line of BUILD.bazel must be a copyright header.",
                filepath = path,
                level = "error",
                line = 1,
            )
        elif not line.startswith("# Copyright " + current_year):
            ctx.emit.finding(
                message = "Copyright year should be '%s'. Please double check if the copyright year is correct." % current_year,
                filepath = path,
                level = "warning",
                line = 1,
            )
    else:
        ctx.emit.finding(
            message = "BUILD.bazel file is empty or could not be read.",
            filepath = path,
            level = "error",
            line = 1,
        )

# Verified by changing a different target name in BUILD.gn.
# Errors are reported as expected. This confirms:
# 1. Both BUILD.bazel and BUILD.gn can be read correctly.
# 2. Target names are retrieved correctly.
# 3. Attributes and attribute values are retrieved and mapped correctly.
def _check_build_files(ctx, target_files):
    """Checks consistency of BUILD.bazel and BUILD.gn files for migrated FIDL libraries."""

    # Only include the attributes which have different name strings in
    # bazel and gn files into this mapping. Attributes not in this
    # mapping are deemed to have the same name and value in both files.
    bazel_to_gn_mapping = {
        "srcs": "sources",
        "deps": "public_deps",
        "api_area": "sdk_area",
        "category": "sdk_category",
        "library_name": "name",
    }

    gn_to_bazel_mapping = {
        "sources": "srcs",
        "public_deps": "deps",
        "sdk_area": "api_area",
        "sdk_category": "category",
        "name": "library_name",
    }

    # Apply the check to the libraries in current CL that have both BUILD.bazel and BUILD.gn.
    # Exclude //sdk/fidl/BUILD.bazel and //sdk/fidl/BUILD.gn.
    for file_info in target_files:
        path = file_info.path
        bazel_content = file_info.bazel_content
        gn_content = file_info.gn_content
        bazel_targets = file_info.bazel_targets
        gn_targets = file_info.gn_targets

        dir_path = _get_dir_path(path)
        if not dir_path:
            ctx.emit.finding(
                message = "Cannot get directory path",
                filepath = path,
                level = "error",
            )
            continue

        # Check Item 9, target names should be the same.
        bazel_target_names = bazel_targets.keys()
        gn_target_names = gn_targets.keys()

        for t in bazel_target_names:
            if t not in gn_target_names:
                ctx.emit.finding(
                    message = "FIDL library target '%s' found in BUILD.bazel does not have a corresponding target in BUILD.gn." % t,
                    filepath = path,
                    level = "error",
                )
        for t in gn_target_names:
            if t not in bazel_target_names:
                ctx.emit.finding(
                    message = "FIDL library target '%s' found in BUILD.gn does not have a corresponding target in BUILD.bazel." % t,
                    filepath = path,
                    level = "error",
                )

        # Get the directory name. It's used for checking if the fidl target name is
        # the same with directory name when it's under //sdk/fidl.
        last_slash_idx = dir_path.rfind("/")
        dir_name = dir_path[last_slash_idx + 1:] if last_slash_idx != -1 else dir_path

        # For libraries under sdk/fidl/, the target name must match the directory name.
        if path.startswith("sdk/fidl/"):
            for t in bazel_target_names:
                if t != dir_name:
                    ctx.emit.finding(
                        message = "Target name in BUILD.bazel ('%s') must match directory name ('%s') for libraries under sdk/fidl/." % (t, dir_name),
                        filepath = path,
                        level = "error",
                    )

        # Check Item 7, BUILD.bazel must load fidl_library from //build/bazel/rules/fidl:fidl_library.bzl
        load_matches = ctx.re.allmatches(r'load\(\s*"//build/bazel/rules/fidl:fidl_library.bzl"\s*,\s*"fidl_library"\s*\)', str(bazel_content))
        if not load_matches:
            ctx.emit.finding(
                message = "BUILD.bazel must load fidl_library from //build/bazel/rules/fidl:fidl_library.bzl",
                filepath = path,
                level = "error",
            )

        # Check Item 6, BUILD.gn must import //build/tools/bazel2gn/bazel_migration.gni
        if "import(\"//build/tools/bazel2gn/bazel_migration.gni\")" not in str(gn_content):
            ctx.emit.finding(
                message = "BUILD.gn must import //build/tools/bazel2gn/bazel_migration.gni",
                filepath = path,
                level = "error",
            )

        # This is the dictionary to keep targets and their attributes values in both gn and bazel build files.
        combined_dict = {}

        for target, attrs in bazel_targets.items():
            for attr, val in attrs.items():
                key = target + "-" + attr
                combined_dict[key] = {"target": target, "attr": attr, "bazel": val, "gn": None}

        for target, attrs in gn_targets.items():
            for attr, val in attrs.items():
                mapped_attr = gn_to_bazel_mapping.get(attr, attr)
                key = target + "-" + mapped_attr
                if key in combined_dict:
                    combined_dict[key]["gn"] = val
                else:
                    combined_dict[key] = {"target": target, "attr": mapped_attr, "bazel": None, "gn": val}

        # Ensure visibility is checked for all targets even if missing in both
        for target in bazel_targets:
            key = target + "-visibility"
            if key not in combined_dict:
                combined_dict[key] = {"target": target, "attr": "visibility", "bazel": None, "gn": None}

        # Check Item 1 & 2, Attributes must match between BUILD.bazel and BUILD.gn
        for key, item in combined_dict.items():
            target = item["target"]
            attr = item["attr"]
            bazel_val = item["bazel"]
            gn_val = item["gn"]

            # Check visibility by _check_visibility() function.
            if attr == "visibility":
                _check_visibility(ctx, path, target, bazel_val, gn_val)
                continue

            # Skip name check, it's handled in Check Item 9.
            if attr == "name":
                continue

            # WORKAROUND: The `bazel_migration_fidl.star` regex parser does not support parsing
            # dictionary values `{ ... }` from BUILD files. The `zither` attribute uses dictionaries
            # (e.g. `zither = { c = ... }`). The regex parser incorrectly extracts the dictionary keys
            # (`c`, `output_namespace`) as top-level attributes in GN, but fails to extract them in Bazel,
            # leading to false positive mismatch errors.
            # We explicitly ignore these attributes here since `zither` is a very rare and specialized case.
            if attr in ["zither", "c", "output_namespace"]:
                continue

            expected_gn_attr = bazel_to_gn_mapping.get(attr, attr)

            if bazel_val == None:
                ctx.emit.finding(
                    message = "Attribute '%s' in BUILD.gn of FIDL library target '%s' does not have a mapped attribute in BUILD.bazel." % (expected_gn_attr, target),
                    filepath = path,
                    level = "error",
                )
            elif gn_val == None:
                ctx.emit.finding(
                    message = "Attribute '%s' in BUILD.bazel of FIDL library target '%s' does not have a mapped attribute in BUILD.gn." % (attr, target),
                    filepath = path,
                    level = "error",
                )
            else:
                # Both are not None! Compare values!
                if attr == "deps":
                    bazel_val = [_normalize_dep(d) for d in bazel_val]
                    gn_val = [_normalize_dep(d) for d in gn_val]

                if type(bazel_val) == "list" and type(gn_val) == "list":
                    if sorted(bazel_val) != sorted(gn_val):
                        ctx.emit.finding(
                            message = "Attribute '%s' in BUILD.bazel of FIDL library target '%s' has different values from '%s' in BUILD.gn." % (attr, target, expected_gn_attr),
                            filepath = path,
                            level = "error",
                        )
                elif bazel_val != gn_val:
                    ctx.emit.finding(
                        message = "Attribute '%s' in BUILD.bazel of FIDL library target '%s' has different values from '%s' in BUILD.gn." % (attr, target, expected_gn_attr),
                        filepath = path,
                        level = "error",
                    )

# Check Item 4
# Verify by removing the migrated target from the bazel2gn_verification_targets.gni in //sdk/fidl
# It confirms:
# 1. The BUILD.gn in //build directory is read correctly.
# 2. The bazel2gn_verification_targets.gni in //sdk/fidl directory is read correctly.
def _check_bazel2gn_verification_inclusion(ctx, target_files):
    """Checks if migrated targets are added to bazel2gn_verifications."""

    # Get the locations of `bazel2gn_verification_targets.gni` files from `//build` directory.
    build_gn_path = "build/BUILD.gn"
    build_gn_content = ctx.io.read_file(ctx.scm.root + "/" + build_gn_path)
    if build_gn_content == None:
        ctx.emit.finding(
            message = "Could not read //build/BUILD.gn",
            filepath = build_gn_path,
            level = "error",
        )
        return

    # Extract all imports of bazel2gn_verification_targets.gni
    bazel2gn_verification_targets_gni_import = []
    for line in str(build_gn_content).splitlines():
        matches = ctx.re.allmatches(r'^import\("//([^"]*bazel2gn_verification_targets\.gni)"\)', line)
        if matches:
            bazel2gn_verification_targets_gni_import.append(matches[0].groups[1])

    if not bazel2gn_verification_targets_gni_import:
        ctx.emit.finding(
            message = "Could not find any imports of `bazel2gn_verification_targets.gni` in `//build/BUILD.gn`.",
            filepath = build_gn_path,
            level = "error",
        )
        return

    # Apply the check to the libraries in current CL with both BUILD.bazel and BUILD.gn.
    for file_info in target_files:
        path = file_info.path

        dir_path = _get_dir_path(path)
        if not dir_path:
            ctx.emit.finding(
                message = "Could not determine directory path for file.",
                filepath = path,
                level = "error",
            )
            continue

        # Find matching .gni file
        matching_gni = ""
        longest_prefix_len = -1
        for imp in bazel2gn_verification_targets_gni_import:
            gni_dir = imp[:imp.rfind("/")] if "/" in imp else ""
            if path.startswith(gni_dir) and len(gni_dir) > longest_prefix_len:
                matching_gni = imp
                longest_prefix_len = len(gni_dir)

        if not matching_gni:
            matching_gni = "build/bazel2gn_verification_targets.gni"

        # Read matching .gni file
        gni_content = ctx.io.read_file(ctx.scm.root + "/" + matching_gni)
        if gni_content == None:
            ctx.emit.finding(
                message = "Could not read `.gni` file '%s'." % matching_gni,
                filepath = path,
                level = "error",
            )
            continue

        expected_dep = '"//%s:verify_bazel2gn"' % dir_path

        if expected_dep not in str(gni_content):
            ctx.emit.finding(
                message = "Migrated target '%s' should be added to `%s`." % (expected_dep, matching_gni),
                filepath = path,
                level = "error",
            )

# Check Item 5
def _check_sdk_fidl_list_inclusion(ctx, target_files):
    """Checks if migrated targets are added to target lists in sdk/fidl/category_lists.bzl."""

    # Get //sdk/fidl/category_lists.bzl content
    sdk_fidl_category_lists_path = "sdk/fidl/category_lists.bzl"
    category_list_content = ctx.io.read_file(ctx.scm.root + "/" + sdk_fidl_category_lists_path)
    if category_list_content == None:
        ctx.emit.finding(
            message = "Could not read //sdk/fidl/category_lists.bzl",
            filepath = sdk_fidl_category_lists_path,
            level = "error",
        )
        return

    # Apply the check to the libraries in current CL with both BUILD.bazel and BUILD.gn.
    # Exclude //sdk/fidl/BUILD.bazel and //sdk/fidl/BUILD.gn.
    for file_info in target_files:
        path = file_info.path
        bazel_targets = file_info.bazel_targets

        dir_path = _get_dir_path(path)
        if not dir_path:
            ctx.emit.finding(
                message = "Could not determine directory path for file.",
                filepath = path,
                level = "error",
            )
            continue

        if not bazel_targets:
            ctx.emit.finding(
                message = "Could not find any fidl_library targets in BUILD.bazel.",
                filepath = path,
                level = "error",
            )
            continue

        # Go through all the fidl_library targets in BUILD.bazel.
        for target_name, attrs in bazel_targets.items():
            category = attrs.get("category")
            stable = attrs.get("stable")

            # Skip if both `category` and `stable` are not specified.
            if category == None and stable == None:
                continue

            # Check if both `category` and `stable` are specified together.
            if (category == None) != (stable == None):
                ctx.emit.finding(
                    message = "Both `category` and `stable` must be specified together in BUILD.bazel for target '%s'." % target_name,
                    filepath = path,
                    level = "error",
                )
                continue

            # Identify the invalid condition.
            if stable == "false" and category != "partner":
                ctx.emit.finding(
                    message = "`stable` must be true unless the category is 'partner' in BUILD.bazel for target '%s'." % target_name,
                    filepath = path,
                    level = "error",
                )
                continue

            # Determine the list name based on `category` and `stable` attributes.
            list_name = ""
            if category == "partner":
                if stable == "true":
                    list_name = "PARTNER_IDK_STABLE_FIDL_LIBRARY_ATOMS_LIST"
                else:
                    list_name = "PARTNER_IDK_UNSTABLE_FIDL_LIBRARY_ATOMS_LIST"
            elif category == "prebuilt":
                list_name = "PREBUILT_FIDL_LIBRARY_ATOMS_LIST"
            elif category == "host_tool":
                list_name = "HOST_TOOL_FIDL_LIBRARY_ATOMS_LIST"
            elif category == "compat_test":
                list_name = "COMPAT_TEST_FIDL_LIBRARY_ATOMS_LIST"

            if list_name:
                list_block = _extract_bazel_target_list_block(str(category_list_content), list_name)

                expected_target = '"//%s:%s_idk"' % (dir_path, target_name)

                if not list_block or expected_target not in list_block:
                    ctx.emit.finding(
                        message = "Migrated target '%s' should be added to `%s` in `sdk/fidl/category_lists.bzl`." % (expected_target, list_name),
                        filepath = path,
                        level = "error",
                    )

def _get_dir_path(path):
    """Extracts the directory path from a file path."""
    idx = path.rfind("/")
    return path[:idx] if idx != -1 else ""

# Remove the comments in the content.
# Comment is :
# 1. Line comment: starts with "#"
# 2. Inline comment: starts with "#" and ends with "\n"
def _remove_comments(content):
    lines = []
    for line in content.splitlines():
        if line.strip().startswith("#"):
            continue

        idx = line.find("#")
        if idx == -1:
            lines.append(line)
            continue

        # A `#` is found, exclude the `#` inside a string.
        # Because `#` is valid in a directory name, so it could be in the label string.
        in_double_quotes = False
        in_single_quotes = False
        escaped = False
        comment_idx = -1
        current_pos = 0

        for _ in range(len(line)):
            if idx == -1:
                break

            for i in range(current_pos, idx):
                char = line[i]

                # A quote just after a backslash will be treated as a normal character.
                if escaped:
                    escaped = False
                    continue

                if char == "\\":
                    escaped = True
                    continue

                # Handle quotes.
                if char == '"' and not in_single_quotes:
                    in_double_quotes = not in_double_quotes
                elif char == "'" and not in_double_quotes:
                    in_single_quotes = not in_single_quotes

            # If the `#` is not inside the quotes, it is the start of a comment.
            if not in_double_quotes and not in_single_quotes:
                comment_idx = idx
                break

            # The `#` is inside the quotes, continue to find the next `#`.
            current_pos = idx + 1
            idx = line.find("#", current_pos)

        if comment_idx != -1:
            lines.append(line[:comment_idx])
        else:
            lines.append(line)
    return "\n".join(lines)

# Extract the fidl targets and attributes from BUILD.gn.
# FIDL target name is the string in fidl("target_name").
def _extract_gn_fidl_targets_map(ctx, content):
    targets = {}
    parts = content.split('fidl("')
    for i in range(1, len(parts)):
        # Get FIDL target name
        part = parts[i]
        idx = part.find('")')

        # If `")` is not found, skip this part.
        # If no target name is found, the caller will report error.
        if idx == -1:
            continue
        target_name = part[:idx].strip()

        # Get the attributes block. 2 = length of `")`
        block_content = part[idx + 2:]
        start_idx = block_content.find("{")
        if start_idx != -1:
            block_content = block_content[start_idx + 1:]
        end_idx = block_content.find("\n}")
        if end_idx != -1:
            block_content = block_content[:end_idx]

        # Retrieve attributes and their values from the attributes block
        attrs = {}
        attr_names = _extract_attributes(ctx, block_content)
        for name in attr_names:
            attrs[name] = _extract_attr_value(ctx, block_content, name)
        targets[target_name] = attrs
    return targets

# Extract the fidl_library targets and attributes from BUILD.bazel.
def _extract_bazel_fidl_targets_map(ctx, content):
    targets = {}
    parts = content.split("fidl_library(")
    for i in range(1, len(parts)):
        part = parts[i]

        # Truncate the part to the end of the fidl_library block.
        end_idx = part.find("\n)")
        if end_idx != -1:
            part = part[:end_idx]

        matches = ctx.re.allmatches(r'name\s*=\s*"([^"]+)"', part)
        if not matches:
            matches = ctx.re.allmatches(r"name\s*=\s*'([^']+)'", part)
        if not matches:
            continue
        target_name = matches[0].groups[1].strip()

        attrs = {}
        attr_names = _extract_attributes(ctx, part)
        for name in attr_names:
            attrs[name] = _extract_attr_value(ctx, part, name)
        targets[target_name] = attrs
    return targets

def _extract_attributes(ctx, content):
    attrs = []
    for line in content.splitlines():
        matches = ctx.re.allmatches(r"^\s*([a-zA-Z_0-9]+)\s*=", line)
        if matches:
            attrs.append(matches[0].groups[1])
    return attrs

def _extract_attr_value(ctx, content, attr_name):
    # Try to find string value on a single line
    for line in content.splitlines():
        matches = ctx.re.allmatches(r"^\s*" + attr_name + r'\s*=\s*"([^"]*)"', line)
        if matches:
            return matches[0].groups[1]
        matches = ctx.re.allmatches(r"^\s*" + attr_name + r"\s*=\s*'([^']*)'", line)
        if matches:
            return matches[0].groups[1]
        matches = ctx.re.allmatches(r"^\s*" + attr_name + r"\s*=\s*(true|false|True|False)", line)
        if matches:
            val = matches[0].groups[1]
            if val == "True":
                return "true"
            if val == "False":
                return "false"
            return val

    # Try to extract list value
    lines = content.splitlines()
    for i in range(len(lines)):
        line = lines[i]
        if ctx.re.allmatches(r"^\s*" + attr_name + r"\s*=\s*\[", line):
            values = []
            matches = ctx.re.allmatches(r'["\']([^"\']+)["\']', line)
            for m in matches:
                values.append(m.groups[1])

            if "]" in line:
                return values

            for j in range(i + 1, len(lines)):
                next_line = lines[j]
                if "]" in next_line:
                    idx = next_line.find("]")
                    part = next_line[:idx]
                    matches = ctx.re.allmatches(r'["\']([^"\']+)["\']', part)
                    for m in matches:
                        values.append(m.groups[1])
                    return values
                else:
                    matches = ctx.re.allmatches(r'["\']([^"\']+)["\']', next_line)
                    for m in matches:
                        values.append(m.groups[1])
            return values
    return None

def _map_gn_visibility_to_bazel(item):
    if item == "*":
        return "//visibility:public"
    if item == ":*":
        return "//visibility:private"
    if item.endswith("/*"):
        return item[:-2] + ":__subpackages__"
    if item.endswith(":*"):
        return item[:-2] + ":__pkg__"
    return item

# Checks if the visibility attributes in BUILD.bazel and BUILD.gn are equivalent.
# FIDL libraries under //sdk/fidl/ should have visibility attribute.
def _check_visibility(ctx, path, target, bazel_vis, gn_vis):
    if not bazel_vis:
        ctx.emit.finding(
            message = "visibility attribute missing in BUILD.bazel for target `%s`" % target,
            filepath = path,
            level = "error",
        )
    if not gn_vis:
        ctx.emit.finding(
            message = "visibility attribute missing in BUILD.gn for target `%s`" % target,
            filepath = path,
            level = "error",
        )

    if bazel_vis and gn_vis:
        mapped_gn_visibility = []
        for v in gn_vis:
            mapped_gn_visibility.append(_map_gn_visibility_to_bazel(v))

        if sorted(bazel_vis) != sorted(mapped_gn_visibility):
            ctx.emit.finding(
                message = "Visibility in BUILD.bazel for target '%s' does not match mapped visibility from BUILD.gn.\n" % target +
                          "BUILD.bazel: %s\n" % bazel_vis +
                          "BUILD.gn (mapped): %s" % mapped_gn_visibility,
                filepath = path,
                level = "error",
            )

def _normalize_dep(dep):
    if ":" in dep:
        idx = dep.find(":")
        dir_part = dep[:idx]
        target_part = dep[idx + 1:]

        dir_idx = dir_part.rfind("/")
        dir_leaf = dir_part[dir_idx + 1:] if dir_idx != -1 else dir_part

        if target_part == dir_leaf:
            return dir_part
    return dep

def _extract_bazel_target_list_block(content, list_name):
    # Try variable assignment list_name = [
    idx = content.find(list_name + " = [")
    if idx != -1:
        end_idx = content.find("]", idx)
        if end_idx != -1:
            return content[idx:end_idx]

    return ""

# Files that should not be checked for the reasons specified.
_BAZEL_BUILD_PATHS_TO_IGNORE = [
    # There is no fidl library in this file.
    "sdk/fidl/BUILD.bazel",

    # These files currently use Bazel SDK macros and thus fail these checks.
    # TODO(https://fxbug.dev/493687765): Remove each entry as the use of the Bazel
    # SDK is removed from it.
    "sdk/fidl/fuchsia.boot/BUILD.bazel",
    "sdk/fidl/fuchsia.driver.compat/BUILD.bazel",
    "sdk/fidl/fuchsia.hardware.clock.measure/BUILD.bazel",
    "sdk/fidl/fuchsia.hardware.qcom.hvdcpopti/BUILD.bazel",
    "sdk/fidl/fuchsia.hardware.sockettunnel/BUILD.bazel",
    "sdk/fidl/fuchsia.power.battery/BUILD.bazel",

    # TODO(https://fxbug.dev/454449781): Enable this check when bazel2gn supports zither libraries.
    "sdk/fidl/zbi/BUILD.bazel",
]

# The potential target file meets the criteria:
# 1. It is a BUILD.bazel file.
# 2. It is newly added or modified.
# 3. It has a corresponding BUILD.gn file in the same directory.
def _is_target_bazel_build_file(ctx, path, meta):
    """Returns True if the file is a newly added or modified FIDL BUILD.bazel file and the BUILD.gn is also in the directory."""

    # TODO(https://fxbug.dev/487883318): Extend the check to FIDL libraries outside //sdk/fidl directory.
    if not path.startswith("sdk/fidl/"):
        return False

    # Only process ADDED or MODIFIED files.
    if meta.action not in ["A", "M"]:
        return False

    # Only process BUILD.bazel files to avoid duplicate checks.
    if not path.endswith("BUILD.bazel"):
        return False

    # Skip ignored BUILD.bazel files.
    if path in _BAZEL_BUILD_PATHS_TO_IGNORE:
        return False

    # Check if corresponding BUILD.gn file exists in the same directory.
    gn_path = path.replace("BUILD.bazel", "BUILD.gn")
    if not ctx.scm.all_files(glob = gn_path):
        return False

    return True

def _fidl_gn2bazel_migration_check(ctx):
    """Main check for FIDL migration from GN to Bazel."""

    target_files = []

    for path, meta in ctx.scm.affected_files().items():
        if not _is_target_bazel_build_file(ctx, path, meta):
            continue

        # Get fidl targets and attributes from BUILD.bazel.
        # Skip the checks if it's not a fidl library.
        orig_bazel_content = ctx.io.read_file(ctx.scm.root + "/" + path)
        bazel_content = _remove_comments(str(orig_bazel_content))
        bazel_targets = _extract_bazel_fidl_targets_map(ctx, bazel_content)
        if not bazel_targets:
            continue

        if meta.action == "A":
            _check_copyright_year(ctx, path, str(orig_bazel_content))

        # Get fidl targets and attributes from BUILD.gn.
        gn_path = path.replace("BUILD.bazel", "BUILD.gn")
        gn_content = ctx.io.read_file(ctx.scm.root + "/" + gn_path)
        gn_content = _remove_comments(str(gn_content))
        gn_targets = _extract_gn_fidl_targets_map(ctx, gn_content)

        # Because the BUILD.bazel has the fidl_library() definition,
        # we expect the BUILD.gn to have the fidl() definition. If not, it's an error.
        if not gn_targets:
            ctx.emit.finding(
                message = "BUILD.bazel has fidl_library() but BUILD.gn lacks fidl().",
                filepath = path,
                level = "error",
            )
            continue

        target_files.append(struct(
            path = path,
            bazel_content = bazel_content,
            gn_content = gn_content,
            bazel_targets = bazel_targets,
            gn_targets = gn_targets,
        ))

    if not target_files:
        return

    _check_build_files(ctx, target_files)
    _check_bazel2gn_verification_inclusion(ctx, target_files)
    _check_sdk_fidl_list_inclusion(ctx, target_files)

def register_bazel_migration_fidl_checks():
    shac.register_check(shac.check(_fidl_gn2bazel_migration_check))
