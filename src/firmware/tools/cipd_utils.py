#!/usr/bin/env fuchsia-vendored-python

# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utilities for working with firmware prebuilts in CIPD.

Firmware is often built out-of-tree in a standalone repo, then uploaded to CIPD
as prebuilt artifacts, then pulled into the Fuchsia tree via jiri manifest.

The exact workflows may differ for various targets, but this script exposes
some common target-agnostic utilities for working with these CIPD packages.
"""

import argparse
import json
import logging
import os
import re
import subprocess
import tempfile

_MY_DIR = os.path.dirname(__file__)
_FUCHSIA_ROOT = os.path.normpath(os.path.join(_MY_DIR, "..", "..", ".."))

# For now assume `git` and `repo` live somewhere on $PATH.
_GIT_TOOL = "git"
_REPO_TOOL = "repo"

# `cipd` is available as a prebuilt in the Fuchsia checkout, but fall back to $PATH
# in case this is being run out-of-tree.
CIPD_TOOL = os.path.join(_FUCHSIA_ROOT, ".jiri_root", "bin", "cipd")
if not os.path.isfile(CIPD_TOOL):
    CIPD_TOOL = "cipd"


class Git:
    """Wraps operations on a git repo."""

    def __init__(self, repo_path: str, revision: str):
        """Initializes a Git object.

        Args:
            repo_path: path to git repo root.
            revision: current revision
        """
        self.repo_path = repo_path
        self.revision = revision

    def git(
        self,
        command: list[str],
        check: bool = True,
        capture_output: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        """Calls `git` in this repo.

        Args:
            command: git command to execute.
            check: passed to subprocess.run().
            capture_output: True to capture output, False to let it through to
                            the calling terminal.

        Returns:
            The resulting subprocess.CompletedProcess object.
        """
        return subprocess.run(
            [_GIT_TOOL, "-C", self.repo_path] + command,
            check=check,
            text=True,
            capture_output=capture_output,
        )

    def changelog(self, start: str | None, end: str) -> str:
        """Returns the additive changelog between two revisions.

        An additive changelog only contains the commits that exist in |end| but
        not |start|. If |start| is not a direct ancestor of |end|, there may
        also be commits only in |start|, which can be determined by calling this
        function again with the commit versions reversed.

        Args:
            start: the starting revision, or None to return the entire history.
            end: the ending revision.

        Returns:
            A changelog of commits that exist in |end| but not in |start|.

            The changelog is formatted as "oneline" descriptions of each commit
            separated by newlines.

        Raises:
            subprocess.CalledProcessError: failed to read the git log.
        """
        log_target = f"{start}..{end}" if start else end
        return self.git(["log", "--oneline", log_target]).stdout.strip()


class Repo:
    """Wraps operations on a `repo` checkout."""

    # Repo spec is a {path, alias} mapping for when we need to handle
    # more complicated repo checkouts.
    #
    # If alias is None, it means to use the project name.
    Spec = dict[str, str | None]

    def __init__(self, root: str, spec: Spec | None = None):
        """Initializes the Repo object.

        Args:
            root: path to repo root.
            spec: a Spec of gits to track, None to track all.

        Raises:
            ValueError if there are multiple git projects mapped to the same
            name (provide a spec to disambiguate) or if the spec had items
            that were not found in the manifest.
        """
        # Use the real path so that we can correlate between the root,
        # mount path, and relative spec paths.
        self.root = os.path.realpath(root)
        self.git_repos = self._list_git_repos(spec)

    def manifest(self, allow_dirty: bool) -> dict[str, str]:
        """Returns a {name: version} manifest of this repo.

        Normally this requires a clean repo, because the git revisions aren't stable if
        they are only local to this machine. A clean repo means they actually exist
        upstream and can be used to point to a specific version.

        Args:
            allow_dirty: allow generating a manifest on a dirty repo.

        Raises:
            ValueError if the repo is not clean.
        """
        if not allow_dirty:
            # `repo status --quiet` doesn't print anything if the repo is clean,
            # otherwise it prints the dirty repo path(s).
            repo_status = subprocess.run(
                [_REPO_TOOL, "status", "--quiet"],
                cwd=self.root,
                check=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            ).stdout.strip()
            if repo_status:
                raise ValueError(
                    f"Repo at '{self.root}' is dirty, cannot create stable manifest"
                )

        return {name: git.revision for name, git in self.git_repos.items()}

    def _list_git_repos(self, spec: Spec | None = None) -> dict[str, Git]:
        """Returns a {name: Git} mapping of all repos in this checkout."""
        # `repo info` gives us the information we need. Output format is:
        #   ---------------
        #   Project: <name>
        #   Mount path: <absolute_path>
        #   Current revision: <revision>
        #   Manifest revision: <revision>
        #   Local Branches: <#> [names]
        #   ---------------
        repo_info = subprocess.run(
            [_REPO_TOOL, "info", "--local-only"],
            cwd=self.root,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        matches = re.findall(
            r"Project: (.*?)$\s*Mount path: (.*?)$\s*Current revision: (.*?)$",
            repo_info.stdout,
            re.MULTILINE,
        )

        gits: dict[str, Git] = {}
        used_specs = set()
        for name, path, revision in matches:
            relative_path = os.path.relpath(path, self.root)

            # If a spec was given, only use the gits listed in the spec and
            # apply the alias if provided.
            if spec:
                if relative_path not in spec:
                    continue
                name = spec[relative_path] or name
                used_specs.add(relative_path)

            # Project names are not guaranteed to be unique, make sure that we
            # aren't clobbering any other git.
            if name in gits:
                raise ValueError(
                    f"Duplicate git '{name}' at {[gits[name], path]}"
                )

            gits[name] = Git(path, revision)

        # Make sure we used all the provided specs.
        if spec:
            unused_specs = set(spec.keys()).difference(used_specs)
            if unused_specs:
                raise ValueError(f"Unused specs: {unused_specs}")

        return gits


def download_cipd(name: str, version: str, path: str) -> None:
    """Downloads a CIPD package.

    Args:
        name: package name.
        version: package version.
        path: path to download the package to.
    """
    subprocess.run(
        [CIPD_TOOL, "ensure", "-root", path, "-ensure-file", "-"],
        check=True,
        capture_output=True,
        text=True,
        input=f"{name} {version}",
    )


def fetch_cipd_tags(name: str, version: str) -> list[str]:
    """Fetches the tags for a given CIPD package.

    Args:
        name: package name.
        version: package version.

    Returns:
        The package tags in "<key>:<value>" format.
    """
    with tempfile.TemporaryDirectory() as temp_dir:
        json_path = os.path.join(temp_dir, "metadata.json")
        subprocess.run(
            [
                CIPD_TOOL,
                "describe",
                name,
                "-version",
                version,
                "-json-output",
                json_path,
            ],
            check=True,
            capture_output=True,
            text=True,
        )

        with open(json_path, "r") as json_file:
            metadata = json.load(json_file)

    # `cipd describe` JSON looks like:
    # {
    #   "result": {
    #     ...
    #     "tags": [
    #       {
    #         "tag": "dev_cipd_version:sMPq54z3QpafC2JIAUsl_RiVi2VIs68ZNsA2ANSBvOIC",
    #         "registered_by": "user:vendor-google-ci-builder@fuchsia-infra.iam.gserviceaccount.com",
    #         "registered_ts": 1665093335
    #       },
    #       ...
    #     ]
    #   }
    # }
    return [t["tag"] for t in metadata["result"]["tags"]]


def get_cipd_version_manifest(
    package: str, version_or_path: str
) -> dict[str, str]:
    """Returns the contents of the manifest.json file in a CIPD package.

    We accept a version or a local path here because it's useful to produce
    a changelist between an existing CIPD package and a not-yet-submitted
    package locally *before* uploading it, to double-check we're uploading
    the right thing.

    Args:
        package: CIPD package name.
        version: CIPD package version or path.

    Returns:
        A {name: version} mapping, or empty dict if manifest.json wasn't found.
    """

    def read_manifest(path: str) -> dict[str, str]:
        try:
            with open(path, "r") as file:
                return json.load(file)
        except FileNotFoundError:
            return {}

    if os.path.isdir(version_or_path):
        return read_manifest(os.path.join(version_or_path, "manifest.json"))

    # Download the package so we can read the manifest file.
    with tempfile.TemporaryDirectory() as temp_dir:
        download_cipd(package, version_or_path, temp_dir)
        return read_manifest(os.path.join(temp_dir, "manifest.json"))


def changelog(
    repo: Repo,
    old_package: str,
    old_version_or_path: str,
    new_package: str,
    new_version_or_path: str,
) -> str:
    """Generates a changelog between the two versions.

    Args:
        repo: Repo object.
        old_package: old CIPD package name.
        old_version_or_path: old CIPD version or path.
        new_package: new CIPD package name.
        new_version_or_path: new CIPD version or path.

    Returns:
        A changelog formatted for use in a git commit message.
    """
    old_manifest = get_cipd_version_manifest(old_package, old_version_or_path)
    new_manifest = get_cipd_version_manifest(new_package, new_version_or_path)

    # The manifests don't necessarily have the exact same items, repos may be
    # added or removed over time, make sure we track them all.
    all_manifest_repos = set(old_manifest.keys()).union(new_manifest.keys())

    # Track diffs as (repo, revision, added, removed) tuples.
    diffs = []

    for manifest_repo in sorted(all_manifest_repos):
        old_revision = old_manifest.get(manifest_repo, None)
        new_revision = new_manifest.get(manifest_repo, None)
        added, removed = None, None

        if new_revision is None:
            # If the repo has been removed, we can't get history since it won't
            # exist in a ToT checkout.
            removed = "[repo has been removed]"
        else:
            git_repo = repo.git_repos[manifest_repo]
            added = git_repo.changelog(old_revision, new_revision)
            if old_revision:
                # If the old revision wasn't a direct ancestor but was a
                # separate branch, there may be some CLs that do not exist now
                # in the new revision.
                removed = git_repo.changelog(new_revision, old_revision)

        diffs.append([manifest_repo, new_revision, added, removed])

    # Format the changelog to be suitable for a commit message.
    lines = ["-- Changelist --"]
    for name, _, added, removed in diffs:
        if not (added or removed):
            continue

        lines.append(f"{name}:")
        if added:
            lines.append(added)
        if removed:
            lines.append("[removed commits:]")
            lines.append(removed)
        lines.append("")

    if len(lines) == 1:
        lines.append("[no changes]")
        lines.append("")

    lines.append("-- Source Revisions --")
    for name, revision, _, _ in diffs:
        if name in new_manifest:
            lines.append(f"{name}: {revision}")
    return "\n".join(lines)


def copy(source_package: str, source_version: str, dest_package: str) -> None:
    """Copies a CIPD package.

    Args:
        source_package: source package name.
        source_version: source version.
        dest_package: destination package name.
    """
    with tempfile.TemporaryDirectory() as temp_dir:
        download_cipd(source_package, source_version, temp_dir)

        command = [
            CIPD_TOOL,
            "create",
            "-name",
            dest_package,
            "-in",
            temp_dir,
            "-install-mode",
            "copy",
        ]
        for tag in fetch_cipd_tags(source_package, source_version):
            command += ["-tag", tag]
        # Always add an extra tag indicating where this copy came from.
        command += ["-tag", f"copied_from:{source_package}/{source_version}"]

        subprocess.run(command, check=True)


def upload(package: str, contents_dir: str) -> str:
    """Uploads a CIPD package.

    Adds CIPD tags using the manifest.json file in the contents_dir, and also marks
    the package with the standard "latest" ref.

    Args:
        package: package name.
        content_dir: path to the directory containing the contents.

    Returns:
        The created package version ID.
    """
    with tempfile.TemporaryDirectory() as temp_dir:
        json_out_path = os.path.join(temp_dir, "cipd_out.json")
        command = [
            CIPD_TOOL,
            "create",
            "-name",
            package,
            "-in",
            contents_dir,
            "-install-mode",
            "copy",
            "-ref",
            "latest",
            "-json-output",
            json_out_path,
        ]

        # Add revision tags from the manifest file.
        with open(os.path.join(contents_dir, "manifest.json"), "r") as file:
            manifest = json.load(file)
        for project, revision in manifest.items():
            # CIPD tags can't have slashes, so replace them with underscores.
            command += ["-tag", f"{project.replace('/', '_')}:{revision}"]

        subprocess.run(command, check=True)

        # The output json looks like:
        # {
        #     "result": {
        #         "package": "package_name",
        #         "instance_id": "cipd_version"
        #     }
        # }
        with open(json_out_path, "r") as file:
            output = json.load(file)
        return output["result"]["instance_id"]


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )

    subparsers = parser.add_subparsers(dest="action", required=True)

    def _add_common_repo_args(parser: argparse.ArgumentParser) -> None:
        """Adds common args for commands that point to a repo."""
        parser.add_argument("repo", help="Path to the repo root")
        parser.add_argument(
            "--spec-file",
            help="Repo specification file as a JSON {path, alias} mapping. By"
            " default all git repos in the manifest are used, but if a spec is"
            " provided only the listed git repos are included. Alias is the name"
            " to give the git repo in the changelog, or null to use the project"
            " name.",
        )

    changelog_parser = subparsers.add_parser(
        "changelog",
        help="Generate a source changelog between two CIPD packages",
    )
    _add_common_repo_args(changelog_parser)
    changelog_parser.add_argument("package", help="The old CIPD package name")
    changelog_parser.add_argument(
        "old_version", help="The old version: ref, tag, ID, or local path"
    )
    changelog_parser.add_argument(
        "new_package_or_version",
        help="The new CIPD package (4-arg form) or version (3-arg form)",
    )
    changelog_parser.add_argument(
        "new_version",
        nargs="?",
        help="The new version: ref, tag, ID, or local path",
    )

    copy_parser = subparsers.add_parser("copy", help="Copy a CIPD package")
    copy_parser.add_argument("source_package", help="Source CIPD name")
    copy_parser.add_argument("source_version", help="Source CIPD version")
    copy_parser.add_argument("dest_package", help="Destination CIPD name")

    manifest_parser = subparsers.add_parser(
        "manifest",
        help="Print a version manifest for the given repo, formatted as expected for"
        " use with this script",
    )
    _add_common_repo_args(manifest_parser)
    manifest_parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="Allow dirty repos. Note that the resulting manifest is not stable since"
        " the revisions only apply to the local checkout, so should only be used for"
        " local testing and verification",
    )

    create_parser = subparsers.add_parser(
        "upload", help="Uploads a CIPD package"
    )
    create_parser.add_argument("package", help="CIPD package name")
    create_parser.add_argument(
        "contents",
        help="Path to the directory containing the package contents; must contain a"
        " manifest.json file which will be translated into CIPD tags",
    )
    create_parser.add_argument(
        "--version-out",
        help="Path to a file to write the new version string to",
    )

    return parser.parse_args()


def main() -> None:
    """Script entry point."""
    logging.basicConfig()
    args = _parse_args()

    if args.action == "changelog":
        if args.spec_file:
            with open(args.spec_file, "r") as f:
                spec = json.load(f)
        else:
            spec = None

        repo = Repo(args.repo, spec=spec)

        if args.new_version:
            # 4-arg format: |new_package_or_version| is a package.
            result = changelog(
                repo,
                args.package,
                args.old_version,
                args.new_package_or_version,
                args.new_version,
            )
        else:
            # 3-arg format: re-use |package| for both, |new_package_or_version|
            # is a version.
            result = changelog(
                repo,
                args.package,
                args.old_version,
                args.package,
                args.new_package_or_version,
            )

        print(result)

    elif args.action == "copy":
        copy(args.source_package, args.source_version, args.dest_package)

    elif args.action == "manifest":
        if args.spec_file:
            with open(args.spec_file, "r") as f:
                spec = json.load(f)
        else:
            spec = None

        repo = Repo(args.repo, spec=spec)
        print(
            json.dumps(
                repo.manifest(args.allow_dirty), indent=2, sort_keys=True
            )
        )

    elif args.action == "upload":
        new_version = upload(args.package, args.contents)
        if args.version_out:
            with open(args.version_out, "w") as file:
                file.write(new_version)

    else:
        raise NotImplementedError(f"Unimplemented command: {args.action}")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as e:
        # Dump stdout/stderr for failed subprocesses so the caller knows what failed.
        logging.error("Failed command: `%s`", " ".join([str(c) for c in e.cmd]))
        if e.stdout:
            logging.error("\n-- stdout --\n%s------------", e.stdout)
        if e.stderr:
            logging.error("\n-- stderr --\n%s------------", e.stderr)
        raise
