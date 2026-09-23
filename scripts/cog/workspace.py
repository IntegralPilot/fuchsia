# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import base64
import fcntl
import json
import logging
import os
import shutil
import subprocess
import urllib.request
from contextlib import contextmanager
from datetime import datetime, timedelta
from functools import cache, cached_property
from pathlib import Path
from typing import (
    Any,
    Callable,
    Concatenate,
    Generator,
    Literal,
    ParamSpec,
    Protocol,
    TextIO,
    TypeVar,
)

import cartfs
import logger as logger
import snapshotter
from util import sanitize_filename


class WorkspaceError(Exception):
    """Base exception for Cartfs errors."""


class RepoSetupError(WorkspaceError):
    """Raised when there is an error setting up the repository."""


class NotInCogWorkspaceError(WorkspaceError):
    """Raised when the current directory is not within a Cog workspace."""


CARTFS_SYMLINK_NAME: str = "cartfs-dir"
COG_METADATA_FILE_NAME: str = ".cog.json"


class CogMetadata:
    """Represents the metadata stored in the .cog.json file."""

    def __init__(
        self,
        workspace_name: str,
        repo_name: str,
        workspace_id: str | None = None,
    ):
        """Initializes CogMetadata.

        Args:
            workspace_name: The name of the cog workspace.
            repo_name: The name of the repository within the workspace.
            workspace_id: The unique ID for the workspace.
        """
        self.workspace_name = workspace_name
        self.repo_name = repo_name
        self.workspace_id = workspace_id

    def to_dict(self) -> dict[str, str | None]:
        """Returns a dictionary representation of the metadata."""
        return {
            "workspace_name": self.workspace_name,
            "repo_name": self.repo_name,
            "workspace_id": self.workspace_id,
        }

    @classmethod
    def from_file(cls, path: Path) -> "CogMetadata | None":
        """Loads metadata from a .cog.json file.

        Args:
            path: The full path to the .cog.json file.

        Returns:
            A CogMetadata instance if the file is valid, otherwise None.
        """
        if not path.exists():
            return None
        try:
            data = json.loads(path.read_text())
            return cls(
                workspace_name=data["workspace_name"],
                repo_name=data["repo_name"],
                workspace_id=data.get("workspace_id"),
            )
        except (
            OSError,
            json.JSONDecodeError,
            KeyError,
        ) as e:
            logger.log_warn(f"Warning: Could not read or parse {path}: {e}")
            return None

    def write(self, directory: Path) -> None:
        """Writes the metadata to a JSON file in the given directory."""
        path = directory / COG_METADATA_FILE_NAME
        path.write_text(json.dumps(self.to_dict(), indent=4))


class HasWorkspace(Protocol):
    workspace: "Workspace"


T = TypeVar("T", bound=HasWorkspace)
P = ParamSpec("P")
R = TypeVar("R")


def lock(
    func: Callable[Concatenate[T, P], R]
) -> Callable[Concatenate[T, P], R]:
    """Wraps a method with `self.workspace.lock()`.

    Note: The decorated method must be called on an object that has a
    `self.workspace` attribute.
    """

    def lock_and_call(self: T, /, *args: P.args, **kwargs: P.kwargs) -> R:
        with self.workspace.lock():
            return func(self, *args, **kwargs)

    return lock_and_call


