# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for collecting licenses for a given target and its transitive closure.

How it works
============
1. The `fuchsia_licenses_collection` rule depends on the `_collect_licenses` aspect.
2. The aspect visits all targets in the build graph and collects:
   1. `LicenseInfo` providers from the 'applicable_licenses' attribute.
   2. `_UnlicensedTargetInfo` providers for each dependent target that should have a license based.
3. The aspect recursively aggregates these providers all the way up to the `root_target`
   that is under analysis. However, it ignores dependencies through rules, attributes, and other
   targets that match `ignore_policy` from collection_policy.bzl. It also ignores tool dependencies.
4. The results for `root_target` are serialized into JSON and passed to verify_licenses_collection.py.
5. verify_licenses_collection.py performs further analysis and may issue build failures if licenses
   are missing.
6. The verified JSON is passed as an output, which is then read by `fuchsia_licenses_spdx`.
"""

load(
    "@rules_license//rules:providers.bzl",
    "LicenseInfo",
)
load("collection_policy.bzl", "ignore_policy", "is_3p_target")
load("common.bzl", "bool_dict", "check_is_target", "is_same_package", "is_target", "to_file_path", "to_label_str", "to_package_str")
load("providers.bzl", "LicensesCollectionInfo")

_VisitedTargetInfo = provider(
    doc = "Information about a target visited by the collecting aspect",
    fields = {
        "is_ignored": "Whether the target was ignored",
        "license_infos": "depset of LicenseInfo instances",
        "unlicensed_target_infos": "depset of _UnlicensedTargetInfo instances",
    },
)

_UnlicensedTargetInfo = provider(
    doc = "Target without associated license",
    fields = {
        "label": "Target's Label object",
        "build_file_path": "The build file where the target resides",
        "rule_kind": "The target's rule kind (e.g. `cc_library`)",
        "unlicensed_resources": "The Depset of the resources that need licenses",
        "rule_attr_names": "The names of the rule attributes where the resources are referenced",
    },
)

def _is_target_in_ignore_policy(target):
    check_is_target(target)
    label = target.label
    if label.workspace_name in ignore_policy.workspaces:
        return True
    label_str = to_label_str(label)
    if label_str in ignore_policy.targets:
        return True
    package_str = to_package_str(label)
    if package_str in ignore_policy.packages:
        return True
    return False

def _is_rule_in_ignore_policy(rule):
    return rule.kind in ignore_policy.rules

def _does_target_need_license(target, optional_rule = None):
    check_is_target(target)
    if _is_target_in_ignore_policy(target):
        return False
    elif optional_rule and _is_rule_in_ignore_policy(optional_rule):
        return False
    elif is_3p_target(target):
        return True
    else:
        return False

def _does_target_need_visiting(target, optional_rule = None):
    check_is_target(target)
    if _is_target_in_ignore_policy(target):
        return False
    elif optional_rule and _is_rule_in_ignore_policy(optional_rule):
        return False
    else:
        return True

_APPLICABLE_LICENSES_ATTR = "applicable_licenses"  # Bazel <= 7.x
_PACKAGE_METADATA_ATTR = "package_metadata"  # Bazel >= 8.x

def _visit_target(target, ctx):
    if not _does_target_need_visiting(target, optional_rule = ctx.rule):
        return [_VisitedTargetInfo(
            is_ignored = True,
        )]

    license_infos = []
    unlicensed_target_infos = []
    transitive_providers = []

    has_applicable_licenses = False
    for license_dep in getattr(ctx.rule.attr, _APPLICABLE_LICENSES_ATTR, []):
        # This target has applicable_licenses. Lets collect the licenses.
        has_applicable_licenses = True
        if LicenseInfo in license_dep:
            license_infos.append(license_dep[LicenseInfo])
        else:
            # applicable_licenses must reference a `license` target, which is a provider of LicenseInfo.
            fail("No LicenseInfo provided for %s. Is this target a `license` target?" % license_dep)

    for metadata_dep in getattr(ctx.rule.attr, _PACKAGE_METADATA_ATTR, []):
        if LicenseInfo in metadata_dep:
            has_applicable_licenses = True
            license_infos.append(metadata_dep[LicenseInfo])

    target_needs_license = not has_applicable_licenses and _does_target_need_license(target, optional_rule = ctx.rule)

    unlicensed_resources = []
    unlicensed_resources_attributes = {}

    def visit_dependency(attr_name, dep):
        if not is_target(dep):
            return

        if _VisitedTargetInfo in dep:
            transitive_provider = dep[_VisitedTargetInfo]
            if not transitive_provider.is_ignored:
                transitive_providers.append(transitive_provider)
        else:
            # Aspects don't visit local files (same package) or files used via exports_files
            # (See https://github.com/bazelbuild/rules_license/issues/107)
            if not is_same_package(target, dep):
                dep_needs_licenses = not has_applicable_licenses and _does_target_need_license(dep)
                if dep_needs_licenses:
                    unlicensed_resources.append(dep)
                    if attr_name not in unlicensed_resources_attributes:
                        unlicensed_resources_attributes[attr_name] = True

    def visit_attributes(attribute_names):
        ignore_attrs_by_rule = ignore_policy.rule_attributes[ctx.rule.kind] if ctx.rule.kind in ignore_policy.rule_attributes else []
        ignore_attrs = bool_dict(ignore_attrs_by_rule + [_APPLICABLE_LICENSES_ATTR, _PACKAGE_METADATA_ATTR] + [e for e in dir(ctx.rule.executable)])

        for attr_name in attribute_names:
            if attr_name in ignore_attrs:
                continue
            attr_value = getattr(ctx.rule.attr, attr_name)
            if type(attr_value) == type([]):
                # Handle label_list attributes:
                for v in attr_value:
                    visit_dependency(attr_name, v)
            elif type(attr_value) == type(dict):
                # Handle label_keyed_string_dict:
                for k in attr_value.keys():
                    visit_dependency(attr_name, k)
            else:
                # label attribute
                visit_dependency(attr_name, attr_value)

    visit_attributes(dir(ctx.rule.attr))

    if target_needs_license or unlicensed_resources:
        unlicensed_target_infos.append(_UnlicensedTargetInfo(
            label = target.label,
            rule_kind = ctx.rule.kind,
            build_file_path = ctx.build_file_path,
            unlicensed_resources = depset(unlicensed_resources) if not target_needs_license else None,
            rule_attr_names = ",".join(sorted(unlicensed_resources_attributes.keys())) if not target_needs_license else None,
        ))

    return [_VisitedTargetInfo(
        is_ignored = False,
        license_infos = depset(license_infos, transitive = [p.license_infos for p in transitive_providers]),
        unlicensed_target_infos = depset(unlicensed_target_infos, transitive = [p.unlicensed_target_infos for p in transitive_providers]),
    )]

_collect_licenses = aspect(
    doc = "Aspect for analyzing the build graph for license information",
    implementation = _visit_target,
    attr_aspects = ["*"],  # Traverse all graph edges.
    attrs = {},
    provides = [_VisitedTargetInfo],
    apply_to_generating_rules = True,
)

def _write_licenses_collection_json(ctx, gathered_licenses_info, output_file):
    licenses = []
    unlicensed_targets = []
    root_target_str = "%s" % ctx.attr.root_target.label

    if not gathered_licenses_info.is_ignored:
        for i in gathered_licenses_info.license_infos.to_list():
            licenses.append({
                "package_name": i.package_name,
                "package_url": i.package_url,
                "license_text": to_file_path(i.license_text),
                "copyright_notice": i.copyright_notice,
            })

        for i in gathered_licenses_info.unlicensed_target_infos.to_list():
            unlicensed_target = {
                "label": to_label_str(i.label),
                "build_file_path": i.build_file_path,
                "rule_kind": i.rule_kind,
            }
            if i.unlicensed_resources:
                unlicensed_target.update({
                    "unlicensed_resources": [to_label_str(r) for r in i.unlicensed_resources.to_list()],
                    "rule_attr_names": i.rule_attr_names.split(","),
                })
            unlicensed_targets.append(unlicensed_target)

    # buildifier: disable=attr-licenses
    json_content = {
        "root_target": root_target_str,
        "licenses": licenses,
        "unlicensed_targets": unlicensed_targets,
    }
    ctx.actions.write(output_file, content = json.encode_indent(json_content))

def _fuchsia_licenses_collection_impl(ctx):
    garthered_licenses_info = ctx.attr.root_target[_VisitedTargetInfo]

    licenses_collection_file = ctx.actions.declare_file("%s.licenses_collection.json" % ctx.attr.name)
    _write_licenses_collection_json(ctx, garthered_licenses_info, licenses_collection_file)

    verified_licenses_collection_file = ctx.actions.declare_file("%s.verified_licenses_collection.json" % ctx.attr.name)

    # Verification is performed as an action (Execution phase) rather than in Starlark (Analysis phase).
    # Otherwise, any verification failure prevents Bazel graph analysis from successful completion,
    # severely impacting infra integration and usability (e.g. can't even run `bazel query`).
    ctx.actions.run(
        progress_message = "Verifying license collection %s" % licenses_collection_file.path,
        inputs = [licenses_collection_file],
        outputs = [verified_licenses_collection_file],
        mnemonic = "FuchsiaLicensesCollectionVerify",
        executable = ctx.executable._verify_collection_tool,
        arguments = [
            "--licenses_collection_input=%s" % licenses_collection_file.path,
            "--verified_licenses_collection_output=%s" % verified_licenses_collection_file.path,
        ],
    )

    return [LicensesCollectionInfo(
        json_file = verified_licenses_collection_file,
        license_files = depset([i.license_text for i in garthered_licenses_info.license_infos.to_list()]),
    )]

fuchsia_licenses_collection = rule(
    doc = """
Collects all licenses in the transitive closure of `root_target`,
while making sure that all prebuilts and third-party dependencies
have associated licenses. Tool dependencies are ignored.
""",
    implementation = _fuchsia_licenses_collection_impl,
    provides = [LicensesCollectionInfo],
    attrs = {
        "root_target": attr.label(
            doc = "The target to collect the licenses from, including its transitive dependencies.",
            mandatory = True,
            aspects = [_collect_licenses],
        ),
        "_verify_collection_tool": attr.label(
            executable = True,
            cfg = "exec",
            default = "//fuchsia/tools/licenses:verify_licenses_collection",
        ),
    },
)
