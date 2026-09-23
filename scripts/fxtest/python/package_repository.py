# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import functools
import json
import os
import re
import typing

import environment


class PackageRepositoryError(Exception):
    """Raised when there was an issue processing the package repository."""


class PackageRepository:
    """Wrapper for package repository data created during the build process."""

    def __init__(self, name_to_merkle: dict[str, str]):
        """Create a representation of a package directory.

        Args:
            name_to_merkle (dict[str, str]): Mapping of package name to merkle hash.
        """
        self.name_to_merkle: dict[str, str] = name_to_merkle

    @classmethod
    def from_env(
        cls, exec_env: environment.ExecutionEnvironment
    ) -> "PackageRepository":
        """Create a package repository wrapper from an environment.

        Args:
            exec_env (environment.ExecutionEnvironment): Environment to load from.

        Raises:
            PackageRepositoryError: If there was an issue loading the package repository information.
        """
        targets_path: str

        if exec_env.package_repositories_file is None:
            raise PackageRepositoryError(
                "No package-repositories.json file was found for the build."
            )

        try:
            with open(exec_env.package_repositories_file, "r") as f:
                targets_path = os.path.join(
                    os.path.dirname(exec_env.package_repositories_file),
                    json.load(f)[0]["targets"],
                )

            name_to_merkle: dict[str, str] = dict()
            with open(targets_path, "r") as f:
                entries = json.load(f)
                key: str
                value: dict[str, typing.Any]
                for key, value in entries["signed"]["targets"].items():
                    if "custom" in value:
                        name = key.split("/")[0]
                        name_to_merkle[name] = value["custom"]["merkle"]

            return cls(name_to_merkle)

        except Exception as e:
            raise PackageRepositoryError(
                f"Error loading package repository: {e}"
            )

    @classmethod
    @functools.lru_cache()
    def from_env_cached(
        cls, exec_env: environment.ExecutionEnvironment
    ) -> "PackageRepository":
        """Create a package repository wrapper from an environment, reusing previous results if present.

        Args:
            exec_env (environment.ExecutionEnvironment): Environment to load from.

        Raises:
            PackageRepositoryError: If there was an issue loading the package repository information.
        """
        return cls.from_env(exec_env)

    def resolve_component_url(self, component_url: str) -> str:
        """Resolve a component URL to include its Merkle hash.

        Args:
            component_url (str): The component URL.

        Returns:
            str: The component URL with ?hash=<merkle> inserted.

        Raises:
            PackageRepositoryError: If parsing fails or the Merkle hash is missing.
        """
        name = extract_package_name_from_url(component_url)
        if name is None:
            raise PackageRepositoryError(
                f"Failed to parse package name for Merkle root matching: {component_url} "
                f"{MERKLE_ERROR_HELP_SUFFIX}"
            )
        if name not in self.name_to_merkle:
            raise PackageRepositoryError(
                f"Could not find a Merkle hash for this test: {component_url} "
                f"{MERKLE_ERROR_HELP_SUFFIX}"
            )
        suffix = f"?hash={self.name_to_merkle[name]}"
        return component_url.replace("#", f"{suffix}#", 1)


MERKLE_ERROR_HELP_SUFFIX = "\nTry running fx build."

_PACKAGE_NAME_REGEX = re.compile(r"fuchsia-pkg://[^/#]+/([^/#]+)#")


def extract_package_name_from_url(url: str) -> str | None:
    """Given a fuchsia-pkg URL, extract and return the package name.

    Example:
      fuchsia-pkg://fuchsia.com/my-package#meta/my-component.cm -> my-package

    Args:
        url (str): A fuchsia-pkg:// URL.

    Returns:
        str | None: The package name from the URL, or None if parsing failed.
    """
    match = _PACKAGE_NAME_REGEX.match(url)
    if match is None:
        return None
    return match.group(1)
