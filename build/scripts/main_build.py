#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia build script.

This script serves as the primary entry point for executing builds in the
Fuchsia source tree. It manages the build environment, handles locking,
orchestrates top-level wrappers (like reproxy for RBE and rsproxy for
ResultStore), and prepares commands for various build tools including
Ninja, Bazel, and fint.

Responsibilities are divided into the following classes:
  * FuchsiaBuildContext: Static configuration and environment state.
  * BuildInvocation: Per-run state including unique IDs and log directories.
  * BuildCommandExecution: Orchestration of the final subprocess execution.
"""

import argparse
import dataclasses
import datetime
import functools
import getpass
import json
import os
import pathlib
import shlex
import shutil
import signal
import subprocess
import sys
import time
import uuid
from collections.abc import Iterable, Sequence
from typing import Any, TextIO

import signal_utils

_JSONPrimitive = str | int | float | bool | None
JSONValue = _JSONPrimitive | dict[str, Any] | list[Any]
JSONObject = dict[str, JSONValue]
JSONArray = list[JSONValue]

_SCRIPT = pathlib.Path(__file__)

# Use the executable of the active Python interpreter to ensure that all
# spawned sub-processes (like fint_build.py) run under the exact same
# interpreter environment.
PYTHON_BIN = pathlib.Path(sys.executable)


def msg(text: str, file: TextIO | None = None) -> None:
    """Print a message prefixed with the script's basename."""
    if file is None:
        file = sys.stdout
    print(f"[{_SCRIPT.name}] {text}", file=file)


def ts_msg(
    text: str, verbose: bool = False, file: TextIO | None = None
) -> None:
    """Print a message with high-precision epoch timestamps for verbose mode."""
    if file is None:
        file = sys.stderr
    if verbose:
        print(f"[{time.time():.9f}] [{_SCRIPT.name}]: {text}", file=file)


GLOBAL_RESULTSTORE_CONFIG = pathlib.Path(".fx/config/resultstore")
LOCAL_RESULTSTORE_CONFIG = pathlib.Path(".resultstore")
DEFAULT_RBE_INSTANCE = "projects/rbe-fuchsia-prod/instances/default"
DEFAULT_RESULTSTORE_INSTANCE = "projects/rbe-fuchsia-prod/instances/default"
DEFAULT_CAS_INSTANCE = "projects/rbe-fuchsia-prod/instances/default"
BAZEL_CRED_HELPER = pathlib.Path(
    "/google/src/head/depot/google3/devtools/blaze/bazel/credhelper/credhelper"
)


@dataclasses.dataclass
class BuildResult(object):
    return_code: int


def _parse_cfg_text(text: str) -> dict[str, str]:
    """Parses key-value pairs from a raw .cfg string, ignoring comments and stripping lines."""
    cfg_data = {}
    for line in text.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            key, sep, value = line.partition("=")
            if sep:
                cfg_data[key.strip()] = value.strip()
    return cfg_data


def _parse_cfg_file(config_path: pathlib.Path) -> dict[str, str]:
    """Safely reads a .cfg file and parses it into a dictionary of key-value pairs."""
    try:
        if config_path.is_file():
            return _parse_cfg_text(config_path.read_text())
    except Exception as e:
        # Speculative configuration parsing of user cfg files is non-critical to
        # core build orchestration. Print a diagnostic warning but proceed.
        msg(
            f"Warning: Failed to read RBE/ResultStore config file {config_path}: {e}",
            file=sys.stderr,
        )
    return {}


class BuildConfigurationError(Exception):
    """Raised when the build configuration is invalid or missing required files."""


@dataclasses.dataclass
class FuchsiaBuildConfig(object):
    """Configuration parameters for the build.

    Some of these configs can be inferred from $build_dir/args.gn.

    Fields:
      rbe: if True, build uses RBE
      resultstore: "all", "none", "ninja", or "bazel"
      profile: if True, collect system profile during build
      tui: if True, enable terminal UI for monitoring build
      verbose: if True, enable verbose logging output
      dry_run: if True, execute in dry-run mode
      status: if True, show build status metrics
      auth_mode: "auto", "user", "machine", or "none" (RBE/ResultStore auth)
      max_concurrency: upper bound on the automatically chosen -j, 0 for none
      fint_params_path: path to Fint static parameters if Fint wrapping is triggered
      fint_context_path: path to Fint context parameters if Fint wrapping is triggered
      output_metadata_json: path to write the structured metadata JSON of build artifacts
      remote_proxy_socket: path to the local RBE proxy socket if passed via CLI
      resultstore_proxy_socket: path to the local ResultStore/BES proxy socket if passed via CLI
      resultstore_instance: the target ResultStore instance name
      cas_instance: the target CAS instance name
      rbe_instance: the target RBE instance name
    """

    rbe: bool | None
    resultstore: str
    profile: bool
    tui: bool
    verbose: bool
    dry_run: bool
    auth_mode: str
    status: bool = True
    max_concurrency: int = 0
    fint_params_path: pathlib.Path | None = None
    fint_context_path: pathlib.Path | None = None
    output_metadata_json: pathlib.Path | None = None
    remote_proxy_socket: pathlib.Path | None = None
    resultstore_proxy_socket: pathlib.Path | None = None
    resultstore_instance: str | None = None
    cas_instance: str | None = None
    rbe_instance: str | None = None

    @staticmethod
    def from_args(
        args: argparse.Namespace, default_resultstore: str | None = None
    ) -> "FuchsiaBuildConfig":
        resultstore_val = args.resultstore or default_resultstore or "none"

        return FuchsiaBuildConfig(
            rbe=args.rbe,
            resultstore=resultstore_val,
            profile=args.profile,
            tui=args.tui,
            verbose=args.verbose,
            dry_run=args.dry_run,
            status=args.status,
            fint_params_path=args.fint_params_path,
            fint_context_path=args.fint_context_path,
            output_metadata_json=args.output_metadata_json,
            auth_mode=args.auth_mode,
            max_concurrency=args.max_concurrency,
            remote_proxy_socket=args.remote_proxy_socket,
            resultstore_proxy_socket=args.resultstore_proxy_socket,
            resultstore_instance=args.resultstore_instance,
            cas_instance=args.cas_instance,
            rbe_instance=args.rbe_instance,
        )


def check_shell_command(cmd: str) -> bool:
    return shutil.which(cmd) is not None


@functools.lru_cache()
def has_loas() -> bool:
    """Checks whether the host system supports LOAS/gcert authentication."""
    return check_shell_command("gcert") and check_shell_command("gcertstatus")