class Workspace:
    """A class to encapsulate a Cog workspace and an associated Cartfs workspace."""

    @staticmethod
    @cache
    def cogd_path() -> Path:
        try:
            return Path(
                subprocess.check_output(
                    ["git-citc", "cogd"],
                    text=True,
                ).strip()
            )
        except (FileNotFoundError, subprocess.CalledProcessError) as e:
            raise NotInCogWorkspaceError(
                "Unable to find the cog workspace. Are you in a cog workspace?"
            ) from e

    def __init__(
        self,
        repo_dir: Path | None = None,
    ) -> None:
        """Initializes a Workspace instance."""
        self.repo_dir = repo_dir or Workspace.cogd_path()
        self.cartfs_instance = cartfs.Cartfs()
        self.cartfs_mount_point = self.cartfs_instance.mount_point
        self._is_cartfs_workspace_clean = False
        self._lock_file_handle: TextIO | None = None
        self._lock_count = 0
        logger.setup_file_logging(self.workspace_root)

    @property
    def workspace_root(self) -> Path:
        return self.repo_dir.parent

    @cached_property
    def workspace_id(self) -> str:
        return (
            (self.workspace_root / ".citc" / "workspace_id").read_text().strip()
        )

    @property
    def workspace_name(self) -> str:
        return self.workspace_root.name

    @property
    def repo_name(self) -> str:
        return self.repo_dir.name

    @cached_property
    def _golden_snapshot_dir(self) -> Path | None:
        """Returns the golden snapshot directory, if it exists and is valid.

        Returns:
            A path to the resolved golden snapshot directory relative to the cartfs mount point if
            found and valid, otherwise None.
        """
        # Golden snapshots are not supported for fuchsia/fuchsia.
        if self.repo_dir.name == "fuchsia":
            return None

        # `.fuchsia_golden_snapshot` is a symlink pointing to the currently active golden snapshot.
        result = (
            self.cartfs_mount_point / ".fuchsia_golden_snapshot"
        ).resolve()

        # Check that the golden snapshot is not corrupted.
        if not (result / ".integration_commit_hash").is_file():
            return None

        # The golden snapshot must be found within the cartfs mount point.
        try:
            return result.relative_to(self.cartfs_mount_point)
        except ValueError:
            return None

    @cached_property
    def cartfs_dir(self) -> Path:
        cartfs_dir = self._get_linked_cartfs_dir()
        if not cartfs_dir:
            raise RepoSetupError("No cartfs directory found.")
        return cartfs_dir

    @property
    def has_cartfs_dir(self) -> bool:
        try:
            _ = self.cartfs_dir
            return True
        except RepoSetupError:
            return False

    @property
    def cartfs_fuchsia_dir(self) -> Path:
        return self.cartfs_dir / "fuchsia"

    def _get_linked_cartfs_dir(self) -> Path | None:
        """Gets the linked cartfs directory for a specific repo in a cog workspace.

        A workspace is considered linked if a symlink named `cartfs-dir` exists
        inside the specified repository directory, pointing to a valid cartfs
        directory. This target cartfs directory must contain a `.cog.json` file
        with a matching `repo_name`, `workspace_name`, and `workspace_id`.

        Returns:
            The absolute path to the linked cartfs directory if found and valid,
            otherwise None.
        """
        symlink_path = self.repo_dir / CARTFS_SYMLINK_NAME
        if not symlink_path.is_symlink():
            return None

        target_path = symlink_path.readlink()
        if not target_path.is_absolute():
            # Handles relative symlinks. The target is relative to the directory
            # containing the symlink.
            target_path = self.repo_dir / target_path

        if not target_path.is_dir():
            return None

        metadata = CogMetadata.from_file(target_path / COG_METADATA_FILE_NAME)
        if not metadata:
            return None

        if (
            metadata.repo_name != self.repo_name
            or metadata.workspace_name != self.workspace_name
            or metadata.workspace_id != self.workspace_id
        ):
            return None

        return target_path

    @cached_property
    def lock_file(self) -> Path:
        lock_dir = Path.home() / ".cache" / "cog"
        lock_dir.mkdir(parents=True, exist_ok=True)
        return lock_dir / sanitize_filename(
            f"{self.workspace_name}-{self.workspace_id}.lock"
        )

    @contextmanager
    def lock(self) -> Generator[None, None, None]:
        """Synchronously locks operations on this Workspace instance.

        Nested lock attempts are no-ops. Not thread safe.
        """
        # Lock is only acquired when entering the first `lock` context.
        self._lock_count += 1
        if self._lock_count == 1:
            try:
                # Acquire lock now, or wait until another process releases the lock.
                self._lock_file_handle = open(self.lock_file, "a+")
                try:
                    fcntl.flock(
                        self._lock_file_handle, fcntl.LOCK_EX | fcntl.LOCK_NB
                    )
                except BlockingIOError:
                    try:
                        lock_owner_pid = self.lock_file.read_text().strip()
                    except Exception:
                        lock_owner_pid = None
                    logger.log_warn(
                        f"Waiting for another process (PID: {lock_owner_pid or 'unknown'}) "
                        "to finish working on this workspace..."
                    )
                    fcntl.flock(self._lock_file_handle, fcntl.LOCK_EX)

                # Update lock file with current process ID.
                self._lock_file_handle.seek(0)
                self._lock_file_handle.truncate()
                self._lock_file_handle.write(str(os.getpid()))
                self._lock_file_handle.flush()
                logger.log_debug(
                    f"Acquired lock for workspace: {self.lock_file}"
                )
            except BaseException:
                logger.log_error("Could not acquire lock for workspace.")
                if self._lock_file_handle:
                    self._lock_file_handle.close()
                    self._lock_file_handle = None
                self._lock_count -= 1
                raise

        try:
            # Lock acquired, continue execution.
            yield
        finally:
            # Lock is only released when exiting the last `lock` context.
            self._lock_count -= 1
            if self._lock_count == 0:
                assert self._lock_file_handle
                self._lock_file_handle.close()
                self._lock_file_handle = None
                logger.log_debug(
                    f"Released lock for workspace: {self.lock_file}"
                )

    def _assert_locked(self) -> None:
        """Asserts that the workspace lock is held by the invoker via `lock`.

        This assertion should be enforced by any method that needs to ensure that this
        workspace is not being actively modified by another `//scripts/cog` process.
        """
        if self._lock_count == 0:
            raise WorkspaceError(
                "Please acquire a workspace lock before calling this method."
            )

    @cached_property
    def config(self) -> dict[str, Any]:
        repo_config_path = (
            self.repo_dir / "scripts" / "cog" / "repo_config.json"
        )
        if not repo_config_path.exists():
            raise FileNotFoundError(
                f"Repo config not found at {repo_config_path}"
            )
        return json.loads(repo_config_path.read_text())

    def init_cartfs_workspace(self, snapshot: bool = True) -> None:
        """Initializes the cartfs workspace.

        Args:
            snapshot: Whether to snapshot the cartfs workspace from a previous instance.
        """
        self._assert_locked()

        if snapshot:
            logger.emit_status("Attempting to snapshot CartFS workspace...")
            self._init_cartfs_workspace_snapshot()

        # If snapshotting failed or was disabled, initialize an empty workspace.
        if not self.has_cartfs_dir:
            logger.emit_status("Creating an empty CartFS workspace...")
            self._init_cartfs_workspace_empty()
            self._is_cartfs_workspace_clean = True

    def _init_cartfs_workspace_snapshot(
        self,
        snapshot_function: Callable[[Path, Path, Path], None] | None = None,
    ) -> None:
        """Snapshots and links to the workspace from the most recent cartfs directory."""
        if snapshot_function is None:
            snapshot_function = snapshotter.snapshot_workspace
        snapshot_src = (
            self._golden_snapshot_dir or self._find_previous_instance()
        )
        if not snapshot_src:
            logger.log_info("No previous cartfs workspace directory found.")
            return

        suggested_directory_name = self.cartfs_instance.suggest_cartfs_dir_name(
            sanitize_filename(f"{self.workspace_name}-{self.workspace_id}")
        )
        try:
            snapshot_function(
                snapshot_src,
                suggested_directory_name,
                self.cartfs_instance.mount_point,
            )
        except Exception:
            logger.log_exception("An error occurred while snapshotting.")
            return

        self._link_to_cartfs(
            self.cartfs_instance.mount_point / suggested_directory_name
        )

        # Trigger Cog workspace sync if we initialized from the golden snapshot.
        if snapshot_src == self._golden_snapshot_dir:
            self._is_cartfs_workspace_clean = True
            self._sync_cog_to_golden_revision()

    def _sync_cog_to_golden_revision(self) -> None:
        logger.emit_status(
            "Syncing Cog CITC base to match golden snapshot superproject..."
        )

        superproject_dir = self.cartfs_dir / "fuchsia-cog-superproject"
        if not superproject_dir.is_dir():
            logger.log_error(
                f"fuchsia-cog-superproject directory not found in CartFS at {superproject_dir}"
            )
            return

        try:
            # Read the superproject commit hash from our isolated snapshot in CartFS
            superproject_commit = self._run(
                ["git", "rev-parse", "HEAD"],
                cwd=superproject_dir,
                capture_output=True,
            ).strip()
        except subprocess.CalledProcessError as e:
            err_msg = e.stderr.decode("utf-8", errors="ignore").strip() or None
            logger.log_error(
                f"Failed to get superproject commit from CartFS snapshot: {e}. Output: {err_msg}"
            )
            return

        logger.log_info(
            f"Syncing Cog superproject '{self.repo_name}' to {superproject_commit}"
        )
        try:
            self._run(
                [
                    "git-citc",
                    "api.call",
                    "Rebase",
                    f'repo_root: "{self.repo_name}" new_base: "{superproject_commit}"',
                ],
                cwd=self.repo_dir,
                capture_output=True,
            )
            logger.emit_status("Cog CITC base synced successfully.")
        except subprocess.CalledProcessError as e:
            err_msg = e.stderr.decode("utf-8", errors="ignore").strip() or None
            logger.log_error(
                f"Failed to sync Cog CITC base: {e}. Output: {err_msg}"
            )

    def _init_cartfs_workspace_empty(self) -> None:
        """Links to a new, empty directory in the cartfs mount for this workspace.

        This method generates a unique directory name based on the workspace name,
        creates the directory, and writes a `.cog.json` metadata file into it.
        """
        suggested_directory_name = self.cartfs_instance.suggest_cartfs_dir_name(
            sanitize_filename(f"{self.workspace_name}-{self.workspace_id}")
        )
        cartfs_workspace_dir = (
            self.cartfs_instance.mount_point / suggested_directory_name
        )
        cartfs_workspace_dir.mkdir()

        self._link_to_cartfs(cartfs_workspace_dir)

    def _link_to_cartfs(self, cartfs_dir: Path) -> None:
        """Links the cog workspace to a cartfs directory.

        This creates a symlink named `cartfs-dir` inside the repository
        directory of the cog workspace. This symlink points to the specified
        cartfs directory, establishing the link between them. If a symlink
        already exists, it will be replaced.

        Additionally, it writes a `.cog.json` metadata file into the cartfs
        directory.

        Args:
            cartfs_dir: The absolute path to the target cartfs directory.
        """

        symlink_path = self.repo_dir / CARTFS_SYMLINK_NAME

        # Create an absolute symlink from the repo directory to the cartfs
        # workspace directory. If a symlink already exists, remove it first.
        if symlink_path.is_symlink():
            symlink_path.unlink()
        symlink_path.symlink_to(cartfs_dir)

        metadata = CogMetadata(
            workspace_name=self.workspace_name,
            repo_name=self.repo_name,
            workspace_id=self.workspace_id,
        )
        metadata.write(cartfs_dir)

        # Invalidate the cached property.
        vars(self).pop("cartfs_dir", None)

    def _find_previous_instance(self) -> Path | None:
        """Finds the most recent cartfs directory for the same repo.

        This method iterates through all directories in the cartfs mount point,
        looking for directories that are linked to a workspace with the same repo
        name as the current one. It then returns the path to the one with the
        most recent modification time.

        Returns:
            The path, relative to the cartfs mount point, to the newest
            directory found, or None if no instances are found.
        """
        mount_point = Path(self.cartfs_instance.mount_point)
        if not mount_point or not mount_point.is_dir():
            return None

        candidates = set()
        for entry in mount_point.iterdir():
            if not entry.is_dir():
                continue

            metadata = CogMetadata.from_file(entry / COG_METADATA_FILE_NAME)
            if not metadata:
                continue

            repo_name = metadata.repo_name

            # Check if it's for the same repo.
            if repo_name != self.repo_name:
                continue

            candidates.add(entry)

        newest_candidate = None
        newest_mtime = -1.0
        for candidate in candidates:
            try:
                mtime = candidate.stat().st_mtime
                if mtime > newest_mtime:
                    newest_mtime = mtime
                    newest_candidate = candidate
            except FileNotFoundError:
                # The directory was deleted between listing and stat-ing.
                continue
        return (
            newest_candidate.relative_to(mount_point)
            if newest_candidate
            else None
        )

    def is_checkout_uptodate(self) -> bool:
        """Checks if the CartFS checkouts are up to date with Cog."""
        self._assert_locked()

        if self.config.get("useJiriUpdateOnly", False):
            # In Superproject, we will only check the integration commit is
            # aligned.
            cog_integration_commit = self.get_cog_commit(
                self.config["repo"]["integration"]
            )
            cartfs_integration_commit = self.get_cartfs_commit("integration")
            logger.log_debug(
                f"Cog integration commit: {cog_integration_commit}"
            )
            logger.log_debug(
                f"CartFS integration commit: {cartfs_integration_commit}"
            )
            return cog_integration_commit == cartfs_integration_commit
        else:
            # In fuchsia, we will check fuchsia commit is aligned.
            cog_fuchsia_commit = self.get_cog_commit(
                self.config["repo"]["fuchsia"]
            )
            cartfs_fuchsia_commit = self.get_cartfs_commit("fuchsia")
            logger.log_debug(f"Cog Fuchsia commit: {cog_fuchsia_commit}")
            logger.log_debug(f"CartFS Fuchsia commit: {cartfs_fuchsia_commit}")
            return cog_fuchsia_commit == cartfs_fuchsia_commit

    def checkout_cartfs_to_cog_revisions(self) -> None:
        """Checkouts the CartFS fuchsia and integration repos to match the revisions in Cog."""
        self._assert_locked()
        if not self._is_jiri_bootstrapped():
            self._bootstrap_jiri()

        cog_integration_repo = self.config["repo"]["integration"]

        cog_fuchsia_commit = self.get_cog_commit(self.config["repo"]["fuchsia"])
        cog_integration_commit = cog_integration_repo and self.get_cog_commit(
            cog_integration_repo
        )

        # Update CartFS integration and fuchsia checkouts.
        if cog_integration_repo:
            self._reinit_integration_repo(cog_integration_commit)
        else:
            self._reinit_integration_repo()

            # Try to find the integration commit that rolled `cog_fuchsia_commit`.
            logger.log_debug(
                f"Current CartFS integration commit: {self.get_cartfs_commit('integration')}"
            )
            cog_integration_commit = self._checkout_integration_roll(
                cog_fuchsia_commit
            )
            logger.log_debug(
                f"New CartFS integration commit: {cog_integration_commit}"
            )

        logger.emit_status(
            "Updating CartFS fuchsia and integration checkouts..."
        )

        self.cartfs_fuchsia_dir.mkdir(parents=True, exist_ok=True)
        self._write_jiri_manifest()
        self._write_jiri_config()

        if self.config.get("useJiriUpdateOnly", False):
            if (self.cartfs_fuchsia_dir / "integration").is_symlink():
                (self.cartfs_fuchsia_dir / "integration").unlink()
            self._update_jiri_checkout()
        else:
            self._sync_fuchsia_repo(cog_fuchsia_commit)
            self._fetch_prebuilts()

        self._create_symlinks()

        # Record the updated commit hashes in CartFS.
        (self.cartfs_dir / ".fuchsia_commit_hash").write_text(
            cog_fuchsia_commit
        )
        (self.cartfs_dir / ".integration_commit_hash").write_text(
            cog_integration_commit
        )

    def get_cog_commit(self, repository: str) -> str:
        """Determines the `repository` commit hash from CitC."""
        repo_states = (
            self._run(
                ["git-citc", "api.get-repo-states", repository],
                cwd=self.repo_dir,
                capture_output=True,
            )
            .strip()
            .split("\n")
        )
        for state in repo_states:
            parts = state.split(":", 1)
            if len(parts) == 2:
                key = parts[0].strip().strip("'\"")
                if key == "base_commit_hash":
                    return parts[1].strip().strip("'\"")

        logger.log_error(f"Failed to get {repository} repo commit hash.")
        raise RepoSetupError(f"Failed to get {repository} repo commit hash.")

    def get_cartfs_commit(
        self, repository: Literal["fuchsia", "integration"]
    ) -> str | None:
        """Determines the fuchsia or integration repo commit hash from CartFS."""
        self._assert_locked()
        hash_file = self.cartfs_dir / f".{repository}_commit_hash"
        if not hash_file.is_file():
            return None

        try:
            return hash_file.read_text().strip()
        except Exception:
            return None

    def _create_symlink(self, target: Path, link_name: Path) -> None:
        """Creates a symlink from link_name to target.

        If a symlink already exists at link_name and points to target, this
        function does nothing.

        If a file, directory, or a different symlink exists at link_name, it will
        be removed and replaced with the new symlink.
        """
        if link_name.is_symlink() and link_name.readlink() == target:
            return

        # If the path exists but is not the desired symlink, remove it.
        if link_name.is_dir() and not link_name.is_symlink():
            shutil.rmtree(link_name)
        else:
            link_name.unlink(missing_ok=True)

        if not link_name.parent.is_dir():
            link_name.parent.mkdir(parents=True, exist_ok=True)

        logger.log_debug(f"Creating symlink from {link_name} to {target}")
        link_name.symlink_to(target)

    def _write_jiri_manifest(self) -> None:
        """Writes the jiri manifest."""
        logger.emit_status("Writing jiri manifest...")
        jiri_manifest = self.cartfs_dir / "fuchsia" / ".jiri_manifest"
        content = (
            "<manifest><imports>"
            + "\n".join(
                f'<localimport file="{file}"/>'
                for file in self.config["jiriImports"]
            )
            + "</imports></manifest>\n"
        )
        jiri_manifest.parent.mkdir(parents=True, exist_ok=True)
        if not jiri_manifest.exists() or jiri_manifest.read_text() != content:
            jiri_manifest.write_text(content)
        logger.log_debug(f"Wrote jiri manifest to {jiri_manifest}")

    def _write_jiri_config(self) -> None:
        """Initializes the jiri config."""
        logger.emit_status("Initializing jiri config...")
        (self.cartfs_fuchsia_dir / ".jiri_root" / "bin").mkdir(
            exist_ok=True, parents=True
        )
        self._create_symlink(
            self.cartfs_mount_point / ".jiri_root" / "bin" / "jiri",
            self.cartfs_fuchsia_dir / ".jiri_root" / "bin" / "jiri",
        )
        self._run(
            [
                ".jiri_root/bin/jiri",
                "init",
                "-analytics-opt=true",
            ],
            cwd=self.cartfs_fuchsia_dir,
        )

        # Clean out any local changes from the CartFS checkout before running `jiri update`.
        # This isn't necessary if we just initialized from a golden snapshot or just created an
        # empty workspace.
        if not self._is_cartfs_workspace_clean:
            self._run(
                [".jiri_root/bin/jiri", "runp", "git", "clean", "-df"],
                cwd=self.cartfs_fuchsia_dir,
            )
            self._run(
                [".jiri_root/bin/jiri", "runp", "git", "restore", "."],
                cwd=self.cartfs_fuchsia_dir,
            )

    def _is_jiri_bootstrapped(self) -> bool:
        """Checks if jiri is bootstrapped."""
        return (
            self.cartfs_mount_point / ".jiri_root" / "bin" / "jiri"
        ).exists()

    def _bootstrap_jiri(self) -> None:
        """Bootstraps jiri if it is not already bootstrapped."""
        logger.emit_status("Bootstrapping jiri...")
        url = "https://fuchsia.googlesource.com/jiri/+/HEAD/scripts/bootstrap_jiri?format=TEXT"
        try:
            with urllib.request.urlopen(url, timeout=30) as response:
                encoded_script = response.read()
                decoded_script = base64.b64decode(encoded_script)
                subprocess.run(
                    ["bash", "-s", self.cartfs_mount_point],
                    input=decoded_script,
                    check=True,
                )
        except (urllib.error.URLError, subprocess.CalledProcessError) as e:
            logger.log_error(f"Failed to bootstrap jiri: {e}")
            raise

    def _update_jiri_checkout(self) -> None:
        """Updates the jiri checkout."""
        logger.emit_status("Updating jiri checkout...")
        logger.log_info(f"Running jiri update in {self.cartfs_fuchsia_dir}")

        cmd = (
            [".jiri_root/bin/jiri", "-v", "update"]
            if logger.get_log_level() <= logging.DEBUG
            else [".jiri_root/bin/jiri", "update"]
        )

        env = os.environ.copy()
        env["FUCHSIA_DIR"] = str(self.cartfs_fuchsia_dir)

        subprocess.run(
            cmd,
            cwd=self.cartfs_fuchsia_dir,
            check=True,
            env=env,
        )

    def _fetch_prebuilts(self) -> None:
        """Fetches prebuilts for the given repo."""
        logger.emit_status(f"Fetching prebuilts for {self.repo_name}...")
        cartfs_fuchsia_dir = self.cartfs_fuchsia_dir

        # Run jiri update and fetch-packages in parallel to speed up the
        # process.
        update_process = subprocess.Popen(
            [".jiri_root/bin/jiri", "update", "--fetch-packages=false"],
            cwd=cartfs_fuchsia_dir,
        )
        fetch_process = subprocess.Popen(
            [".jiri_root/bin/jiri", "fetch-packages"], cwd=cartfs_fuchsia_dir
        )
        update_process.wait()
        fetch_process.wait()

        # Create update history file manually since update with --fetch-packages=false doesn't do it.
        self._run(
            [
                ".jiri_root/bin/jiri",
                "snapshot",
                ".jiri_root/update_history/latest",
            ],
            cwd=cartfs_fuchsia_dir,
        )

    def _reinit_integration_repo(self, revision: str | None = None) -> None:
        """Destroys and re-clones the `integration` checkout in CartFS, with a depth of 100 cls."""
        logger.emit_status(f"Reinitializing the integration repository...")
        integration_dir = self.cartfs_dir / "integration"
        if integration_dir.is_dir():
            shutil.rmtree(integration_dir)

        remote = self.config["integration_url"]
        git_clone_cmd = [
            "git",
            "clone",
            remote,
            "integration",
            "--depth=100",
        ]
        if revision:
            git_clone_cmd.append(f"--revision={revision}")

        logger.emit_status("Cloning integration repo...")
        self._run(git_clone_cmd, self.cartfs_dir)

    def _checkout_integration_roll(self, fuchsia_commit: str) -> str:
        """Checks out the CartFS integration repo to the commit rolling `fuchsia_commit`.

        This is no-op if a roll cl containing `fuchsia_commit` isn't found.

        Returns the commit that was checked out.
        """
        integration_dir = self.cartfs_dir / "integration"
        if not integration_dir.is_dir():
            raise RepoSetupError("No integration directory found.")

        # We use the first 7 characters of the fuchsia repo to look up in
        # integration repo's commit message
        commit_hash_prefix = fuchsia_commit[:7]
        logger.log_debug(f"Fuchsia commit_hash_prefix: '{commit_hash_prefix}'")

        integration_base_commit_hash = (
            self._run(
                ["git", "log", "--grep", commit_hash_prefix, "--format=%H"],
                cwd=integration_dir,
                capture_output=True,
            )
            .strip()
            .split("\n")[-1]
        )
        logger.log_debug(
            f"Associated integration_base_commit_hash: '{integration_base_commit_hash}'"
        )

        if not integration_base_commit_hash:
            # TODO(https://fxbug.dev/500722390): This isn't completely correct.
            logger.log_info(
                "Fuchsia commit is not rolled to integration repo yet. We will "
                "use the latest integration repo commit hash."
            )
            return self._run(
                ["git", "rev-parse", "HEAD"],
                cwd=integration_dir,
                capture_output=True,
            ).strip()
        else:
            # checkout the integration repo based on the fuchsia repo commit hash
            self._run(
                ["git", "reset", "--hard", integration_base_commit_hash],
                cwd=integration_dir,
            )
            return integration_base_commit_hash

    def _sync_fuchsia_repo(self, commit: str) -> None:
        """Syncs the fuchsia repository to the specified commit hash."""
        integration_dir = self.cartfs_dir / "integration"
        if not integration_dir.is_dir():
            raise RepoSetupError("No integration directory found.")

        # clone fuchsia repository and reset it to the commit hash
        logger.emit_status(
            "Syncing the CartFS fuchsia checkout from "
            f"{self.get_cartfs_commit('fuchsia')} to {commit}"
        )

        backup_dir = None
        # For a newly created workspace, we would expect the fuchsia directory
        # exist but not setup yet. In this case the .git support is not there
        # yet so we skip resetting.
        if (self.cartfs_fuchsia_dir / ".git").exists():
            try:
                subprocess.run(
                    ["git", "reset", "--hard"],
                    cwd=self.cartfs_fuchsia_dir,
                    check=True,
                    capture_output=True,
                )
            except subprocess.CalledProcessError as e:
                logger.log_warn(
                    f"git reset failed in CartFS, likely corruption: {e}"
                )
                logger.log_warn(
                    "Attempting recovery by deleting corrupted directory."
                )
                timestamp = datetime.now().strftime("%Y%m%d%H%M%S")
                backup_dir = self.cartfs_fuchsia_dir.with_name(
                    f"fuchsia.broken.{timestamp}"
                )
                logger.log_warn(f"Moving corrupted directory to {backup_dir}")
                os.rename(self.cartfs_fuchsia_dir, backup_dir)

                # Fix .git/HEAD in backup directory to allow git commands
                backup_git_head = backup_dir / ".git" / "HEAD"
                try:
                    backup_git_head.write_text("ref: refs/heads/main")
                except Exception as e:
                    logger.log_warn(f"Failed to fix .git/HEAD in backup: {e}")

        if not (self.cartfs_fuchsia_dir / ".git").exists():
            # We fetch fuchsia repository from the last 4 days because git hook will
            # refer to commit from yesterday to generate integration_daily_commit_hash.
            integration_commit_timestamp = int(
                self._run(
                    ["git", "show", "-s", "--format=%ct"],
                    cwd=integration_dir,
                    capture_output=True,
                ).strip()
            )
            integration_commit_time = datetime.fromtimestamp(
                integration_commit_timestamp
            )
            four_days_ago = (
                integration_commit_time - timedelta(days=4)
            ).strftime("%Y-%m-%d")
            # We use a step-by-step approach (init, remote add, fetch, reset) instead of a single `git clone`
            # because `git clone` was failing in the CartFS FUSE mount, likely due to I/O limitations
            # during index-pack. Breaking it down allows us to bypass these filesystem issues.
            # We use --shallow-since to keep a few days of history to ensure
            # build integrity while keeping the operation fast.
            self._run(["git", "init", "fuchsia"], cwd=self.cartfs_dir)
            fuchsia_dir = self.cartfs_dir / "fuchsia"
            self._run(
                [
                    "git",
                    "remote",
                    "add",
                    "origin",
                    "https://fuchsia.googlesource.com/fuchsia",
                ],
                cwd=fuchsia_dir,
            )
            self._run(
                [
                    "git",
                    "fetch",
                    "origin",
                    commit,
                    f"--shallow-since={four_days_ago}",
                ],
                cwd=fuchsia_dir,
            )
            self._run(["git", "reset", "--hard", "FETCH_HEAD"], cwd=fuchsia_dir)
            self._run(
                [
                    "git",
                    "fetch",
                    "origin",
                    "main:refs/remotes/origin/main",
                ],
                cwd=fuchsia_dir,
            )
        else:
            self._run(
                [
                    "git",
                    "fetch",
                    "origin",
                    commit,
                ],
                self.cartfs_fuchsia_dir,
            )
            self._run(
                ["git", "reset", "--hard", commit], self.cartfs_fuchsia_dir
            )

        if backup_dir:
            logger.log_warn(
                "Restoring ignored files and directories from backup."
            )
            cartfs_rel_root = self.cartfs_dir.relative_to(
                self.cartfs_mount_point
            )

            # 1. Snapshot hardcoded large directories
            hardcoded_dirs = ["out", "prebuilt", ".cipd", ".fx", ".jiri_root"]
            for dir_name in hardcoded_dirs:
                from_rel = cartfs_rel_root / backup_dir.name / dir_name
                to_rel = (
                    cartfs_rel_root / self.cartfs_fuchsia_dir.name / dir_name
                )
                backup_path = backup_dir / dir_name
                target_path = self.cartfs_fuchsia_dir / dir_name

                if backup_path.exists():
                    try:
                        snapshotter.copy_cartfs_directory(from_rel, to_rel)
                    except Exception:
                        logger.log_exception(
                            f"Failed to restore {dir_name} via snapshot"
                        )
                        # Fallback to individual copy if it failed (e.g. ALREADY_EXISTS)
                        if backup_path.is_dir():
                            logger.log_warn(
                                f"Falling back to individual file copy for {dir_name}"
                            )
                            self._merge_directories(backup_path, target_path)

            # 2. Dynamically discover other ignored files
            try:
                ignored_output = subprocess.run(
                    [
                        "git",
                        "ls-files",
                        "--others",
                        "--ignored",
                        "--exclude-standard",
                    ],
                    cwd=backup_dir,
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout

                for line in ignored_output.splitlines():
                    path_str = line.strip()
                    if not path_str:
                        continue

                    # Skip if it belongs to hardcoded dirs
                    if any(
                        path_str.startswith(d + "/") for d in hardcoded_dirs
                    ):
                        continue

                    # Skip __pycache__
                    if "__pycache__" in path_str:
                        continue

                    backup_path = backup_dir / path_str
                    target_path = self.cartfs_fuchsia_dir / path_str

                    if backup_path.is_file():
                        if not target_path.exists():
                            try:
                                target_path.parent.mkdir(
                                    parents=True, exist_ok=True
                                )
                                shutil.copyfile(backup_path, target_path)
                            except Exception as e:
                                logger.log_error(
                                    f"Failed to copy file {path_str}: {e}"
                                )
            except subprocess.CalledProcessError as e:
                logger.log_error(f"Failed to list ignored files: {e}")

        # Couple of places in the build expect to find JIRI_HEAD for fuchsia
        # repository. In a normal checkout by jiri, the JIRI_HEAD is created
        # automatically.
        # https://fuchsia.googlesource.com/jiri/+/refs/heads/main/project/project.go#689
        # For our cartfs checkout, we need to create it manually.
        if not self.config.get("useJiriUpdateOnly", False):
            shutil.copyfile(
                self.cartfs_fuchsia_dir / ".git" / "HEAD",
                self.cartfs_fuchsia_dir / ".git" / "JIRI_HEAD",
            )

    def _merge_directories(self, src: Path, dst: Path) -> None:
        """Recursively copies files from src to dst, not overwriting existing files."""
        for item in src.iterdir():
            s = src / item.name
            d = dst / item.name
            if s.is_symlink():
                if not (d.exists() or d.is_symlink()):
                    try:
                        d.parent.mkdir(parents=True, exist_ok=True)
                        os.symlink(os.readlink(s), d)
                    except Exception as e:
                        logger.log_error(
                            f"Failed to copy symlink {s} to {d}: {e}"
                        )
            elif s.is_dir():
                self._merge_directories(s, d)
            else:
                if not d.exists():
                    try:
                        d.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copy2(s, d)
                    except Exception as e:
                        logger.log_error(f"Failed to copy {s} to {d}: {e}")

    def _create_symlinks(self) -> None:
        """Creates symlinks for the prebuilts."""
        logger.emit_status("Creating symlinks...")
        # Link the paths in the repo to cartfs
        (self.cartfs_fuchsia_dir / ".fx" / "config").mkdir(
            exist_ok=True, parents=True
        )

        def _get_path(path: str) -> Path:
            root, relative_path = path.split("//", 1)
            return {
                "@cartfs": self.cartfs_dir,
                "@cog": self.repo_dir,
            }[root] / relative_path

        for dest, src in self.config["symlinkMap"].items():
            self._create_symlink(_get_path(src), _get_path(dest))

        if self.config.get("useJiriUpdateOnly", False):
            # If we are using jiri update only, we don't need to run jiri hooks
            # manually since jiri will do it for us.
            return

        # Manually execute jiri hooks. The hooks are defined in
        # https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/manifests/platform#14
        # and are automatically executed by jiri during `jiri update`. Since we
        # are not using `jiri update`, we need to execute them manually.
        hooks = [
            "scripts/devshell/lib/add_symlink_to_bin.sh",
            "sdk/ctf/build/internal/create_ctf_releases_gni.sh",
            "build/info/create_jiri_hook_files.sh",
            "tools/build/scripts/generate_prebuilt_versions.sh",
            "tools/build/scripts/extract_protobuf_py3_wheel.sh",
            "tools/build/scripts/extract_pydantic_core_wheel.sh",
        ]

        for hook in hooks:
            self._run([hook], self.cartfs_fuchsia_dir)

        # Invoke git status in the fuchsia directory in the background. This
        # will make the future `fx format-code` command faster.
        subprocess.Popen(
            ["git", "status"],
            cwd=self.repo_dir,
            start_new_session=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def _run(
        self,
        cmd: list[str],
        cwd: Path,
        capture_output: bool = False,
    ) -> str:
        """Runs a command."""
        logger.log_debug(f"Running command: '{' '.join(cmd)}' in {cwd}")

        # Set FUCHSIA_DIR environment variable to the cartfs fuchsia directory.
        # This is needed for the hooks to work correctly.
        env = os.environ.copy()
        env["FUCHSIA_DIR"] = str(self.cartfs_fuchsia_dir)

        # If we are not debugging, we want to capture the output so we can print it on error.
        # If we are debugging, stdout/stderr are None, so output goes to stdout/stderr.
        run_capture_output = (
            capture_output or logger.get_log_level() > logging.DEBUG
        )

        process = subprocess.run(
            cmd,
            cwd=cwd,
            check=True,
            capture_output=run_capture_output,
            env=env,
        )
        return (
            process.stdout.decode("utf-8", errors="ignore")
            if capture_output
            else ""
        )
