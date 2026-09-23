#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import collections
import copy
import datetime
import enum
import functools
import hashlib
import json
import os
import re
import shutil
import sys
import textwrap
import tomllib

CARGO_PACKAGE_CONTENTS = """\
# Copyright %(year)s The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# source GN: %(target)s"

cargo-features = ["per-package-target"]

[package]
name = "%(package_name)s"
version = "%(version)s"
license = "BSD-3-Clause"
authors = ["rust-fuchsia@fuchsia.com"]
description = "Rust crate for Fuchsia"
repository = "https://fuchsia.googlesource.com"
edition = "%(edition)s"
%(default_target)s

%(bin_or_lib)s
%(is_proc_macro)s
name = "%(crate_name)s"
path = "%(source_root)s"
"""

CARGO_PACKAGE_NO_WORKSPACE = """\
[workspace]
# empty workspace table excludes this crate from thinking it should be in a workspace
"""

CARGO_PACKAGE_DEP = """\
[%(dep_type)s.%(crate_name)s]
version = "%(version)s"
path = "%(crate_path)s"

"""


class ToolchainType(enum.Enum):
    TARGET = 1
    HOST = 2


def strip_toolchain(target):
    return re.search("[^(]*", target)[0]


def extract_toolchain(target):
    """Return the toolchain part of the provided label, or None if it doesn't have one."""
    if "(" not in target:
        # target has no toolchain specified
        return None
    substr = target[target.find("(") + 1 :]
    if not target.endswith(")"):
        raise ValueError("target %s missing closing `)`")
    return substr[:-1]  # remove closing `)`


def version_from_toolchain(toolchain):
    """Return a version to use to allow host and target crates to coexist."""
    version = "0.0.1"
    if toolchain != None and toolchain.startswith("//build/toolchain:host_"):
        version = "0.0.2"
    return version


def classify_toolchain(toolchain):
    if toolchain != None and toolchain.startswith("//build/toolchain:host_"):
        return ToolchainType.HOST
    else:
        return ToolchainType.TARGET


def lookup_gn_pkg_name(project, target, *, for_workspace):
    if for_workspace:
        return mangle_label(target)
    metadata = project.targets[target]
    return metadata["output_name"]


def rebase_gn_path(root_path, location, directory=False):
    assert location[0:2] == "//"
    # remove the prefix //
    path = location[2:]
    target = os.path.dirname(path) if directory else path
    return os.path.join(root_path, target)


def mangle_label(label):
    assert label[0:2] == "//"
    # remove the prefix //
    label = label[2:]
    result = []
    for c in label:
        if c == "-":
            result.append("--")
        elif c == "_":
            result.append("__")
        elif c == "/":
            result.append("_-_")
        elif c == ":":
            result.append("-_-")
        elif c == ".":
            result.append("_--")
        elif c == "(" or c == ")":
            result.append("-__")
        else:
            result.append(c)
    return "".join(result)


class Project(object):
    def __init__(self, project_json):
        self.targets = project_json["targets"]
        self.patches = None

    @functools.cached_property
    def rust_targets(self):
        return {
            target: meta
            for target, meta in self.targets.items()
            if "crate_root" in meta
        }

    @functools.cached_property
    def rust_targets_by_source_root(self):
        result = collections.defaultdict(list)
        for target, meta in self.rust_targets.items():
            source_root = meta["crate_root"]
            result[source_root].append(target)
        return dict(result)

    @functools.cached_property
    def reachable_targets(self):
        result = set(["//:default"])
        pending = ["//:default"]
        while pending:
            current = pending.pop()
            meta = self.targets[current]
            for dep_kind in ["deps", "public_deps", "data_deps"]:
                for dep in meta.get(dep_kind, []):
                    if dep not in result:
                        result.add(dep)
                        pending.append(dep)

        return result

    def expand_source_set_or_group(self, target):
        """Returns a list of dependencies if the target is a source_set.

        Returns dependencies as a list of strings if the target is a
        source_set, or None otherwise.
        """
        meta = self.targets[target]
        if meta["type"] in ("source_set", "group"):
            return meta["deps"]

    def find_test_targets(self, source_root):
        overlapping_targets = self.rust_targets_by_source_root.get(
            source_root, []
        )
        return [
            t
            for t in overlapping_targets
            if "--test" in self.targets[t]["rustflags"]
        ]