def _collect_rbe_metadata(log_dir: pathlib.Path) -> JSONObject:
    """Scans the active reproxy logs directory for diagnostic files and CAS logs.

    Args:
        log_dir: Path to the active build invocation's log directory.

    Returns:
        A dictionary containing paths to the reproxy log directory, diagnostic
        log files, CAS upload candidates (.rrpl/.rpl), and serialized proto logs.
    """
    rbe_metadata: JSONObject = {}
    reproxy_log_dir = log_dir / "reproxy_logs"
    if not reproxy_log_dir.exists():
        return rbe_metadata

    reproxy_log_dir_abs = reproxy_log_dir.resolve()
    rbe_metadata["log_dir"] = str(reproxy_log_dir_abs)

    diagnostic_logs = {}
    for name in ["bootstrap.INFO", "reproxy.INFO", "rbe_metrics.txt"]:
        candidate = reproxy_log_dir_abs / name
        if candidate.exists():
            diagnostic_logs[name] = str(candidate)
    if diagnostic_logs:
        rbe_metadata["diagnostic_logs"] = diagnostic_logs

    cas_candidates = []
    try:
        for item in reproxy_log_dir_abs.iterdir():
            if item.is_file() and item.suffix in (".rrpl", ".rpl"):
                cas_candidates.append(str(item.resolve()))
    except Exception:
        pass
    if cas_candidates:
        rbe_metadata["cas_upload_candidates"] = cas_candidates

    for pb_name, pb_filename in [
        ("rbe_metrics_pb", "rbe_metrics.pb"),
        ("reproxy_log_pb", "reproxy_log.pb"),
    ]:
        candidate = reproxy_log_dir_abs / pb_filename
        if candidate.exists():
            rbe_metadata[pb_name] = str(candidate)

    return rbe_metadata


def exists(path: pathlib.Path) -> bool:
    """Checks if a path exists."""
    return path.exists()


def is_executable(path: pathlib.Path) -> bool:
    """Checks if a path exists and is executable."""
    return path.exists() and os.access(path, os.X_OK)


def write_text(path: pathlib.Path, text: str) -> None:
    """Writes text to a file."""
    path.write_text(text)


def mkdir(
    path: pathlib.Path, parents: bool = True, exist_ok: bool = True
) -> None:
    """Creates a directory."""
    path.mkdir(parents=parents, exist_ok=exist_ok)


def read_json(path: pathlib.Path) -> Any:
    """Reads and parses a JSON file, raising BuildConfigurationError on failure."""
    if not exists(path):
        raise BuildConfigurationError(
            f"{path} does not exist. Make sure you have run 'fx set'."
        )
    try:
        with open(path) as f:
            return json.load(f)
    except json.JSONDecodeError as e:
        raise BuildConfigurationError(f"Failed to parse {path}: {e}")
    except Exception as e:
        raise BuildConfigurationError(f"Failed to read {path}: {e}")


def get_cpu_count() -> int:
    return os.cpu_count() or 1


def rbe_cpu_concurrency(rbe_enabled: bool) -> int:
    """Computes the build concurrency (-j) supported by the host CPU."""
    cpus = get_cpu_count()
    if rbe_enabled:
        # The recommendation from the Goma team is to use 10*cpu-count for C++.
        return cpus * 10
    return cpus


def ensure_file_descriptor_limit(limit: int) -> None:
    """Ensures the soft limit for file descriptors is at least 'limit'."""
    try:
        import resource
    except ImportError:
        return

    try:
        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        if soft < limit:
            resource.setrlimit(resource.RLIMIT_NOFILE, (limit, hard))
    except (ValueError, resource.error):
        pass


def _check_rbe_env_vars(environ: dict[str, str]) -> None:
    """Warns if environment variables starting with 'RBE_' are set."""
    rbe_vars = sorted([k for k in environ if k.startswith("RBE_")])
    if rbe_vars:
        msg(
            f"Warning: The following environment variables starting with 'RBE_' "
            f"are set and may override RBE tool configurations: {', '.join(rbe_vars)}"
        )


def str_to_resultstore(value: str) -> str:
    val_lower = value.lower()
    if val_lower in ("true", "1", "yes", "all"):
        return "all"
    elif val_lower in ("false", "0", "no", "none"):
        return "none"
    elif val_lower in ("ninja", "bazel"):
        return val_lower
    raise argparse.ArgumentTypeError(
        f"Invalid --resultstore value: '{value}'. Expected true/false/all/none/ninja/bazel."
    )


def normalize_resultstore_value(val: Any) -> str:
    if val is True or val == "all":
        return "all"
    elif val is False or val is None or val == "none":
        return "none"
    elif val in ("ninja", "bazel"):
        return val
    return "none"


def parse_properties(content: str) -> dict[str, str]:
    """Parses a simple KEY=VALUE properties string, supporting export syntax."""
    props: dict[str, str] = {}
    for line in content.splitlines():
        line = line.strip()
        if not line or line.startswith("//"):
            continue
        k, sep, v = line.partition("=")
        if sep:
            key = k.strip().removeprefix("export ").strip()
            if key:
                try:
                    tokens = shlex.split(v.strip(), comments=True)
                except ValueError:
                    continue
                if tokens:
                    props[key] = tokens[0]
    return props


def load_properties(path: pathlib.Path) -> dict[str, str]:
    """Reads a simple KEY=VALUE properties file safely, supporting export syntax."""
    if not exists(path):
        return {}
    try:
        content = path.read_text()
    except OSError:
        return {}
    return parse_properties(content)


def load_user_preference(path: pathlib.Path) -> str | None:
    """Loads resultstore preference, supporting both new and legacy formats."""
    props = load_properties(path)
    if "resultstore" in props:
        return normalize_resultstore_value(props["resultstore"])
    elif "RESULTSTORE_ENABLED" in props:
        # Legacy binary representation: 1=all, 0=none
        val = props["RESULTSTORE_ENABLED"]
        if val == "1":
            return "all"
        elif val == "0":
            return "none"
    return None


def gcp_instance_name(value: str) -> str:
    """Validates and returns a 4-component GCP instance name.

    Expected format: 'projects/<project-id>/instances/<instance-name>'
    """
    parts = value.split("/")
    if (
        len(parts) != 4
        or parts[0] != "projects"
        or parts[2] != "instances"
        or not parts[1]
        or not parts[3]
    ):
        raise argparse.ArgumentTypeError(
            f"Invalid instance format '{value}'. "
            "Expected format: 'projects/<project-id>/instances/<instance-name>'"
        )
    return value


def str_to_bool(value: str) -> bool:
    if isinstance(value, bool):
        return value
    if value.lower() in ("true", "1", "yes"):
        return True
    elif value.lower() in ("false", "0", "no"):
        return False
    raise argparse.ArgumentTypeError(f"Boolean value expected, got {value}")


class BuildLock:
    """This context-manager ensures at most one build is running per build dir.

    Importantly, it prints a lock-acquired message so noninteractive agents
    understand what is happening.
    """

    def __init__(
        self, build_dir: pathlib.Path, print_message: bool = False
    ) -> None:
        # LINT.IfChange(build_lock)
        self.build_lock_file = pathlib.Path(f"{build_dir}.build_lock")
        # LINT.ThenChange(//tools/devshell/lib/vars.sh:build_lock)
        self._has_shlock = check_shell_command("shlock")
        self.print_message = print_message

    def __enter__(self) -> "BuildLock":
        if self._has_shlock:
            while (
                subprocess.call(
                    [
                        "shlock",
                        "-f",
                        str(self.build_lock_file),
                        "-p",
                        str(os.getpid()),
                    ]
                )
                != 0
            ):
                time.sleep(0.1)

            if self.print_message:
                # This message is critical for AI agents to understand when a build
                # is proceeding after acquiring a lock. Do not remove.
                print("Lock acquired, proceeding with build.")
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        if self._has_shlock:
            self.build_lock_file.unlink(missing_ok=True)
        if self.print_message:
            print("Build completed.")


def resolve_source_dir(environ: dict[str, str]) -> pathlib.Path:
    """Resolves and returns the absolute path to the Fuchsia source checkout directory."""
    source_dir_str = environ.get("FUCHSIA_DIR", "")
    if source_dir_str:
        return pathlib.Path(source_dir_str).resolve()
    try:
        return find_fuchsia_dir()
    except ValueError:
        # Fallback to finding it relative to this script: //build/scripts/main_build.py
        return _SCRIPT.resolve().parent.parent.parent


class FuchsiaBuildContext(object):
    """FuchsiaBuildContext contains paths that are relevant to building.

    Fields:
      source_dir: location of source checkout (absolute)
      out_dir: write-able location where logs may be written (absolute)
      build_dir: where build is executed and artifacts are created (absolute)
      env: environment variables
      config: build parameters
    """

    def __init__(
        self,
        source_dir: pathlib.Path,
        out_dir: pathlib.Path,
        build_dir: pathlib.Path,
        env: dict[str, str],
        config: FuchsiaBuildConfig,
    ) -> None:
        self.source_dir = source_dir
        self.out_dir = out_dir
        self.build_dir = build_dir
        self.env = env
        self.config = config

    @functools.cached_property
    def authenticated_user(self) -> str:
        """Resolves and caches the user identity for RBE/ResultStore authentication.

        Returns:
            The resolved user name string.

        Raises:
            BuildConfigurationError: If the user cannot be resolved and
              gcert authentication is required.
        """
        user = None
        if "USER" in self.env:
            user = self.env["USER"]
        else:
            try:
                user = getpass.getuser()
            except Exception:
                pass

        if not user:
            if self.loas_type != "skip":
                raise BuildConfigurationError(
                    "USER environment variable is not set and could not be "
                    "inferred. This is required for RBE/ResultStore LOAS/gcert authentication."
                )
            user = "builder"
        return user

    @property
    def auth_env(self) -> dict[str, str]:
        """Returns a dictionary of authentication-related environment variables."""
        env: dict[str, str] = {}
        env["USER"] = self.authenticated_user

        if not self.needs_auth:
            return env

        env["FX_BUILD_LOAS_TYPE"] = self.loas_type

        # Forward Google Application Credentials if present or fallback to defaults safely
        if "GOOGLE_APPLICATION_CREDENTIALS" in self.env:
            env["GOOGLE_APPLICATION_CREDENTIALS"] = self.env[
                "GOOGLE_APPLICATION_CREDENTIALS"
            ]
        elif self.resolved_auth_mode != "machine":
            try:
                default_adc = (
                    pathlib.Path.home()
                    / ".config/gcloud/application_default_credentials.json"
                )
                env["GOOGLE_APPLICATION_CREDENTIALS"] = str(default_adc)
            except (RuntimeError, KeyError):
                pass

        # Forward GCE metadata host overrides to support Java/Bazel inside sandboxes.
        # This automatically translates standard GCE_METADATA_HOST to the Java-specific
        # G_CLOUD_METADATA_HOST and GCLOUD_METADATA_HOST variables, allowing Bazel to
        # natively route credentials requests to our local metadata proxy.
        if "GCE_METADATA_HOST" in self.env:
            gce_host = self.env["GCE_METADATA_HOST"]
            env["GCE_METADATA_HOST"] = gce_host
            env["G_CLOUD_METADATA_HOST"] = gce_host
            env["GCLOUD_METADATA_HOST"] = gce_host

        return env

    @property
    def build_service_env(self) -> dict[str, str]:
        """Returns a dictionary of environment variables related to build-service proxying/overrides."""
        env: dict[str, str] = {}

        # Override the remote execution and resultstore proxy endpoints with
        # unix:// socket paths if explicit CLI sockets are passed.
        # LINT.IfChange(bazel_socket_env_vars)
        if self.config.remote_proxy_socket:
            socket_str = str(self.config.remote_proxy_socket)
            # Override for reproxy, through build/rbe/fuchsia-reproxy-wrap.sh:
            env["RBE_service"] = f"unix://{socket_str}"
            # Override for rsproxy, through build/resultstore/fuchsia-rsproxy-wrap.sh:
            env["RS_cas_service"] = f"unix://{socket_str}"
            # Consumed by build/bazel/scripts/generate_invocation_bazelrc.py to route Bazel remote traffic
            env["FX_INTERNAL_BAZEL_RBE_SOCKET_PATH"] = socket_str

        if self.config.resultstore_proxy_socket:
            socket_str = str(self.config.resultstore_proxy_socket)
            # Override for rsproxy, through build/resultstore/fuchsia-rsproxy-wrap.sh:
            env["RS_rs_service"] = f"unix://{socket_str}"
            # Consumed by build/bazel/scripts/generate_invocation_bazelrc.py to route Bazel resultstore traffic
            env["FX_INTERNAL_BAZEL_RESULTSTORE_SOCKET_PATH"] = socket_str
        # LINT.ThenChange(
        #   //build/bazel/scripts/generate_invocation_bazelrc.py:bazel_socket_env_vars,
        #   //build/bazel_sdk/tests/scripts/bazel_test.py:bazel_socket_env_vars,
        #   //build/resultstore/fuchsia-rsproxy-wrap.sh:rs_service_env_vars
        # )

        # Forward ResultStore/CAS instance names passed from CLI arguments only if they differ from on-disk configured defaults.
        # These RS_ variables directly drive/influence the rsproxy daemon.
        # LINT.IfChange(rs_instance_env_vars)
        if self.config.resultstore_instance:
            if (
                self.config.resultstore_instance
                != self.resolved_resultstore_instance
            ):
                env["RS_rs_instance"] = self.config.resultstore_instance
        if self.config.cas_instance:
            if self.config.cas_instance != self.resolved_cas_instance:
                env["RS_cas_instance"] = self.config.cas_instance
        # LINT.ThenChange(//build/resultstore/fuchsia-rsproxy-wrap.sh:rs_instance_env_vars)

        # Forward RBE instance name passed from CLI arguments only if it differs from the on-disk configured default.
        if self.config.rbe_instance:
            if self.config.rbe_instance != self.resolved_rbe_instance:
                env["RBE_instance"] = self.config.rbe_instance

        return env

    @staticmethod
    def from_args(
        args: argparse.Namespace,
        environ: dict[str, str],
    ) -> "FuchsiaBuildContext":
        source_dir = resolve_source_dir(environ)

        out_dir = args.out_dir
        if not out_dir:
            out_dir = source_dir / "out"

        # Resolve sticky user preferences (local overrides global)
        resultstore_pref = None
        global_config = source_dir / GLOBAL_RESULTSTORE_CONFIG
        local_config = args.build_dir / LOCAL_RESULTSTORE_CONFIG

        global_pref = load_user_preference(global_config)
        if global_pref:
            resultstore_pref = global_pref

        local_pref = load_user_preference(local_config)
        if local_pref:
            resultstore_pref = local_pref

        return FuchsiaBuildContext(
            source_dir=source_dir,
            out_dir=out_dir,
            build_dir=args.build_dir,
            env=environ,
            config=FuchsiaBuildConfig.from_args(
                args, default_resultstore=resultstore_pref
            ),
        )

    @functools.cached_property
    def resolved_rbe_instance(self) -> str:
        """Finds the default RBE instance configured on disk."""
        config_path = self.source_dir / "build/rbe/fuchsia-reproxy.cfg"
        cfg = _parse_cfg_file(config_path)
        return cfg.get("instance", DEFAULT_RBE_INSTANCE)

    @functools.cached_property
    def resolved_resultstore_instance(self) -> str:
        """Finds the default ResultStore instance configured on disk."""
        config_path = (
            self.source_dir / "build/resultstore/fuchsia-resultstore.cfg"
        )
        cfg = _parse_cfg_file(config_path)
        return cfg.get("rs_instance", DEFAULT_RESULTSTORE_INSTANCE)

    @functools.cached_property
    def resolved_cas_instance(self) -> str:
        """Finds the default CAS instance configured on disk."""
        config_path = (
            self.source_dir / "build/resultstore/fuchsia-resultstore.cfg"
        )
        cfg = _parse_cfg_file(config_path)
        return cfg.get("cas_instance", DEFAULT_CAS_INSTANCE)

    @property
    def rbe_settings_file(self) -> pathlib.Path:
        return self.build_dir / "rbe_settings.json"

    @property
    def fint_build_py(self) -> pathlib.Path:
        return self.source_dir / "tools/integration/fint/fint_build.py"

    def _fint_wrapper_cmd(
        self,
        static_path: pathlib.Path | None = None,
        context_path: pathlib.Path | None = None,
        print_artifact_dir: bool = False,
        ninja_error_logging_output: pathlib.Path | None = None,
    ) -> Iterable[str]:
        """Constructs and yields command-line arguments for executing fint_build.py.

        Args:
            static_path: Path to the Fint static parameters textproto.
            context_path: Path to the Fint context parameters textproto.
            print_artifact_dir: If True, appends the query flag to print the
              artifact directory path and exits instead of running the build.
            ninja_error_logging_output: Path where Ninja should write its error logs (ninja_errors.json).
        """
        yield str(PYTHON_BIN)
        yield "-S"
        yield "-u"
        yield str(self.fint_build_py)

        if static_path:
            yield "--static"
            yield str(static_path)
        if context_path:
            yield "--context"
            yield str(context_path)
        if self.config.verbose:
            yield "--verbose"
        if print_artifact_dir:
            yield "--print-artifact-dir"
        else:
            if ninja_error_logging_output:
                yield "--ninja-error-logging-output"
                yield str(ninja_error_logging_output)
            yield "--"

    def fint_build_cmd(
        self, ninja_error_logging_output: pathlib.Path | None = None
    ) -> Iterable[str]:
        """Constructs and yields command-line arguments for standard Fint build execution."""
        if not self.config.fint_params_path:
            return
        yield from self._fint_wrapper_cmd(
            static_path=self.config.fint_params_path,
            context_path=self.config.fint_context_path,
            ninja_error_logging_output=ninja_error_logging_output,
        )

    @functools.cached_property
    def fint_artifact_dir(self) -> pathlib.Path | None:
        """Parses and returns the Fint artifact directory path from context parameters if specified."""
        if not self.config.fint_context_path:
            return None

        try:
            cmd = list(
                self._fint_wrapper_cmd(
                    context_path=self.config.fint_context_path,
                    print_artifact_dir=True,
                )
            )
            output = subprocess.check_output(
                cmd,
                text=True,
                stderr=subprocess.PIPE,
            ).strip()
            if output:
                return pathlib.Path(output)
        except (subprocess.CalledProcessError, OSError) as e:
            msg(
                f"Failed to delegate artifact_dir parsing to fint_build.py: {e}",
                file=sys.stderr,
            )
        return None

    @property
    def rbe_config_json(self) -> pathlib.Path:
        return self.build_dir / "rbe_config.json"

    @property
    def check_loas_script(self) -> pathlib.Path:
        return self.source_dir / "build/auth/check_loas_restrictions.sh"

    @property
    def top_build_wrapper(self) -> pathlib.Path:
        return self.source_dir / "build/scripts/top_build_wrap.sh"

    @property
    def args_gn(self) -> pathlib.Path:
        return self.build_dir / "args.gn"

    @property
    def gn_trace_path(self) -> pathlib.Path:
        """Returns the path to the GN-generated trace file."""
        return self.build_dir / "fuchsia_gn_trace.json"

    @property
    def rsninja_sh(self) -> pathlib.Path:
        return self.source_dir / "build/resultstore/rsninja.sh"

    @property
    def ninja_edge_weights_csv(self) -> pathlib.Path:
        return self.build_dir / "ninja_edge_weights.csv"

    def get_rbe_reproxy_configs(self) -> Iterable[pathlib.Path]:
        """Yields the paths to the RBE reproxy configuration files."""
        for cfg in self._rbe_config_data:
            yield self.build_dir / cfg["path"]

    @functools.cached_property
    def _rbe_config_data(self) -> Iterable[dict[str, Any]]:
        """Read and parse RBE config data."""
        data = read_json(self.rbe_config_json)
        if isinstance(data, list):
            return data
        return []

    @functools.cached_property
    def _rbe_settings(self) -> dict[str, Any]:
        """Automatically detect RBE usage from a GN-generated JSON file."""
        return read_json(self.rbe_settings_file)

    @property
    def rbe_enabled(self) -> bool:
        if self.config.rbe is not None:
            return self.config.rbe

        return self._rbe_settings.get("final", {}).get("needs_reproxy", False)

    @property
    def needs_auth(self) -> bool:
        if self.config.resultstore != "none":
            return True

        return self._rbe_settings.get("final", {}).get("needs_auth", False)

    @functools.cached_property
    def resolved_auth_mode(self) -> str:
        """Resolves the concrete authentication mode (user, machine, or none)."""
        mode = self.config.auth_mode
        if mode == "none" or not self.needs_auth:
            return "none"

        if mode == "auto":
            # Auto-detect if we are in an infra/bot environment
            infra_env_hints = ["BUILDBUCKET_ID", "SWARMING_TASK_ID"]
            if any(hint in self.env for hint in infra_env_hints):
                return "machine"
            return "user"

        return mode

    @property
    def concurrency(self) -> int:
        """The -j value to use, the minimum over all limiting factors."""
        factors = [rbe_cpu_concurrency(self.rbe_enabled)]
        if self.config.max_concurrency > 0:
            factors.append(self.config.max_concurrency)
        return min(factors)

    @functools.cached_property
    def loas_type(self) -> str:
        """Automatically detect the LOAS type."""
        if self.resolved_auth_mode == "none":
            return "skip"

        if self.resolved_auth_mode == "machine":
            return "skip"

        # Under 'user' mode: if we have LOAS, detect the specific restriction level
        if has_loas():
            check_loas_script = self.check_loas_script
            if is_executable(check_loas_script):
                try:
                    output = subprocess.check_output(
                        [str(check_loas_script)],
                        text=True,
                        stderr=subprocess.DEVNULL,
                        env=self.env,
                    )
                    lines = output.strip().splitlines()
                    if lines:
                        loas_type = lines[-1]
                        if loas_type == "unrestricted":
                            # In corporate environments, an unrestricted LOAS type implies we can use
                            # Bazel's gcert credential helper. However, if that helper is not executable or
                            # accessible (e.g. on new systems, cloudtops with unmounted SrcFS, or due to
                            # specific subdirectory permission/ACL restrictions), any attempt to run Bazel
                            # with --config=gcertauth will fail immediately with permission denied.
                            # In these circumstances, we gracefully downgrade to "restricted" mode to fall
                            # back to local gcloud Application Default Credentials (ADC).
                            if not is_executable(BAZEL_CRED_HELPER):
                                msg(
                                    f"WARNING: Bazel credential helper on SrcFS is not accessible: {BAZEL_CRED_HELPER}",
                                    file=sys.stderr,
                                )
                                msg(
                                    "WARNING: Gracefully falling back to restricted (local gcloud/ADC) mode.",
                                    file=sys.stderr,
                                )
                                return "restricted"
                        return loas_type
                except subprocess.CalledProcessError:
                    pass
            return "skip"

        # If no LOAS is available but we are in 'user' mode, we must be using local OAuth/ADC
        return "skip"