def get_features(rustflags):
    features = []
    feature_pat = re.compile(r"--cfg=feature=\"(.*)\"$")
    for flag in rustflags:
        if match := feature_pat.match(flag):
            features.append(match.group(1))
    return features


def get_check_cfgs(rustflags):
    check_cfg = []
    check_cfg_pat = re.compile(r"--check-cfg=(.*)$")
    for flag in rustflags:
        if match := check_cfg_pat.match(flag):
            check_cfg.append(match.group(1))
    return check_cfg


def get_cfgs(rustflags: list[str], api_level_cfgs: list[str]) -> list[str]:
    cfgs: list[str] = []
    cfg_pat = re.compile(r"--cfg=([^=]*)=(.*)$")
    for flag in rustflags:
        if flag.startswith("--cfg=feature"):
            continue
        if match := cfg_pat.match(flag):
            # __rust_toolchain is for a cfg that's used to invalidate old
            # toolchain versions by changing the ninja command line, which is
            # of no use to cargo.
            if match.group(1) != "__rust_toolchain":
                cfgs.append(f"{match.group(1)}={match.group(2)}")
        elif flag == "@rust_api_level_cfg_flags.txt":
            # This is a special response file used to provide all the api level
            # cfg flags.  We've already read that file, so insert those directly
            # into the cfgs.
            cfgs.extend(api_level_cfgs)
        elif flag.startswith("@"):
            try:
                with open(flag[1:]) as f:
                    for line in f:
                        if line.startswith("--cfg="):
                            cfgs.append(line[len("--cfg=") :])
            except FileNotFoundError:
                print(f"Warning: Could not find response file {flag[1:]}")
        elif flag.startswith("--cfg="):
            cfgs.append(flag[len("--cfg=") :])
    return cfgs