class BuildInvocation(object):
    """BuildInvocation represents a single build run.

    It encapsulates the FuchsiaBuildContext, and is responsible
    for generating a unique ID, and creating a single-use log directory.
    """

    def __init__(self, context: FuchsiaBuildContext) -> None:
        self.context = context
        # Accessing log_dir triggers the cached_property evaluation,
        # which creates the directory and writes the invocation_id.
        _ = self.log_dir

    def write_metadata_json(self, output_path: pathlib.Path) -> None:
        """Serializes build log paths and metadata to a structured JSON file.

        Enables downstream build recipes to dynamically resolve and upload logs
        (like RBE and ResultStore) without hardcoding source-tree path patterns.

        Args:
            output_path: Path where the structured metadata JSON will be written.
        """
        context = self.context
        log_dir = self.log_dir

        fint_build_artifacts = None
        artifact_dir = context.fint_artifact_dir
        if artifact_dir:
            candidate = artifact_dir / "build_artifacts.json"
            if candidate.exists():
                fint_build_artifacts = str(candidate.resolve())

        gn_trace = None
        gn_trace_candidate = context.gn_trace_path
        if gn_trace_candidate.exists():
            gn_trace = str(gn_trace_candidate.resolve())

        metadata: JSONObject = {}
        if fint_build_artifacts:
            metadata["fint_build_artifacts"] = fint_build_artifacts
        if gn_trace:
            metadata["gn_trace"] = gn_trace

        # LINT.IfChange(build_profile_filenames)
        profile_dir = log_dir / "build_profile"
        system_profile_path = profile_dir / "system_profile.json"
        hardware_profile_path = profile_dir / "hardware_profile.json"

        build_profile: JSONObject = {}
        if system_profile_path.exists():
            build_profile["system_profile"] = str(system_profile_path.resolve())
        if hardware_profile_path.exists():
            build_profile["hardware_profile"] = str(
                hardware_profile_path.resolve()
            )

        if build_profile:
            metadata["build_profile"] = build_profile
        # LINT.ThenChange(//build/scripts/top_build_wrap.sh:build_profile_filenames)

        rbe_metadata = _collect_rbe_metadata(log_dir)
        if rbe_metadata:
            metadata["rbe"] = rbe_metadata

        try:
            mkdir(output_path.parent)
            with open(output_path, "w") as f:
                json.dump(metadata, f, indent=2)
                f.write("\n")
        except Exception as e:
            msg(f"Failed to write metadata JSON: {e}", file=sys.stderr)

    @functools.cached_property
    def build_uuid(self) -> str:
        """Generates a unique ID for this build."""
        return str(uuid.uuid4())

    @functools.cached_property
    def timestamp(self) -> str:
        """Returns the timestamp for the start of this build."""
        return datetime.datetime.now().strftime("%Y%m%d-%H%M%S")

    # LINT.IfChange(build_log_dir_structure)
    @functools.cached_property
    def log_dir(self) -> pathlib.Path:
        """Creates and returns the log directory for this invocation.

        The invocation id is recorded in the new log directory
        in a file named "invocation_id".
        """
        logs_root = self.context.out_dir / "_build_logs"
        build_dir_name = self.context.build_dir.name
        log_dir_base = logs_root / build_dir_name
        mkdir(log_dir_base)

        # Use consistent UUID and timestamp
        log_dir = log_dir_base / f"build.{self.timestamp}.{self.build_uuid[:8]}"
        mkdir(log_dir)

        # Record the invocation Id
        write_text(log_dir / "invocation_id", self.build_uuid + "\n")
        return log_dir

    # LINT.ThenChange(//tools/devshell/lib/vars.sh:build_log_dir_structure)

    @functools.cached_property
    def temp_home(self) -> pathlib.Path:
        """Returns a temporary fallback directory to use as $HOME.

        This is intended solely as an ephemeral, non-persistent workaround to
        support user-less, home-less environments (such as sandboxed infra
        containers in b/559830870) where $HOME is not set and the current UID
        does not have an entry in /etc/passwd (preventing standard tool lookups
        from falling back successfully).
        """
        path = self.log_dir / ".home"
        mkdir(path)
        return path

    @property
    def ninja_errors_path(self) -> pathlib.Path:
        """The path where Ninja-specific structured action failures are recorded."""
        return self.log_dir / "ninja_errors.json"

    def top_build_command_prefix(self) -> Iterable[str]:
        """Construct the prefix command for the top-level wrapper."""
        context = self.context
        # top_build_wrapper is a wrapper orchestrator whose purpose is to
        # auto-start/stop processes around the build.
        yield str(context.top_build_wrapper)

        if context.config.dry_run:
            yield "--dry-run"

        if context.config.tui:
            yield "--tui"

        if context.rbe_enabled:
            yield "--rbe"
            for cfg_path in context.get_rbe_reproxy_configs():
                yield "--reproxy-cfg"
                yield str(cfg_path)

        # LOAS handling
        yield "--loas-type"
        yield context.loas_type
        if context.resolved_auth_mode == "machine":
            yield "--use-machine-credentials"

        # Log directory setup
        yield "--build-dir"
        yield str(context.build_dir)
        yield "--log-dir"
        yield str(self.log_dir)

        if context.config.resultstore in ("all", "ninja"):
            yield "--resultstore"
            args_gn = context.args_gn
            if exists(args_gn):
                yield "--pre-build-uploads"
                yield str(args_gn)

        if context.config.profile:
            yield "--profile"

        if context.config.verbose:
            yield "--verbose"

    def get_build_env(self) -> dict[str, str]:
        """Curate a build environment for this invocation."""
        build_env = {
            "FX_BUILD_UUID": self.build_uuid,
            "FX_BUILD_LOGDIR": str(self.log_dir),
            "TERM": self.context.env.get(
                "TERM", "dumb"
            ),  # passed for the pretty ninja UI
            "PATH": self.context.env.get(
                "PATH", ""
            ),  # passed through. The ninja actions should invoke tools without relying on PATH.
            # Ensure HOME is defined to support user-less/home-less environments (b/559830870)
            "HOME": self.context.env.get("HOME") or str(self.temp_home),
            # By default, also show the number of actively running actions.
            "NINJA_STATUS": self.context.env.get(
                "NINJA_STATUS", "[%f/%t][%p/%w](%r) "
            ),
            # By default, print the 4 oldest commands that are still running.
            "NINJA_STATUS_MAX_COMMANDS": self.context.env.get(
                "NINJA_STATUS_MAX_COMMANDS", "4"
            ),
            "NINJA_STATUS_REFRESH_MILLIS": self.context.env.get(
                "NINJA_STATUS_REFRESH_MILLIS", "100"
            ),
            "NINJA_PERSISTENT_MODE": self.context.env.get(
                "NINJA_PERSISTENT_MODE", "0"
            ),
            "PYTHONPYCACHEPREFIX": self.context.env.get(
                "PYTHONPYCACHEPREFIX",
                str(self.context.build_dir / "__pycache__"),
            ),
        }

        if not self.context.config.status:
            # Setting TERM=dumb is a standard convention to turn off interactive features.
            # See https://en.wikipedia.org/wiki/Computer_terminal#Dumb_terminals
            # and https://developers.google.com/style/inclusive-documentation#ableist-language
            # regarding non-inclusive language.
            build_env["TERM"] = "dumb"
            build_env["NINJA_STATUS"] = "[%f/%t] "

        # Forwarded standard variables
        forward_vars = [
            "SSH_AUTH_SOCK",  # need to forward the authentication socket (used by gnubby) for bazel
            "MAKEFLAGS",
            "TMPDIR",  # was passed for Goma on macOS, but it might have other uses.
            "CLICOLOR_FORCE",
            "FUCHSIA_BAZEL_DISK_CACHE",
            "FUCHSIA_BAZEL_DISK_CACHE_SIZE",
            "FUCHSIA_BAZEL_JOB_COUNT",
            "FUCHSIA_BAZEL_PRINT_COMMANDS",
            "FUCHSIA_DEBUG_BAZEL_SANDBOX",
            "NINJA_PERSISTENT_TIMEOUT_SECONDS",
            "NINJA_PERSISTENT_LOG_FILE",
            "FX_BUILD_RBE_STATS",
            "FX_BUILD_QUIET",
            "FX_REMOTE_BUILD_METRICS",  # Honor environment variable to disable RBE build metrics.
            "BUILDBUCKET_ID",  # Propagated in infra environments to tag ResultStore/BES uploads
            "BUILDBUCKET_BUILDER",
            "SWARMING_TASK_ID",
        ]
        for var in forward_vars:
            if var in self.context.env:
                build_env[var] = self.context.env[var]

        build_env.update(self.context.auth_env)

        # Inject ResultStore induction signals to allow passive wrappers to dynamically
        # configure themselves at runtime.
        resultstore = self.context.config.resultstore
        # LINT.IfChange(resultstore_ninja_env_vars)
        build_env["FX_INTERNAL_RESULTSTORE_NINJA"] = (
            "1" if resultstore in ("all", "ninja") else "0"
        )
        # LINT.ThenChange(//build/resultstore/fuchsia-rsproxy-wrap.sh:resultstore_ninja_env_vars)

        # LINT.IfChange(resultstore_bazel_env_vars)
        resultstore_bazel = "0"
        if resultstore in ("all", "bazel"):
            # Decide between "resultstore" (developer) and "resultstore_infra" (infra).
            if (
                self.context.loas_type == "unrestricted"
                and "BUILDBUCKET_ID" not in self.context.env
            ):
                resultstore_bazel = "resultstore"
            else:
                resultstore_bazel = "resultstore_infra"
        build_env["FX_INTERNAL_RESULTSTORE_BAZEL"] = resultstore_bazel
        # LINT.ThenChange(//build/bazel/wrapper.bazel.sh:resultstore_bazel_env_vars)

        build_env.update(self.context.build_service_env)

        return build_env

    def new_build_command_execution(
        self,
        command_type: str,
        build_command: list[str],
        is_build: bool = True,
    ) -> "BuildCommandExecution":
        """Creates a self-contained BuildCommandExecution."""
        build_env = self.get_build_env()
        context = self.context

        # For non-build commands (e.g. 'bazel query' or 'bazel info'), we bypass
        # top_build_wrap.sh completely since no remote execution (reproxy) or
        # ResultStore (rsproxy) background wrappers/daemons are required.
        if not is_build:
            return BuildCommandExecution(
                full_command=list(build_command),
                env=build_env,
                invocation=self,
                is_build=is_build,
            )

        top_cmd = list(self.top_build_command_prefix())
        resultstore_post_build_uploads = []

        if context.config.resultstore in ("all", "ninja"):
            if command_type == "ninja":
                ninja_log_dir = self.log_dir / "ninja_logs"
                resultstore_post_build_uploads.extend(
                    [
                        ninja_log_dir / "ninja_action_metrics.json",
                        ninja_log_dir / "ninja_dirty_sources.log",
                        context.ninja_edge_weights_csv,
                    ]
                )

        top_cmd.extend(
            arg
            for f in resultstore_post_build_uploads
            for arg in ("--post-build-uploads", str(f))
        )

        # Prepare Ninja-specific options
        if command_type == "ninja":
            build_command = self._inject_ninja_args(build_command)

        fint_cmd = list(
            context.fint_build_cmd(
                ninja_error_logging_output=self.ninja_errors_path
            )
        )
        if fint_cmd:
            build_command = fint_cmd + list(build_command)

        full_cmd = top_cmd + ["--"] + list(build_command)

        return BuildCommandExecution(
            full_command=full_cmd,
            env=build_env,
            invocation=self,
            is_build=is_build,
        )

    def _inject_ninja_args(
        self,
        build_command: list[str],
    ) -> list[str]:
        """Return new build command with Ninja-specific flags injected in the right place."""
        ninja_log_dir = self.log_dir / "ninja_logs"
        mkdir(ninja_log_dir)

        # Record the set of inputs that triggered build actions.
        dirty_sources = ninja_log_dir / "ninja_dirty_sources.log"
        # Record action count metrics.
        action_metrics = ninja_log_dir / "ninja_action_metrics.json"
        # Record structured action failures
        error_logging_output = self.ninja_errors_path

        ninja_bin = build_command[0]
        remaining_args = build_command[1:]
        return [
            ninja_bin,
            "--dirty_sources_list",
            str(dirty_sources),
            "--action_metrics_output",
            str(action_metrics),
            "--error_logging_output",
            str(error_logging_output),
        ] + list(remaining_args)


@dataclasses.dataclass
class BuildCommandExecution(object):
    """BuildCommandExecution represents a single build command.

    To execute the build command, call .run().

    Fields:
      full_command: shell command tokens
      env: environment variables
      invocation: the build invocation this execution belongs to
      cleanup_files: list of files to remove after execution
    """

    full_command: Sequence[str]
    env: dict[str, str]
    invocation: BuildInvocation
    is_build: bool = True
    cleanup_files: list[pathlib.Path] = dataclasses.field(default_factory=list)

    def _run_without_locking(self) -> BuildResult:
        """Execute the build command."""
        config = self.invocation.context.config
        if config.verbose or config.dry_run:
            env_str = " ".join(
                f"{k}={shlex.quote(v)}" for k, v in sorted(self.env.items())
            )
            msg(
                f"Running: {env_str} {' '.join(shlex.quote(c) for c in self.full_command)}"
            )

        if config.verbose:
            auth_summary = (
                f"\n[Auth Configuration Resolved]:\n"
                f"  Auth Mode:       {self.invocation.context.resolved_auth_mode}\n"
                f"  LOAS Type:       {self.invocation.context.loas_type}\n"
                f"  Auth User:       {self.invocation.context.authenticated_user}\n"
                f"  Metadata Host:   {self.env.get('GCE_METADATA_HOST', 'NOT_SET')}\n"
                f"  Java Metadata:   {self.env.get('G_CLOUD_METADATA_HOST', 'NOT_SET')}\n"
                f"  G-A-C Path:      {self.env.get('GOOGLE_APPLICATION_CREDENTIALS', 'NOT_SET')}\n"
                f"--------------------------------------------------"
            )
            msg(auth_summary)
        # Note: when config.dry_run is set, we still execute the command,
        # but we have forwarded --dry-run to the top_build_wrapper, which
        # will skip the actual build execution. This allows for high-fidelity
        # verification of the entire wrapper orchestration stack.

        # If the TUI is enabled, we MUST NOT use a separate process group,
        # otherwise the TUI will be suspended when it tries to interact with
        # the terminal (SIGTTIN/SIGTTOU).
        use_separate_pgrp = not config.tui

        # Use SignalManagedProcess to handle the entire lifecycle:
        # 1. Setup the child (un-ignore signals, optionally isolate PGID).
        # 2. Spawn the process.
        # 3. Forward signals while waiting for termination.
        #
        # Note: We must stay alive to maintain the build lock and cleanup files.
        managed = signal_utils.SignalManagedProcess(
            self.full_command,
            env=self.env,
            separate_pgrp=use_separate_pgrp,
            verbose=config.verbose,
        )

        ts_msg("Starting build command execution...", config.verbose)
        return_code = -1
        try:
            return_code = managed.run()
        finally:
            ts_msg(
                f"Build command execution completed with status {return_code}",
                config.verbose,
            )

        return BuildResult(return_code=return_code)

    def run(self) -> BuildResult:
        """Execute the build command, guarded by a build lock if it is a build command.

        Returns:
          exit code of the command, 0 for success.
        """
        try:
            if self.is_build:
                quiet = os.getenv("FX_BUILD_QUIET") == "1"
                with BuildLock(
                    self.invocation.context.build_dir, print_message=quiet
                ):
                    return self._run_without_locking()
            else:
                return self._run_without_locking()
        finally:
            for f in self.cleanup_files:
                f.unlink(missing_ok=True)