def write_toml_file(
    fout,
    metadata,
    project,
    target,
    lookup,
    root_path,
    root_build_dir,
    gn_cargo_dir,
    for_workspace,
    version,
    api_level_cfgs: list[str],
):
    rust_crates_path = os.path.join(root_path, "third_party/rust_crates")

    editions = [
        flag.split("=")[1]
        for flag in metadata["rustflags"]
        if flag.startswith("--edition=")
    ]
    edition = editions[0] if editions else "2015"

    if metadata["type"] in [
        "rust_library",
        "rust_proc_macro",
        "static_library",
        "shared_library",
    ]:
        target_type = "[lib]"
    else:
        if "--test" in metadata["rustflags"]:
            target_type = "[[test]]"
        else:
            target_type = "[[bin]]"

    if metadata["type"] == "rust_proc_macro":
        is_proc_macro = "proc-macro = true"
    else:
        is_proc_macro = ""

    features = get_features(metadata["rustflags"])
    extra_configs = get_cfgs(metadata["rustflags"], api_level_cfgs)
    check_cfgs = get_check_cfgs(metadata["rustflags"])

    crate_type = "rlib"
    package_name = lookup_gn_pkg_name(
        project, target, for_workspace=for_workspace
    )

    default_target = ""
    if classify_toolchain(extract_toolchain(target)) == ToolchainType.TARGET:
        default_target = 'default-target = "x86_64-unknown-fuchsia"'

    fout.write(
        CARGO_PACKAGE_CONTENTS
        % {
            "target": target,
            "package_name": package_name,
            "crate_name": metadata["crate_name"],
            "version": version,
            "year": datetime.datetime.now().year,
            "bin_or_lib": target_type,
            "is_proc_macro": is_proc_macro,
            "lib_crate_type": crate_type,
            "edition": edition,
            "source_root": rebase_gn_path(root_path, metadata["crate_root"]),
            "rust_crates_path": rust_crates_path,
            "default_target": default_target,
        }
    )

    env_vars = metadata.get("rustenv", [])
    if extra_configs or env_vars:
        with open(
            os.path.join(gn_cargo_dir, str(lookup[target]), "build.rs"), "w"
        ) as buildfile:
            template = textwrap.dedent(
                """\
                //! build script for {target}
                fn main() {{
                // build script does not read any files
                println!("cargo:rerun-if-changed=build.rs");

                {body}
                {env_vars}
                }}
            """
            )
            body = "\n".join(
                f'println!(r#"cargo:rustc-cfg={cfg}"#);'
                for cfg in extra_configs
            )
            env_vars = "\n".join(
                f'println!("cargo:rustc-env={env}");' for env in env_vars
            )
            buildfile.write(
                template.format(target=target, body=body, env_vars=env_vars)
            )

    extra_test_deps = set()
    if target_type in {"[lib]", "[[bin]]"}:
        test_targets = project.find_test_targets(metadata["crate_root"])
        # hack to filter to just matching toolchains:
        test_targets = [
            t for t in test_targets if t.split("(")[1:] == target.split("(")[1:]
        ]

        test_deps = set()
        for test_target in test_targets:
            test_deps.update(project.targets[test_target]["deps"])

        unreachable_test_deps = sorted(
            [dep for dep in test_deps if dep not in project.reachable_targets]
        )
        if unreachable_test_deps:
            fout.write(
                "# Note: disabling tests because test deps are not included in the build: %s\n"
                % unreachable_test_deps
            )
            fout.write("test = false\n")
        elif not test_targets:
            fout.write(
                "# Note: disabling tests because no test target was found with the same source root\n"
            )
            fout.write("test = false\n")
        else:
            fout.write(
                "# Note: using extra deps from discovered test target(s): %s\n"
                % test_targets
            )
            extra_test_deps = sorted(test_deps - set(metadata["deps"]))

    if not for_workspace:
        fout.write(CARGO_PACKAGE_NO_WORKSPACE)

    if not for_workspace:
        # In a workspace, patches are ignored, so we skip emitting all the patch lines to cut down on warning spam
        fout.write("\n[patch.crates-io]\n")
        for patch in project.patches:
            path = project.patches[patch]["path"]
            fout.write(
                '%s = { path = "%s/%s"' % (patch, rust_crates_path, path)
            )
            if package := project.patches[patch].get("package"):
                fout.write(', package = "%s"' % (package,))
            fout.write(" }\n")
        fout.write("\n")

    def expand_and_deduplicate(deps, visited=None):
        if visited is None:
            visited = set()
        for dep in deps:
            if dep in visited:
                continue
            visited.add(dep)
            expanded = project.expand_source_set_or_group(dep)
            if expanded:
                for exp in expand_and_deduplicate(expanded, visited):
                    yield exp
            else:
                yield dep

    # collect all dependencies
    deps = list(expand_and_deduplicate(metadata["deps"]))

    dep_crate_names = set()

    def write_deps(deps, dep_type):
        while deps:
            dep = deps.pop()

            # If a dependency points to a source set or group, expand it into a list
            # of its deps, and append them to the deps list. Finally, continue
            # to the next item, since a source set itself is not considered a
            # dependency for our purposes.
            expanded_deps = project.expand_source_set_or_group(dep)
            if expanded_deps:
                deps.extend(expanded_deps)
                continue

            # ignore non-rust deps
            if "crate_name" not in project.targets[dep]:
                continue

            # this is a third-party dependency
            # TODO remove this when all things use GN. temporary hack?
            if "third_party/rust_crates:" in dep:
                match = re.search(r"rust_crates:([\w-]*)", dep)
                crate_name, version = str(match.group(1)).rsplit("-v", 1)
                if crate_name in dep_crate_names:
                    # Don't add the same crate twice. Can happen with many
                    # versions of the same crate declared with different
                    # features.
                    continue
                # Remove the trailing suffix from proc-macro crates
                version = version.removesuffix("_proc_macro")
                dep_crate_names.add(crate_name)
                version = version.replace("_", ".")
                fout.write('[%s."%s"]\n' % (dep_type, crate_name))
                fout.write('version = "%s"\n' % version)
                fout.write("default-features = false\n")
                if dep_features := get_features(
                    project.targets[dep]["rustflags"]
                ):
                    # Filter out features that break our unusual build until they're properly fixed
                    # upstream
                    # TODO(376501054): fix ahash upstream and then remove this.
                    if crate_name == "ahash":
                        dep_features = [
                            f for f in dep_features if f != "folded_multiply"
                        ]
                    fout.write("features = %s\n" % json.dumps(dep_features))
                if crate_name in features:
                    # Make the dependency optional if there is a feature with
                    # the same name. Later, we'll make sure to list the feature
                    # in the default feature list so that we do actually include
                    # the dependency.
                    fout.write("optional = true\n")
            # this is a in-tree rust target
            else:
                toolchain = extract_toolchain(dep)
                version = version_from_toolchain(toolchain)
                crate_name = lookup_gn_pkg_name(
                    project, dep, for_workspace=for_workspace
                )
                if crate_name in dep_crate_names:
                    # Don't add the same crate twice. Can happen with many
                    # versions of the same crate declared with different
                    # features.
                    continue
                dep_crate_names.add(crate_name)
                dep_dir = os.path.join(gn_cargo_dir, str(lookup[dep]))
                fout.write(
                    CARGO_PACKAGE_DEP
                    % {
                        "dep_type": dep_type,
                        "crate_path": dep_dir,
                        "crate_name": crate_name,
                        "version": version,
                    }
                )

    write_deps(deps, "dependencies")
    write_deps(extra_test_deps, "dev-dependencies")

    if features:
        fout.write("\n[features]\n")
        # Filter 'default' feature out to avoid generating a duplicated entry.
        features = [x for x in features if x != "default"]
        fout.write("default = %s\n" % json.dumps(features))

        for feature in features:
            # Filter features that are also dependencies
            # https://users.rust-lang.org/t/features-and-dependencies-cannot-have-the-same-name/47746/2
            if feature not in dep_crate_names:
                fout.write("%s = []\n" % feature)
    if not for_workspace:
        if check_cfgs:
            fout.write("\n[lints.rust]\n")
            fout.write('unexpected_cfgs = { level = "warn", check-cfg = [')
            for check_cfg in check_cfgs:
                fout.write("'%s'," % check_cfg)
            fout.write("] }\n")
        else:
            # Disable check-cfg in cargo if not available to avoid the noise.
            fout.write("\n[lints.rust]\n")
            fout.write('unexpected_cfgs = { level = "allow" }\n')