# TODO: De-duplicate with find_fuchsia_dir in //build/bazel/scripts/build_utils.py.
def find_fuchsia_dir(from_path: pathlib.Path | None = None) -> pathlib.Path:
    """Find the Fuchsia checkout from a specific path.

    Args:
        from_path: Optional starting path for search. Defaults to the current directory.
    Returns:
        Path to the Fuchsia checkout directory (absolute).
    Raises:
        ValueError if the path could not be found.
    """
    start_path = from_path.resolve() if from_path else pathlib.Path.cwd()
    cur_path = start_path
    while True:
        if exists(cur_path / ".jiri_manifest"):
            return cur_path
        prev_path = cur_path
        cur_path = cur_path.parent
        if cur_path == prev_path:
            raise ValueError(
                f"Could not find Fuchsia checkout directory from: {start_path}"
            )


def new_ninja_build_command_execution(
    context: FuchsiaBuildContext,
    ninja_args: Sequence[str],
) -> BuildCommandExecution:
    """Construct a ninja build command.

    Behavior:
    - Selects the Ninja binary: uses the first argument if it ends in 'ninja'
      or 'rsninja.sh', otherwise defaults to $PREBUILT_NINJA or 'ninja'.
      If 'rsninja.sh' exists in the source tree, it is preferred.
    - Computes concurrency: uses -j if provided, otherwise defaults to the
      context's concurrency (which is auto-detected based on RBE and CPU count).
    - Ensures file descriptor limits are sufficient for the concurrency.
    - Sets a default load average limit on Darwin if not provided.
    - Always inserts '-C <build_directory>' to ensure Ninja runs in the correct
      build directory.
    - Adds '--edge_weights_list' to track build performance.
    """
    # Ninja argument massage logic
    concurrency: str | None = None
    load: str | None = None
    remaining = []

    # If the first argument looks like a ninja path, we'll use it.
    # Otherwise we'll use the default one.
    if ninja_args and (
        ninja_args[0].endswith("ninja") or ninja_args[0].endswith("rsninja.sh")
    ):
        ninja_bin = ninja_args[0]
        it = iter(ninja_args[1:])
    else:
        ninja_bin = context.env.get("PREBUILT_NINJA", "ninja")
        # Parity check for rsninja.sh
        rsninja = context.rsninja_sh
        if is_executable(rsninja):
            ninja_bin = str(rsninja)
        it = iter(ninja_args)

    for opt in it:
        if opt == "-j":
            try:
                concurrency = next(it)
            except StopIteration:
                raise BuildConfigurationError("-j requires an argument")
        elif opt.startswith("-j"):
            concurrency = opt[2:]
        elif opt == "-l":
            try:
                load = next(it)
            except StopIteration:
                raise BuildConfigurationError("-l requires an argument")
        elif opt.startswith("-l"):
            load = opt[2:]
        else:
            remaining.append(opt)

    if load is None and sys.platform == "darwin":
        load = str(get_cpu_count() * 20)

    if concurrency is None:
        concurrency = str(context.concurrency)
        # Check ulimit for file descriptors
        if concurrency:
            ensure_file_descriptor_limit(int(concurrency) * 2)

    ninja_args_list = ["-j", str(concurrency)]
    if load:
        ninja_args_list.extend(["-l", str(load)])

    # Add edge weights
    ninja_args_list.append(
        f"--edge_weights_list={context.ninja_edge_weights_csv}"
    )

    build_cmd = (
        [ninja_bin]
        + ninja_args_list
        + ["-C", str(context.build_dir)]
        + remaining
    )
    is_build = is_ninja_build_command(build_cmd)
    invocation = BuildInvocation(context)
    return invocation.new_build_command_execution(
        "ninja", build_cmd, is_build=is_build
    )