def main():
    # TODO(tmandry): Remove all hardcoded paths and replace with args.
    parser = argparse.ArgumentParser()
    parser.add_argument("--root_build_dir", required=True)
    parser.add_argument("--fuchsia_dir", required=True)
    parser.add_argument(
        "--api_level_cfg_flags",
        required=True,
        help="Path to rust_api_level_cfg_flags.txt response file",
    )
    parser.add_argument("json_path")
    args = parser.parse_args()

    json_path = args.json_path
    root_path = os.path.abspath(args.fuchsia_dir)
    root_build_dir = os.path.abspath(args.root_build_dir)
    gn_cargo_dir = os.path.join(root_build_dir, "cargo")
    rust_crates_path = os.path.join(root_path, "third_party/rust_crates")

    # remove the previously generated rust crates
    shutil.rmtree(gn_cargo_dir, ignore_errors=True)
    os.makedirs(gn_cargo_dir)

    # unconditionally write a stamp to prevent GN from re-running this action
    with open(os.path.join(gn_cargo_dir, "generate_cargo.stamp"), "w") as f:
        f.truncate()

    try:
        with open(json_path, "r") as json_file:
            project = Project(json.loads(json_file.read()))
    except (IOError, json.decoder.JSONDecodeError) as err:
        print("Failed to generate Cargo.toml files")
        print("No project.json in the root of your out directory!")
        print("Run gn with the --ide=json flag set")
        print(f"Caused by: Could not parse file {json_path}: {err}")
        # returns 0 so that CQ doesn't fail if this isn't set properly
        return 0

    # this will be removed eventually?
    with open(rust_crates_path + "/Cargo.toml", "rb") as f:
        project.patches = tomllib.load(f)["patch"]["crates-io"]

    api_level_cfgs: list[str] = []
    with open(args.api_level_cfg_flags) as f:
        for line in f:
            if line.startswith("--cfg="):
                api_level_cfgs.append(line[len("--cfg=") :])

    lookup = {}
    for target in project.rust_targets:
        # hash is the GN target name without the prefixed //
        lookup[target] = hashlib.sha1(target[2:].encode("utf-8")).hexdigest()

    # a dict of "toolchain label" to list of Cargo.toml files in it
    # special case: the key None means the default toolchain
    workspace_dirs_by_toolchain = collections.defaultdict(list)

    for target in project.rust_targets:
        toolchain = extract_toolchain(target)
        version = version_from_toolchain(toolchain)
        cargo_toml_dir = os.path.join(gn_cargo_dir, str(lookup[target]))
        try:
            os.makedirs(cargo_toml_dir)
        except OSError:
            print("Failed to create directory for Cargo: %s" % cargo_toml_dir)

        for_workspace_cargo_toml_dir = os.path.join(
            gn_cargo_dir, "for_workspace", str(lookup[target])
        )
        try:
            os.makedirs(for_workspace_cargo_toml_dir)
        except OSError:
            print(
                "Failed to create directory for Cargo: %s"
                % for_workspace_cargo_toml_dir
            )

        metadata = project.targets[target]
        with open(os.path.join(cargo_toml_dir, "Cargo.toml"), "w") as fout:
            write_toml_file(
                fout,
                metadata,
                project,
                target,
                lookup,
                root_path,
                root_build_dir,
                gn_cargo_dir,
                for_workspace=False,
                version=version,
                api_level_cfgs=api_level_cfgs,
            )

        if (
            not target.startswith("//third_party/rust_crates:")
        ) and target in project.reachable_targets:
            workspace_dirs_by_toolchain[toolchain].append(
                (
                    target,
                    os.path.relpath(for_workspace_cargo_toml_dir, root_path),
                )
            )
            with open(
                os.path.join(for_workspace_cargo_toml_dir, "Cargo.toml"), "w"
            ) as fout:
                write_toml_file(
                    fout,
                    metadata,
                    project,
                    target,
                    lookup,
                    root_path,
                    root_build_dir,
                    os.path.join(gn_cargo_dir, "for_workspace"),
                    for_workspace=True,
                    version=version,
                    api_level_cfgs=api_level_cfgs,
                )

    # TODO: refactor into separate function
    for toolchain, workspace_dirs in workspace_dirs_by_toolchain.items():
        subdir = os.path.join(gn_cargo_dir, "for_workspace")
        if toolchain:
            # Strip off the leading "//" from the toolchain label so we don't
            # accidentally use it as an absolute path.
            path_safe_toolchain = toolchain.lstrip("/")
            subdir = os.path.join(subdir, "toolchain", path_safe_toolchain)
        else:
            # the workspace for the default toolchain (None in the dict) just
            # lives in for_workspace directly.
            pass

        try:
            os.makedirs(subdir, exist_ok=True)
        except OSError:
            print("Failed to create directory for Cargo: %s" % subdir)

        with open(
            os.path.join(subdir, "Cargo_for_fuchsia_dir.toml"), "w"
        ) as fout:
            fout.write("[workspace]\nmembers = [\n")
            for target, dir in workspace_dirs:
                fout.write("  # %s\n" % target)
                fout.write("  %s,\n" % json.dumps(dir))
            fout.write("]\n")

            fout.write('exclude = ["third_party/rust_crates",]\n')
            fout.write("\n[patch.crates-io]\n")
            for patch in project.patches:
                path = project.patches[patch]["path"]
                fout.write(
                    "%s = { path = %s"
                    % (
                        patch,
                        json.dumps(
                            os.path.join("third_party/rust_crates", path)
                        ),
                    )
                )
                if package := project.patches[patch].get("package"):
                    fout.write(', package = "%s"' % (package,))
                fout.write(" }\n")
            fout.write("\n")

    rust_targets = sorted(
        [
            {
                "label": t,
                "crate_name": project.targets[t]["crate_name"],
                "type": project.targets[t]["type"],
                "cargo_manifest_dir": lookup[t],
                "crate_root": project.targets[t]["crate_root"],
            }
            for t in project.rust_targets
            if t in project.reachable_targets
        ],
        key=lambda t: t["label"],
    )

    # Returns a single rust target per "base" label (not including toolchain),
    # either for fuchsia toolchains or host toolchains. This is used for rustdoc
    # where we only want to document each crate once for fuchsia and once for host.
    def rustdoc_targets(host):
        cur = ""
        result = []
        for t in rust_targets:
            disable = None
            if meta := project.rust_targets[t["label"]].get("metadata"):
                disable = meta.get("disable_rustdoc")
            if disable == [True] or (
                t["type"] == "executable" and disable != [False]
            ):
                continue
            l = t["label"].replace(".actual", "")
            base = l.split("(")[0]
            is_host = "(//build/toolchain:host" in l
            if host == is_host:
                if base == cur:
                    continue
                cur = base
                result.append(copy.deepcopy(t))
                result[-1]["label"] = l
        return result

    def dump_json(obj, filename):
        with open(os.path.join(gn_cargo_dir, filename), "w") as f:
            json.dump(obj, f)

    dump_json(rust_targets, "rust_targets.json")
    dump_json(rustdoc_targets(host=False), "rustdoc_targets.json")
    dump_json(rustdoc_targets(host=True), "rustdoc_host_targets.json")

    return 0


if __name__ == "__main__":
    sys.exit(main())