def is_ninja_build_command(ninja_args: list[str]) -> bool:
    """Returns True if the ninja command is a build-like command."""
    for arg in ninja_args:
        if arg in (
            "-n",
            "--dry-run",
            "-h",
            "--help",
            "--version",
        ) or arg.startswith("-t"):
            return False
    return True


def _is_bazel_binary(path_str: str) -> bool:
    """Returns True if the path_str points to a bazel binary."""
    return "bazel" in pathlib.Path(path_str).name


def is_bazel_build_command(bazel_args: list[str]) -> bool:
    """Returns True if the bazel command is a build-like command."""
    for arg in bazel_args:
        # Skip leading options, env assignments, env prefix, and the bazel binary itself
        if (
            arg.startswith("-")
            or "=" in arg
            or arg in ("env", "bazel")
            or _is_bazel_binary(arg)
        ):
            continue
        # The first non-option/non-env/non-binary argument is the subcommand!
        return arg in ("build", "test", "run", "coverage")
    return False


def new_bazel_build_command_execution(
    context: FuchsiaBuildContext,
    bazel_args: list[str],
) -> BuildCommandExecution:
    """Construct a bazel build command.

    Behavior:
    - Selects the Bazel binary: uses the first argument if 'bazel' in its name,
      otherwise defaults to 'bazel'.
    """
    if bazel_args and _is_bazel_binary(bazel_args[0]):
        # Already has bazel binary
        build_cmd = bazel_args
    else:
        build_cmd = ["bazel"] + list(bazel_args)

    is_build = is_bazel_build_command(build_cmd)
    invocation = BuildInvocation(context)
    return invocation.new_build_command_execution(
        "bazel", build_cmd, is_build=is_build
    )


def new_other_build_command_execution(
    context: FuchsiaBuildContext,
    other_args: list[str],
) -> BuildCommandExecution:
    """Construct an arbitrary build command.

    Note: main_build.py is the orchestrator for actual, standard build execution
    pipelines (such as Ninja and Bazel). It is NOT expected or intended to receive
    or wrap fint-specific commands, as fint is managed and executed externally.

    Args:
        context: FuchsiaBuildContext.
        other_args: list of arguments for the command.
    """
    if not other_args:
        raise BuildConfigurationError(
            "other command requires at least one argument."
        )

    is_build = True
    invocation = BuildInvocation(context)
    return invocation.new_build_command_execution(
        "other", other_args, is_build=is_build
    )


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=pathlib.Path, required=True)
    parser.add_argument("--out-dir", type=pathlib.Path)

    # Custom handling for boolean flags to support --flag=true/false
    parser.add_argument("--rbe", type=str_to_bool, nargs="?", const=True)
    parser.add_argument("--no-rbe", action="store_false", dest="rbe")

    parser.add_argument(
        "--resultstore",
        type=str_to_resultstore,
        nargs="?",
        const="all",
        default=None,
        help="Upload build events and metadata to ResultStore (all, ninja, bazel, or none; default: none).",
    )
    parser.add_argument(
        "--no-resultstore",
        action="store_const",
        const="none",
        dest="resultstore",
        help="Disable uploading build events to ResultStore.",
    )

    parser.add_argument("--profile", type=str_to_bool, nargs="?", const=True)
    parser.add_argument("--no-profile", action="store_false", dest="profile")

    parser.add_argument("--tui", type=str_to_bool, nargs="?", const=True)
    parser.add_argument("--no-tui", action="store_false", dest="tui")

    parser.add_argument(
        "--auth-mode",
        choices=["auto", "user", "machine", "none"],
        default="auto",
        help="Specify RBE/ResultStore authentication mode (auto, user, machine, or none; default: auto).",
    )

    parser.add_argument(
        "--max-concurrency",
        type=int,
        default=0,
        help="Upper bound on the automatically chosen -j value, or 0 for no bound. Ignored when -j is passed explicitly.",
    )

    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--no-status", action="store_false", dest="status")

    parser.add_argument(
        "--fint-params-path",
        type=pathlib.Path,
        default=None,
        help="Path to the Fint static parameters textproto. If provided, main_build.py automatically wraps execution inside fint_build.py.",
    )
    parser.add_argument(
        "--fint-context-path",
        type=pathlib.Path,
        default=None,
        help="Path to the Fint context parameters textproto. If provided, forwarded to fint_build.py --context.",
    )
    parser.add_argument(
        "--output-metadata-json",
        type=pathlib.Path,
        default=None,
        help="Path to write the structured metadata JSON describing all build logs and artifacts.",
    )
    parser.add_argument(
        "--remote-proxy-socket",
        type=pathlib.Path,
        default=None,
        help="Path to the local RBE proxy socket.",
    )
    parser.add_argument(
        "--resultstore-proxy-socket",
        type=pathlib.Path,
        default=None,
        help="Path to the local ResultStore/BES proxy socket.",
    )
    parser.add_argument(
        "--resultstore-instance",
        type=gcp_instance_name,
        default=None,
        help="Specify the target ResultStore instance name (format: projects/<project-id>/instances/<instance-name>).",
    )
    parser.add_argument(
        "--cas-instance",
        type=gcp_instance_name,
        default=None,
        help="Specify the target CAS instance name (format: projects/<project-id>/instances/<instance-name>).",
    )
    parser.add_argument(
        "--rbe-instance",
        type=gcp_instance_name,
        default=None,
        help="Specify the target RBE instance name (format: projects/<project-id>/instances/<instance-name>).",
    )

    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser(
        "ninja",
        help="Execute a Ninja build.",
        description="Expects arbitrary Ninja arguments to be passed after 'ninja'.",
    ).set_defaults(func=new_ninja_build_command_execution)
    subparsers.add_parser(
        "bazel",
        help="Execute a Bazel build.",
        description="Expects arbitrary Bazel arguments to be passed after 'bazel'.",
    ).set_defaults(func=new_bazel_build_command_execution)
    subparsers.add_parser(
        "other",
        help="Execute an arbitrary command.",
        description="Expects arbitrary arguments to be passed after 'other'.",
    ).set_defaults(func=new_other_build_command_execution)
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def main(argv: list[str]) -> int:
    args, unknown = _MAIN_ARG_PARSER.parse_known_args(argv)

    environ = dict(os.environ)
    _check_rbe_env_vars(environ)
    context = FuchsiaBuildContext.from_args(args, environ)

    invocation = None
    try:
        exec_info = args.func(context, unknown)
        invocation = exec_info.invocation
        return exec_info.run().return_code
    except BuildConfigurationError as e:
        msg(f"Error: {e}", file=sys.stderr)
        return 1
    except signal_utils.BuildInterruptedError as e:
        # SignalManagedProcess ensures that we have already waited for any
        # child processes before this is raised.
        sig_name = signal.Signals(e.signum).name
        msg(
            f"Interrupted by {sig_name}, exiting ({e.return_code})",
            file=sys.stderr,
        )
        return e.return_code
    except KeyboardInterrupt:
        # Fallback for standard interrupts outside SignalManagedProcess
        msg("Received KeyboardInterrupt, exiting (130)", file=sys.stderr)
        return 130
    finally:
        if args.output_metadata_json:
            active_invocation = invocation
            if not active_invocation:
                try:
                    active_invocation = BuildInvocation(context)
                except Exception:
                    pass
            if active_invocation:
                active_invocation.write_metadata_json(args.output_metadata_json)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
