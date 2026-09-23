#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import builtins
import contextlib
import getpass
import io
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from collections.abc import Generator
from contextlib import contextmanager
from typing import Any
from unittest import mock

import main_build
import signal_utils


def default_args() -> argparse.Namespace:
    """Returns a default-populated argparse.Namespace using the production parser."""
    return main_build._MAIN_ARG_PARSER.parse_args(
        ["--build-dir", "out/default", "ninja"]
    )


class MainBuildTestBase(unittest.TestCase):
    """Base class for main_build tests with shared helpers."""

    def setUp(self) -> None:
        # Default mock for read_json to avoid file system errors for rbe_settings.json etc.
        self.read_json_patcher = mock.patch.object(
            main_build, "read_json", return_value={}
        )
        self.mock_read_json = self.read_json_patcher.start()

    def tearDown(self) -> None:
        self.read_json_patcher.stop()

    @contextmanager
    def mock_invocation_context(
        self, build_uuid: str = "uuid-123", timestamp: str = "ts-456"
    ) -> Generator[tuple[mock.Mock, mock.Mock], None, None]:
        """Helper to mock BuildInvocation boilerplate."""
        with mock.patch.object(
            main_build.BuildInvocation,
            "build_uuid",
            new_callable=mock.PropertyMock,
            return_value=build_uuid,
        ):
            with mock.patch.object(
                main_build.BuildInvocation,
                "timestamp",
                new_callable=mock.PropertyMock,
                return_value=timestamp,
            ):
                with mock.patch.object(main_build, "mkdir") as mock_mkdir:
                    with mock.patch.object(
                        main_build, "write_text"
                    ) as mock_write:
                        yield mock_mkdir, mock_write

    def create_context(
        self, env: dict[str, str] | None = None, **config_kwargs: Any
    ) -> main_build.FuchsiaBuildContext:
        """Helper to create a FuchsiaBuildContext with specific config."""
        config_vals: dict[str, Any] = {
            "rbe": False,
            "resultstore": "none",
            "profile": False,
            "tui": False,
            "verbose": False,
            "dry_run": False,
            "status": True,
            "auth_mode": "auto",
        }
        config_vals.update(config_kwargs)
        config = main_build.FuchsiaBuildConfig(**config_vals)

        env_vals = {"USER": "fake-user"}
        if env is not None:
            env_vals.update(env)

        return main_build.FuchsiaBuildContext(
            source_dir=pathlib.Path("/tmp/fuchsia"),
            out_dir=pathlib.Path("/tmp/out"),
            build_dir=pathlib.Path("/tmp/out/default"),
            env=env_vals,
            config=config,
        )


class FuchsiaBuildContextTest(MainBuildTestBase):
    def test_properties(self) -> None:
        source_dir = pathlib.Path("/tmp/fuchsia")
        out_dir = pathlib.Path("/tmp/out")
        build_dir = out_dir / "default"
        context = self.create_context()
        context.source_dir = source_dir
        context.out_dir = out_dir
        context.build_dir = build_dir

        self.assertEqual(
            context.rbe_settings_file, build_dir / "rbe_settings.json"
        )
        self.assertEqual(context.rbe_config_json, build_dir / "rbe_config.json")
        self.assertEqual(
            context.check_loas_script,
            source_dir / "build/auth/check_loas_restrictions.sh",
        )
        self.assertEqual(
            context.top_build_wrapper,
            source_dir / "build/scripts/top_build_wrap.sh",
        )
        self.assertEqual(context.args_gn, build_dir / "args.gn")
        self.assertEqual(
            context.rsninja_sh, source_dir / "build/resultstore/rsninja.sh"
        )
        self.assertEqual(
            context.ninja_edge_weights_csv, build_dir / "ninja_edge_weights.csv"
        )

    def test_loas_type_skip_when_no_auth(self) -> None:
        context = self.create_context(resultstore="none")
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=False,
        ):
            self.assertEqual(context.loas_type, "skip")

    def test_loas_type_detected_when_needs_auth(self) -> None:
        context = self.create_context()
        context.env = {"FOO": "BAR"}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(main_build, "has_loas", return_value=True):
            with mock.patch.object(
                main_build, "is_executable", return_value=True
            ):
                with mock.patch.object(
                    subprocess,
                    "check_output",
                    return_value="some output\nrestricted\n",
                ) as mock_sub:
                    self.assertEqual(context.loas_type, "restricted")
                    mock_sub.assert_called_once_with(
                        [str(context.check_loas_script)],
                        text=True,
                        stderr=mock.ANY,
                        env=context.env,
                    )

    def test_loas_type_unrestricted_when_cred_helper_accessible(self) -> None:
        context = self.create_context()
        context.env = {"FOO": "BAR"}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(main_build, "has_loas", return_value=True):

            def mock_is_executable(path: pathlib.Path) -> bool:
                # Both the check_loas_script and BAZEL_CRED_HELPER are accessible
                return True

            with mock.patch.object(
                main_build, "is_executable", side_effect=mock_is_executable
            ):
                with mock.patch.object(
                    subprocess,
                    "check_output",
                    return_value="unrestricted\n",
                ):
                    self.assertEqual(context.loas_type, "unrestricted")

    def test_loas_type_downgraded_when_cred_helper_inaccessible(self) -> None:
        context = self.create_context()
        context.env = {"FOO": "BAR"}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(main_build, "has_loas", return_value=True):

            def mock_is_executable(path: pathlib.Path) -> bool:
                # The check_loas_script is executable, but BAZEL_CRED_HELPER is not
                return str(path) != str(main_build.BAZEL_CRED_HELPER)

            with mock.patch.object(
                main_build, "is_executable", side_effect=mock_is_executable
            ):
                with mock.patch.object(
                    subprocess,
                    "check_output",
                    return_value="unrestricted\n",
                ):
                    with mock.patch.object(main_build, "msg") as mock_msg:
                        self.assertEqual(context.loas_type, "restricted")
                        mock_msg.assert_any_call(
                            f"WARNING: Bazel credential helper on SrcFS is not accessible: {main_build.BAZEL_CRED_HELPER}",
                            file=mock.ANY,
                        )

    def test_rbe_settings_missing_throws(self) -> None:
        context = self.create_context(rbe=None)
        self.mock_read_json.side_effect = main_build.BuildConfigurationError(
            "missing file"
        )
        with self.assertRaises(main_build.BuildConfigurationError) as cm:
            _ = context.rbe_enabled
        self.assertEqual(str(cm.exception), "missing file")

    def test_concurrency_capped(self) -> None:
        context = self.create_context(rbe=True, max_concurrency=64)
        with mock.patch.object(main_build, "get_cpu_count", return_value=96):
            self.assertEqual(context.concurrency, 64)

    def test_concurrency_cap_does_not_raise_concurrency(self) -> None:
        context = self.create_context(rbe=True, max_concurrency=64)
        with mock.patch.object(main_build, "get_cpu_count", return_value=4):
            self.assertEqual(context.concurrency, 40)

    def test_concurrency_uncapped(self) -> None:
        context = self.create_context(rbe=True)
        with mock.patch.object(main_build, "get_cpu_count", return_value=96):
            self.assertEqual(context.concurrency, 960)

    def test_parse_properties(self) -> None:
        test_cases = [
            ("key=value\n", {"key": "value"}),
            ("  key  =  value  \n", {"key": "value"}),
            ('export key="value"\n', {"key": "value"}),
            ("export key='value'\n", {"key": "value"}),
            ('key="value" # comments!\n', {"key": "value"}),
            ("=empty_key\nkey=value\n", {"key": "value"}),
            ("# comments\n// comments\n\nkey=value\n", {"key": "value"}),
            ("empty_val=\n", {}),
            ("invalid_value\n", {}),
            ('key="unclosed\nkey2=value\n', {"key2": "value"}),
        ]
        for content, expected in test_cases:
            self.assertEqual(
                main_build.parse_properties(content),
                expected,
                f"Failed parsing: {content!r}",
            )

    def test_load_user_preference(self) -> None:
        with mock.patch.object(
            main_build, "exists", return_value=True
        ), mock.patch.object(
            pathlib.Path, "read_text", return_value="resultstore=all\n"
        ):
            result = main_build.load_user_preference(pathlib.Path("path"))
            self.assertEqual(result, "all")

    def test_preference_precedence_hierarchy(self) -> None:
        # Table of precedence scenarios: (args, global_val, local_val, expected_result)
        test_cases: list[tuple[list[str], str | None, str | None, str]] = [
            # Case A: No config files, no flag -> defaults to "none"
            ([], None, None, "none"),
            # Case B: Global config file present, no flag -> defaults to global ("all")
            ([], "all", None, "all"),
            # Case C: Both present, no flag -> local overrides global ("ninja")
            ([], "all", "ninja", "ninja"),
            # Case D: Both present, but CLI flag overrides both ("bazel")
            (["--resultstore=bazel"], "all", "ninja", "bazel"),
            # Case E: Both present, but CLI --no-resultstore overrides both ("none")
            (["--no-resultstore"], "all", "ninja", "none"),
        ]

        environ = {"FUCHSIA_DIR": "/tmp/fuchsia"}

        for args, global_val, local_val, expected in test_cases:
            full_args = ["--build-dir", "out/default"] + args + ["ninja"]
            parsed_args = main_build._MAIN_ARG_PARSER.parse_args(full_args)

            def mock_load(path: pathlib.Path) -> str | None:
                if ".fx/config/resultstore" in str(path):
                    return global_val
                if ".resultstore" in str(path):
                    return local_val
                return None

            with mock.patch.object(
                main_build, "load_user_preference", side_effect=mock_load
            ):
                ctx = main_build.FuchsiaBuildContext.from_args(
                    parsed_args, environ
                )
                self.assertEqual(
                    ctx.config.resultstore,
                    expected,
                    f"Failed precedence: args={args}, global={global_val}, local={local_val}",
                )

    def test_resolve_source_dir_from_env(self) -> None:
        """Verifies resolve_source_dir returns absolute path from environment variable."""
        env = {"FUCHSIA_DIR": "/custom/fuchsia/path"}
        resolved = main_build.resolve_source_dir(env)
        self.assertEqual(
            resolved, pathlib.Path("/custom/fuchsia/path").resolve()
        )

    @mock.patch.object(main_build, "find_fuchsia_dir")
    def test_resolve_source_dir_from_find_fuchsia_dir(
        self, mock_find: mock.Mock
    ) -> None:
        """Verifies resolve_source_dir calls find_fuchsia_dir when environment is empty."""
        mock_find.return_value = pathlib.Path("/found/fuchsia")
        resolved = main_build.resolve_source_dir({})
        self.assertEqual(resolved, pathlib.Path("/found/fuchsia"))
        mock_find.assert_called_once()

    @mock.patch.object(
        main_build, "find_fuchsia_dir", side_effect=ValueError("Not found")
    )
    def test_resolve_source_dir_fallback(self, mock_find: mock.Mock) -> None:
        """Verifies resolve_source_dir falls back to relative path if find_fuchsia_dir raises ValueError."""
        resolved = main_build.resolve_source_dir({})
        expected = main_build._SCRIPT.resolve().parent.parent.parent
        self.assertEqual(resolved, expected)

    def test_authenticated_user_from_env(self) -> None:
        """Verifies that authenticated_user resolves from USER in env."""
        context = self.create_context()
        context.env = {"USER": "custom-user"}
        self.assertEqual(context.authenticated_user, "custom-user")

    def test_authenticated_user_from_getpass(self) -> None:
        """Verifies that authenticated_user falls back to getpass.getuser() when USER is absent."""
        context = self.create_context()
        context.env = {}
        with mock.patch.object(getpass, "getuser", return_value="login-user"):
            self.assertEqual(context.authenticated_user, "login-user")

    def test_authenticated_user_missing_raises_error(self) -> None:
        """Verifies that authenticated_user raises BuildConfigurationError when USER cannot be resolved and loas_type is not skip."""
        context = self.create_context()
        context.env = {}
        with mock.patch.object(getpass, "getuser", side_effect=Exception()):
            with mock.patch.object(
                main_build.FuchsiaBuildContext,
                "loas_type",
                new_callable=mock.PropertyMock,
                return_value="restricted",
            ):
                with self.assertRaises(main_build.BuildConfigurationError):
                    _ = context.authenticated_user

    def test_authenticated_user_missing_defaults_to_builder(self) -> None:
        """Verifies that authenticated_user defaults to 'builder' when USER cannot be resolved and loas_type is skip."""
        context = self.create_context()
        context.env = {}
        with mock.patch.object(getpass, "getuser", side_effect=Exception()):
            with mock.patch.object(
                main_build.FuchsiaBuildContext,
                "loas_type",
                new_callable=mock.PropertyMock,
                return_value="skip",
            ):
                self.assertEqual(context.authenticated_user, "builder")

    @mock.patch.object(
        pathlib.Path, "home", return_value=pathlib.Path("/mock/home")
    )
    def test_auth_env_needs_auth_false(self, mock_home: mock.Mock) -> None:
        """Verifies that auth_env only contains USER when needs_auth is False."""
        context = self.create_context(resultstore="none")
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=False,
        ):
            self.assertEqual(context.auth_env, {"USER": "fake-user"})

    @mock.patch.object(
        pathlib.Path, "home", return_value=pathlib.Path("/mock/home")
    )
    def test_auth_env_machine_auth_omits_google_application_credentials(
        self, mock_home: mock.Mock
    ) -> None:
        """Verifies auth_env omits GOOGLE_APPLICATION_CREDENTIALS fallback under machine auth mode."""
        context = self.create_context(auth_mode="machine", resultstore="all")
        context.env = {}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(getpass, "getuser", side_effect=Exception()):
            env = context.auth_env
            self.assertEqual(env["FX_BUILD_LOAS_TYPE"], "skip")
            self.assertEqual(env["USER"], "builder")
            self.assertNotIn("GOOGLE_APPLICATION_CREDENTIALS", env)

    def test_auth_env_gce_metadata_host_propagation(self) -> None:
        """Verifies that GCE_METADATA_HOST is translated into G_CLOUD_METADATA_HOST and GCLOUD_METADATA_HOST."""
        context = self.create_context(resultstore="all")
        host_addr = "127.0.0.1:8080"
        context.env = {"GCE_METADATA_HOST": host_addr}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ):
            env = context.auth_env
            self.assertEqual(env["GCE_METADATA_HOST"], host_addr)
            self.assertEqual(env["G_CLOUD_METADATA_HOST"], host_addr)
            self.assertEqual(env["GCLOUD_METADATA_HOST"], host_addr)

    def test_get_build_env_proxy_socket_propagation(self) -> None:
        """Verifies that remote_proxy_socket and resultstore_proxy_socket propagate correct environment variables."""
        rbe_socket = pathlib.Path("/tmp/rbe.sock")
        bes_socket = pathlib.Path("/tmp/bes.sock")
        context = self.create_context(
            remote_proxy_socket=rbe_socket,
            resultstore_proxy_socket=bes_socket,
        )
        invocation = main_build.BuildInvocation(context)
        with self.mock_invocation_context():
            env = invocation.get_build_env()
            self.assertEqual(env["RBE_service"], f"unix://{rbe_socket}")
            self.assertEqual(env["RS_cas_service"], f"unix://{rbe_socket}")
            self.assertEqual(env["RS_rs_service"], f"unix://{bes_socket}")
            self.assertEqual(
                env["FX_INTERNAL_BAZEL_RBE_SOCKET_PATH"], str(rbe_socket)
            )
            self.assertEqual(
                env["FX_INTERNAL_BAZEL_RESULTSTORE_SOCKET_PATH"],
                str(bes_socket),
            )

    @mock.patch.object(
        pathlib.Path, "home", return_value=pathlib.Path("/mock/home")
    )
    def test_auth_env_has_loas_true(self, mock_home: mock.Mock) -> None:
        """Verifies auth_env when needs_auth is True and has_loas() is True."""
        context = self.create_context(resultstore="all")
        context.env = {"USER": "custom-user"}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(
            main_build, "has_loas", return_value=True
        ), mock.patch.object(
            main_build.FuchsiaBuildContext,
            "loas_type",
            new_callable=mock.PropertyMock,
            return_value="restricted",
        ):
            env = context.auth_env
            self.assertEqual(env["FX_BUILD_LOAS_TYPE"], "restricted")
            self.assertEqual(env["USER"], "custom-user")
            self.assertEqual(
                env["GOOGLE_APPLICATION_CREDENTIALS"],
                "/mock/home/.config/gcloud/application_default_credentials.json",
            )

    @mock.patch.object(
        pathlib.Path, "home", return_value=pathlib.Path("/mock/home")
    )
    def test_auth_env_has_loas_false_defaults_to_builder(
        self, mock_home: mock.Mock
    ) -> None:
        """Verifies auth_env defaults USER to builder when has_loas() is False and USER is absent."""
        context = self.create_context(resultstore="all")
        context.env = {}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(
            main_build, "has_loas", return_value=False
        ), mock.patch.object(
            getpass, "getuser", side_effect=Exception()
        ):
            env = context.auth_env
            self.assertEqual(env["FX_BUILD_LOAS_TYPE"], "skip")
            self.assertEqual(env["USER"], "builder")
            self.assertEqual(
                env["GOOGLE_APPLICATION_CREDENTIALS"],
                "/mock/home/.config/gcloud/application_default_credentials.json",
            )

    @mock.patch.object(
        pathlib.Path, "home", return_value=pathlib.Path("/mock/home")
    )
    def test_auth_env_has_loas_false_forwards_user(
        self, mock_home: mock.Mock
    ) -> None:
        """Verifies auth_env forwards USER when has_loas() is False and USER is explicitly set."""
        context = self.create_context(resultstore="all")
        context.env = {"USER": "custom-bot"}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(
            main_build, "has_loas", return_value=False
        ), mock.patch.object(
            getpass, "getuser", side_effect=Exception()
        ):
            env = context.auth_env
            self.assertEqual(env["FX_BUILD_LOAS_TYPE"], "skip")
            self.assertEqual(env["USER"], "custom-bot")
            self.assertEqual(
                env["GOOGLE_APPLICATION_CREDENTIALS"],
                "/mock/home/.config/gcloud/application_default_credentials.json",
            )

    def test_auth_env_home_raises_runtime_error(self) -> None:
        """Verifies that auth_env handles Path.home() raising RuntimeError safely by omitting credentials."""
        context = self.create_context(resultstore="all")
        context.env = {}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ), mock.patch.object(
            main_build, "has_loas", return_value=False
        ), mock.patch.object(
            getpass, "getuser", side_effect=Exception()
        ), mock.patch.object(
            pathlib.Path, "home", side_effect=RuntimeError("Cannot find home")
        ):
            env = context.auth_env
            self.assertEqual(env["FX_BUILD_LOAS_TYPE"], "skip")
            self.assertEqual(env["USER"], "builder")
            self.assertNotIn("GOOGLE_APPLICATION_CREDENTIALS", env)

    def test_resolved_auth_mode_explicit_none(self) -> None:
        """Verifies that auth_mode 'none' always resolves to 'none'."""
        context = self.create_context(auth_mode="none")
        self.assertEqual(context.resolved_auth_mode, "none")

    def test_resolved_auth_mode_needs_auth_false(self) -> None:
        """Verifies that if needs_auth is False, resolved_auth_mode is always 'none'."""
        context = self.create_context(auth_mode="machine", resultstore="none")
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=False,
        ):
            self.assertEqual(context.resolved_auth_mode, "none")

    def test_resolved_auth_mode_explicit_machine(self) -> None:
        """Verifies that explicit 'machine' auth_mode resolves to 'machine'."""
        context = self.create_context(auth_mode="machine", resultstore="all")
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ):
            self.assertEqual(context.resolved_auth_mode, "machine")

    def test_resolved_auth_mode_explicit_user(self) -> None:
        """Verifies that explicit 'user' auth_mode resolves to 'user'."""
        context = self.create_context(auth_mode="user", resultstore="all")
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ):
            self.assertEqual(context.resolved_auth_mode, "user")

    def test_resolved_auth_mode_auto_on_bot(self) -> None:
        """Verifies that 'auto' auth_mode on a bot (with BUILDBUCKET_ID) resolves to 'machine'."""
        context = self.create_context(auth_mode="auto", resultstore="all")
        context.env = {"BUILDBUCKET_ID": "123"}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ):
            self.assertEqual(context.resolved_auth_mode, "machine")

    def test_resolved_auth_mode_auto_on_workstation(self) -> None:
        """Verifies that 'auto' auth_mode on a workstation (no bot hints) resolves to 'user'."""
        context = self.create_context(auth_mode="auto", resultstore="all")
        context.env = {}
        with mock.patch.object(
            main_build.FuchsiaBuildContext,
            "needs_auth",
            new_callable=mock.PropertyMock,
            return_value=True,
        ):
            self.assertEqual(context.resolved_auth_mode, "user")


class BuildInvocationTest(MainBuildTestBase):
    def test_init_caching(self) -> None:
        context = self.create_context()
        with self.mock_invocation_context("uuid-123", "ts-456") as (
            mock_mkdir,
            mock_write,
        ):
            invocation = main_build.BuildInvocation(context)
            self.assertEqual(invocation.build_uuid, "uuid-123")
            self.assertEqual(invocation.timestamp, "ts-456")
            log_dir = pathlib.Path(
                "/tmp/out/_build_logs/default/build.ts-456.uuid-123"
            )
            self.assertEqual(str(invocation.log_dir), str(log_dir))

            expected_mkdir_calls = [
                mock.call(pathlib.Path("/tmp/out/_build_logs/default")),
                mock.call(log_dir),
            ]
            mock_mkdir.assert_has_calls(expected_mkdir_calls)
            mock_write.assert_called_once_with(
                log_dir / "invocation_id", "uuid-123\n"
            )

    def test_get_build_env(self) -> None:
        context = self.create_context()
        context.env = {"TERM": "xterm", "USER": "fuchsia-user", "EXTRA": "val"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["FX_BUILD_UUID"], "uuid-123")
            self.assertEqual(env["TERM"], "xterm")
            self.assertEqual(env["USER"], "fuchsia-user")
            self.assertNotIn("EXTRA", env)
            self.assertEqual(env["NINJA_STATUS"], "[%f/%t][%p/%w](%r) ")

    def test_get_build_env_forward_buildbucket(self) -> None:
        context = self.create_context()
        context.env = {
            "USER": "fuchsia-user",
            "BUILDBUCKET_ID": "8670925737098591985",
            "BUILDBUCKET_BUILDER": "fuchsia-builder",
            "SWARMING_TASK_ID": "616a9bc24f0",
        }
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["BUILDBUCKET_ID"], "8670925737098591985")
            self.assertEqual(env["BUILDBUCKET_BUILDER"], "fuchsia-builder")
            self.assertEqual(env["SWARMING_TASK_ID"], "616a9bc24f0")

    def test_get_build_env_forward_rs_variables(self) -> None:
        context = self.create_context(
            resultstore_instance="projects/fuchsia-infra/instances/default_instance",
            cas_instance="projects/fuchsia-infra/instances/default_instance",
            rbe_instance="projects/fuchsia-infra/instances/default_instance",
        )
        context.env = {"USER": "fuchsia-user"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(
                env["RS_rs_instance"],
                "projects/fuchsia-infra/instances/default_instance",
            )
            self.assertEqual(
                env["RS_cas_instance"],
                "projects/fuchsia-infra/instances/default_instance",
            )
            self.assertEqual(
                env["RBE_instance"],
                "projects/fuchsia-infra/instances/default_instance",
            )

    def test_get_build_env_no_forward_when_matching_disk_defaults(self) -> None:
        context = self.create_context(
            resultstore_instance="projects/rbe-fuchsia-prod/instances/default",
            cas_instance="projects/rbe-fuchsia-prod/instances/default",
            rbe_instance="projects/rbe-fuchsia-prod/instances/default",
        )
        context.env = {"USER": "fuchsia-user"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertNotIn("RS_rs_instance", env)
            self.assertNotIn("RS_cas_instance", env)
            self.assertNotIn("RBE_instance", env)

    def test_build_service_env_empty_defaults(self) -> None:
        context = self.create_context()
        self.assertEqual(context.build_service_env, {})

    def test_build_service_env_custom_overrides(self) -> None:
        context = self.create_context(
            resultstore_instance="projects/custom-rs/instances/default",
            cas_instance="projects/custom-cas/instances/default",
            rbe_instance="projects/custom-rbe/instances/default",
            remote_proxy_socket="/tmp/rbe.sock",
            resultstore_proxy_socket="/tmp/rs.sock",
        )
        env = context.build_service_env
        self.assertEqual(
            env["RS_rs_instance"], "projects/custom-rs/instances/default"
        )
        self.assertEqual(
            env["RS_cas_instance"], "projects/custom-cas/instances/default"
        )
        self.assertEqual(
            env["RBE_instance"], "projects/custom-rbe/instances/default"
        )
        self.assertEqual(env["RBE_service"], "unix:///tmp/rbe.sock")
        self.assertEqual(env["RS_cas_service"], "unix:///tmp/rbe.sock")
        self.assertEqual(env["RS_rs_service"], "unix:///tmp/rs.sock")

    def test_parse_cfg_text(self) -> None:
        cfg_text = (
            "\n"
            "        # Comment line\n"
            "        key1 = value1\n"
            "        key2=value2\n"
            "        # Another comment\n"
            "        key3 =  value3" + "  \n"
        )
        parsed = main_build._parse_cfg_text(cfg_text)
        self.assertEqual(
            parsed, {"key1": "value1", "key2": "value2", "key3": "value3"}
        )

    def test_parse_cfg_file_missing_returns_empty(self) -> None:
        non_existent = pathlib.Path("/nonexistent/file.cfg")
        self.assertEqual(main_build._parse_cfg_file(non_existent), {})

    def test_get_build_env_no_forward_rs_variables_from_env(self) -> None:
        context = self.create_context()
        context.env = {
            "USER": "fuchsia-user",
            "RS_rs_instance": "projects/fuchsia-infra/instances/default_instance",
            "RS_cas_instance": "projects/fuchsia-infra/instances/default_instance",
        }
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertNotIn("RS_rs_instance", env)
            self.assertNotIn("RS_cas_instance", env)

    def test_get_build_env_homeless_fallback(self) -> None:
        context = self.create_context()
        context.env = {"USER": "fuchsia-user"}
        with self.mock_invocation_context() as (mock_mkdir, mock_write):
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["HOME"], str(invocation.temp_home))
            mock_mkdir.assert_any_call(invocation.temp_home)

    def test_get_build_env_home_preserved_without_creating_temp_home(
        self,
    ) -> None:
        context = self.create_context()
        context.env = {"USER": "fuchsia-user", "HOME": "/my/custom/home"}
        with self.mock_invocation_context() as (mock_mkdir, mock_write):
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["HOME"], "/my/custom/home")
            expected_temp_home = invocation.log_dir / ".home"
            # Verify that the expected temporary home was never created on disk
            for call in mock_mkdir.call_args_list:
                self.assertNotEqual(call[0][0], expected_temp_home)

    def test_get_build_env_no_status(self) -> None:
        context = self.create_context(status=False)
        context.env = {"TERM": "xterm"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["TERM"], "dumb")
            self.assertEqual(env["NINJA_STATUS"], "[%f/%t] ")

    def test_get_build_env_resultstore(self) -> None:
        # 1. Test resultstore="none" (default bypass)
        context = self.create_context(resultstore="none")
        context.env = {"USER": "fuchsia-user"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["FX_INTERNAL_RESULTSTORE_NINJA"], "0")
            self.assertEqual(env["FX_INTERNAL_RESULTSTORE_BAZEL"], "0")

        # 2. Test resultstore="ninja" (only Ninja)
        context = self.create_context(resultstore="ninja")
        context.env = {"USER": "fuchsia-user"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["FX_INTERNAL_RESULTSTORE_NINJA"], "1")
            self.assertEqual(env["FX_INTERNAL_RESULTSTORE_BAZEL"], "0")

        # 3. Test resultstore="bazel" (only Bazel, local dev with unrestricted LOAS)
        context = self.create_context(resultstore="bazel")
        context.env = {"USER": "fuchsia-user"}
        with self.mock_invocation_context():
            with mock.patch.object(
                main_build.FuchsiaBuildContext,
                "loas_type",
                new_callable=mock.PropertyMock,
                return_value="unrestricted",
            ):
                invocation = main_build.BuildInvocation(context)
                env = invocation.get_build_env()
                self.assertEqual(env["FX_INTERNAL_RESULTSTORE_NINJA"], "0")
                self.assertEqual(
                    env["FX_INTERNAL_RESULTSTORE_BAZEL"], "resultstore"
                )

        # 4. Test resultstore="bazel" (only Bazel, infra with BUILDBUCKET_ID)
        context = self.create_context(resultstore="bazel")
        context.env = {"USER": "fuchsia-user", "BUILDBUCKET_ID": "12345"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["FX_INTERNAL_RESULTSTORE_NINJA"], "0")
            self.assertEqual(
                env["FX_INTERNAL_RESULTSTORE_BAZEL"], "resultstore_infra"
            )

        # 5. Test resultstore="all" (both tools)
        context = self.create_context(resultstore="all")
        context.env = {"USER": "fuchsia-user", "BUILDBUCKET_ID": "12345"}
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            env = invocation.get_build_env()
            self.assertEqual(env["FX_INTERNAL_RESULTSTORE_NINJA"], "1")
            self.assertEqual(
                env["FX_INTERNAL_RESULTSTORE_BAZEL"], "resultstore_infra"
            )

    def test_get_build_env_proxy_socket_propagation(self) -> None:
        """Verifies that remote_proxy_socket and resultstore_proxy_socket propagate correct environment variables."""
        rbe_socket = pathlib.Path("/tmp/rbe.sock")
        bes_socket = pathlib.Path("/tmp/bes.sock")
        context = self.create_context(
            remote_proxy_socket=rbe_socket,
            resultstore_proxy_socket=bes_socket,
        )
        context.env = {"USER": "fuchsia-user"}
        invocation = main_build.BuildInvocation(context)
        with self.mock_invocation_context():
            env = invocation.get_build_env()
            self.assertEqual(env["RBE_service"], f"unix://{rbe_socket}")
            self.assertEqual(env["RS_cas_service"], f"unix://{rbe_socket}")
            self.assertEqual(env["RS_rs_service"], f"unix://{bes_socket}")
            self.assertEqual(
                env["FX_INTERNAL_BAZEL_RBE_SOCKET_PATH"], str(rbe_socket)
            )
            self.assertEqual(
                env["FX_INTERNAL_BAZEL_RESULTSTORE_SOCKET_PATH"],
                str(bes_socket),
            )

    def test_get_build_env_missing_user_error(self) -> None:
        context = self.create_context()
        context.env = {}  # No USER
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            with mock.patch.object(
                main_build.FuchsiaBuildContext,
                "needs_auth",
                new_callable=mock.PropertyMock,
                return_value=True,
            ):
                with mock.patch.object(
                    main_build.FuchsiaBuildContext,
                    "loas_type",
                    new_callable=mock.PropertyMock,
                    return_value="gcert",
                ):
                    with mock.patch.object(
                        getpass, "getuser", side_effect=Exception()
                    ):
                        with self.assertRaises(
                            main_build.BuildConfigurationError
                        ) as cm:
                            invocation.get_build_env()
                        self.assertIn(
                            "USER environment variable is not set",
                            str(cm.exception),
                        )


class BuildCommandExecutionTest(unittest.TestCase):
    @mock.patch.object(main_build, "BuildLock")
    @mock.patch.object(subprocess, "Popen")
    @mock.patch.dict(os.environ, {"FX_BUILD_QUIET": "0"})
    def test_run(self, mock_popen: mock.Mock, mock_lock: mock.Mock) -> None:
        # We still need context and invocation for the execution object
        # Create them manually to avoid TestBase dependency
        config = main_build.FuchsiaBuildConfig(
            rbe=False,
            resultstore="none",
            profile=False,
            tui=False,
            verbose=False,
            dry_run=False,
            auth_mode="auto",
        )
        context = main_build.FuchsiaBuildContext(
            source_dir=pathlib.Path("/tmp/fuchsia"),
            out_dir=pathlib.Path("/tmp/out"),
            build_dir=pathlib.Path("/tmp/out/default"),
            env={"USER": "fake-user"},
            config=config,
        )
        with mock.patch.object(
            main_build.BuildInvocation,
            "build_uuid",
            new_callable=mock.PropertyMock,
            return_value="uuid-123",
        ):
            with mock.patch.object(
                main_build.BuildInvocation,
                "timestamp",
                new_callable=mock.PropertyMock,
                return_value="ts",
            ):
                with mock.patch.object(main_build, "mkdir"):
                    with mock.patch.object(main_build, "write_text"):
                        invocation = main_build.BuildInvocation(context)

        exec_info = main_build.BuildCommandExecution(
            full_command=["cmd", "arg"],
            env={"VAR": "VAL"},
            invocation=invocation,
            cleanup_files=[pathlib.Path("/tmp/cleanup")],
        )

        mock_process = mock.Mock()
        mock_process.pid = 5678
        mock_process.wait.return_value = 0
        mock_popen.return_value = mock_process

        with mock.patch.object(main_build, "exists", return_value=True):
            with mock.patch.object(pathlib.Path, "unlink") as mock_unlink:
                result = exec_info.run()
                self.assertEqual(result.return_code, 0)
                mock_popen.assert_called_once()
                mock_unlink.assert_called_once_with(missing_ok=True)
                mock_lock.assert_called_once_with(
                    invocation.context.build_dir, print_message=False
                )

    @mock.patch.object(main_build, "BuildLock")
    @mock.patch.object(subprocess, "Popen")
    @mock.patch.dict(os.environ, {"FX_BUILD_QUIET": "0"})
    def test_run_dry_run(
        self, mock_popen: mock.Mock, mock_lock: mock.Mock
    ) -> None:
        config = main_build.FuchsiaBuildConfig(
            rbe=False,
            resultstore="none",
            profile=False,
            tui=False,
            verbose=False,
            dry_run=True,
            auth_mode="auto",
        )
        context = main_build.FuchsiaBuildContext(
            source_dir=pathlib.Path("/tmp/fuchsia"),
            out_dir=pathlib.Path("/tmp/out"),
            build_dir=pathlib.Path("/tmp/out/default"),
            env={"USER": "fake-user"},
            config=config,
        )
        with mock.patch.object(
            main_build.BuildInvocation,
            "build_uuid",
            new_callable=mock.PropertyMock,
            return_value="uuid-123",
        ):
            with mock.patch.object(
                main_build.BuildInvocation,
                "timestamp",
                new_callable=mock.PropertyMock,
                return_value="ts",
            ):
                with mock.patch.object(main_build, "mkdir"):
                    with mock.patch.object(main_build, "write_text"):
                        invocation = main_build.BuildInvocation(context)

        exec_info = main_build.BuildCommandExecution(
            full_command=["cmd", "arg"],
            env={"VAR": "VAL"},
            invocation=invocation,
            cleanup_files=[],
        )

        mock_process = mock.Mock()
        mock_process.pid = 5678
        mock_process.wait.return_value = 0
        mock_popen.return_value = mock_process

        result = exec_info.run()
        self.assertEqual(result.return_code, 0)
        # Even in dry_run mode, we should call the subprocess because
        # we forwarded --dry-run to the wrapper.
        mock_popen.assert_called_once()
        mock_lock.assert_called_once()


class BuildLockTest(unittest.TestCase):
    @mock.patch.object(main_build, "check_shell_command", return_value=True)
    @mock.patch.object(subprocess, "call")
    @mock.patch.object(builtins, "print")
    def test_acquire_lock_success(
        self,
        mock_print: mock.Mock,
        mock_call: mock.Mock,
        mock_check: mock.Mock,
    ) -> None:
        mock_call.return_value = 0
        build_dir = pathlib.Path("/tmp/build")
        with main_build.BuildLock(build_dir, print_message=True):
            pass
        mock_call.assert_called_with(
            [
                "shlock",
                "-f",
                str(build_dir.with_suffix(".build_lock")),
                "-p",
                mock.ANY,
            ]
        )
        mock_print.assert_any_call("Lock acquired, proceeding with build.")
        mock_print.assert_any_call("Build completed.")

    @mock.patch.object(main_build, "check_shell_command", return_value=True)
    @mock.patch.object(subprocess, "call")
    @mock.patch.object(time, "sleep")
    @mock.patch.object(builtins, "print")
    def test_acquire_lock_retries(
        self,
        mock_print: mock.Mock,
        mock_sleep: mock.Mock,
        mock_call: mock.Mock,
        mock_check: mock.Mock,
    ) -> None:
        mock_call.side_effect = [1, 0]
        build_dir = pathlib.Path("/tmp/build")
        with main_build.BuildLock(build_dir, print_message=True):
            pass
        self.assertEqual(mock_call.call_count, 2)
        mock_sleep.assert_called_once()
        mock_print.assert_any_call("Lock acquired, proceeding with build.")
        mock_print.assert_any_call("Build completed.")


class FindFuchsiaDirTest(unittest.TestCase):
    def test_find_success(self) -> None:
        # Mock exists() at the module level
        with mock.patch.object(main_build, "exists") as mock_exists:
            # .jiri_manifest checks:
            # 1. /tmp/a/b/c/.jiri_manifest -> False
            # 2. /tmp/a/b/.jiri_manifest -> False
            # 3. /tmp/a/.jiri_manifest -> True
            mock_exists.side_effect = [False, False, True]

            start = pathlib.Path("/tmp/a/b/c")
            res = main_build.find_fuchsia_dir(start)

            self.assertEqual(res, pathlib.Path("/tmp/a"))
            self.assertEqual(mock_exists.call_count, 3)

    def test_find_failure(self) -> None:
        with mock.patch.object(pathlib.Path, "exists", return_value=False):
            with self.assertRaises(ValueError):
                main_build.find_fuchsia_dir(pathlib.Path("/tmp/only/two"))


class GcpInstanceNameTest(unittest.TestCase):
    def test_valid_instance_name(self) -> None:
        self.assertEqual(
            main_build.gcp_instance_name(
                "projects/my-project/instances/default"
            ),
            "projects/my-project/instances/default",
        )

    def test_invalid_instance_name_raises(self) -> None:
        for invalid in (
            "",
            "projects",
            "projects/my-project",
            "projects/my-project/instances",
            "projects/my-project/instances/",
            "my-project/instances/default",
            "projects/instances/default",
            "projects/my-project/default",
            "projects/my-project/instances/default/",
            "projects/my-project/instances/default/extra",
        ):
            with self.assertRaises(argparse.ArgumentTypeError):
                main_build.gcp_instance_name(invalid)


class StrToBoolTest(unittest.TestCase):
    def test_str_to_bool(self) -> None:
        self.assertTrue(main_build.str_to_bool("true"))
        self.assertTrue(main_build.str_to_bool("1"))
        self.assertTrue(main_build.str_to_bool("yes"))
        self.assertFalse(main_build.str_to_bool("false"))
        self.assertFalse(main_build.str_to_bool("0"))
        self.assertFalse(main_build.str_to_bool("no"))
        with self.assertRaises(Exception):
            main_build.str_to_bool("maybe")


class CheckRbeEnvVarsTest(unittest.TestCase):
    def test_no_rbe_vars(self) -> None:
        f = io.StringIO()
        with contextlib.redirect_stdout(f):
            main_build._check_rbe_env_vars({"PATH": "/bin"})
        self.assertEqual(f.getvalue(), "")

    def test_rbe_vars_warning(self) -> None:
        f = io.StringIO()
        with contextlib.redirect_stdout(f):
            main_build._check_rbe_env_vars(
                {"RBE_FOO": "1", "RBE_BAR": "2", "PATH": "/bin"}
            )
        output = f.getvalue()
        self.assertIn("Warning", output)
        self.assertIn("RBE_BAR, RBE_FOO", output)


class RbeCpuConcurrencyTest(unittest.TestCase):
    def test_local(self) -> None:
        with mock.patch.object(main_build, "get_cpu_count", return_value=8):
            self.assertEqual(
                main_build.rbe_cpu_concurrency(rbe_enabled=False), 8
            )

    def test_rbe(self) -> None:
        with mock.patch.object(main_build, "get_cpu_count", return_value=8):
            self.assertEqual(
                main_build.rbe_cpu_concurrency(rbe_enabled=True), 80
            )


class TopBuildCommandPrefixTest(MainBuildTestBase):
    def test_basic(self) -> None:
        context = self.create_context(rbe=False, resultstore="none")
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            prefix = list(invocation.top_build_command_prefix())
            self.assertIn(
                "/tmp/fuchsia/build/scripts/top_build_wrap.sh", prefix[0]
            )
            self.assertIn("--build-dir", prefix)
            self.assertNotIn("--rbe", prefix)
            self.assertNotIn("--dry-run", prefix)

    def test_dry_run_forwarding(self) -> None:
        context = self.create_context(dry_run=True)
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            prefix = list(invocation.top_build_command_prefix())
            self.assertIn("--dry-run", prefix)

    def test_rbe_resultstore(self) -> None:
        context = self.create_context(rbe=True, resultstore="all")
        with mock.patch.multiple(
            main_build.FuchsiaBuildContext,
            rbe_enabled=mock.PropertyMock(return_value=True),
            get_rbe_reproxy_configs=lambda s: [pathlib.Path("cfg")],
        ):
            with self.mock_invocation_context():
                invocation = main_build.BuildInvocation(context)
                prefix = list(invocation.top_build_command_prefix())
                self.assertIn("--rbe", prefix)
                self.assertIn("--reproxy-cfg", prefix)
                self.assertIn("--resultstore", prefix)

    def test_tui(self) -> None:
        context = self.create_context(tui=True)
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)
            prefix = list(invocation.top_build_command_prefix())
            self.assertIn("--tui", prefix)


class InjectNinjaArgsTest(MainBuildTestBase):
    def test_injection(self) -> None:
        context = self.create_context()
        with self.mock_invocation_context() as (mock_mkdir, _):
            invocation = main_build.BuildInvocation(context)
            cmd = ["ninja", "target"]
            injected = invocation._inject_ninja_args(cmd)
            self.assertEqual(injected[0], "ninja")
            self.assertIn("--dirty_sources_list", injected)
            self.assertIn("--action_metrics_output", injected)
            self.assertEqual(injected[-1], "target")
            mock_mkdir.assert_any_call(invocation.log_dir / "ninja_logs")


class NewBuildCommandExecutionTest(MainBuildTestBase):
    def test_new_build_command_execution_ninja(self) -> None:
        context = self.create_context(rbe=False, resultstore="none")
        with self.mock_invocation_context("uuid-123", "ts-456"):
            invocation = main_build.BuildInvocation(context)
            with mock.patch.multiple(
                main_build.FuchsiaBuildContext,
                rbe_enabled=mock.PropertyMock(return_value=False),
                needs_auth=mock.PropertyMock(return_value=False),
            ):
                with mock.patch.object(main_build, "mkdir"):
                    exec_info = invocation.new_build_command_execution(
                        "ninja", ["ninja", "target"]
                    )
                    self.assertEqual(
                        exec_info.full_command[0],
                        str(context.top_build_wrapper),
                    )
                    self.assertIn("--", exec_info.full_command)
                    self.assertEqual(exec_info.env["FX_BUILD_UUID"], "uuid-123")

    def test_new_build_command_execution_ninja_resultstore(self) -> None:
        context = self.create_context(rbe=False, resultstore="ninja")
        with self.mock_invocation_context("uuid-123", "ts-456"):
            invocation = main_build.BuildInvocation(context)
            with mock.patch.object(main_build, "mkdir"):
                exec_info = invocation.new_build_command_execution(
                    "ninja", ["ninja", "target"]
                )
                self.assertIn("--post-build-uploads", exec_info.full_command)
                metrics_path = (
                    invocation.log_dir
                    / "ninja_logs"
                    / "ninja_action_metrics.json"
                )
                self.assertIn(str(metrics_path), exec_info.full_command)


class PrepareFunctionsTest(MainBuildTestBase):
    def test_bazel(self) -> None:
        context = self.create_context()
        with self.mock_invocation_context():
            exec_info = main_build.new_bazel_build_command_execution(
                context, ["build", "target"]
            )
            self.assertIsInstance(exec_info, main_build.BuildCommandExecution)
            self.assertIn("bazel", exec_info.full_command)

    def test_bazel_non_build(self) -> None:
        context = self.create_context()
        with self.mock_invocation_context():
            exec_info = main_build.new_bazel_build_command_execution(
                context, ["info"]
            )
            self.assertIsInstance(exec_info, main_build.BuildCommandExecution)
            self.assertFalse(exec_info.is_build)
            self.assertNotIn(
                str(context.top_build_wrapper), exec_info.full_command
            )
            self.assertEqual(exec_info.full_command, ["bazel", "info"])

    def test_ninja_non_build(self) -> None:
        context = self.create_context()
        with self.mock_invocation_context():
            exec_info = main_build.new_ninja_build_command_execution(
                context, ["--help"]
            )
            self.assertIsInstance(exec_info, main_build.BuildCommandExecution)
            self.assertFalse(exec_info.is_build)
            self.assertNotIn(
                str(context.top_build_wrapper), exec_info.full_command
            )
            self.assertIn("--help", exec_info.full_command)

    def test_fint(self) -> None:
        context = self.create_context()
        context.config.fint_params_path = pathlib.Path("/tmp/static.proto")
        with self.mock_invocation_context():
            exec_info = main_build.new_other_build_command_execution(
                context, ["ls", "-l"]
            )
            self.assertIsInstance(exec_info, main_build.BuildCommandExecution)
            self.assertTrue(
                any(
                    "fint_build.py" in str(arg)
                    for arg in exec_info.full_command
                )
            )
            self.assertEqual(len(exec_info.cleanup_files), 0)

    def test_other(self) -> None:
        context = self.create_context()
        with self.mock_invocation_context():
            exec_info = main_build.new_other_build_command_execution(
                context, ["ls", "-l"]
            )
            self.assertIsInstance(exec_info, main_build.BuildCommandExecution)
            self.assertIn("ls", exec_info.full_command)
            self.assertIn("-l", exec_info.full_command)

    def test_ninja_missing_j_arg(self) -> None:
        context = self.create_context()
        with self.assertRaises(main_build.BuildConfigurationError) as cm:
            main_build.new_ninja_build_command_execution(context, ["-j"])
        self.assertEqual(str(cm.exception), "-j requires an argument")

    def test_ninja_concurrency_cap(self) -> None:
        context = self.create_context(rbe=True, max_concurrency=64)
        with mock.patch.object(main_build, "get_cpu_count", return_value=96):
            with self.mock_invocation_context():
                exec_info = main_build.new_ninja_build_command_execution(
                    context, ["default"]
                )
                j_idx = exec_info.full_command.index("-j")
                self.assertEqual(exec_info.full_command[j_idx + 1], "64")

    def test_ninja_explicit_j_overrides_cap(self) -> None:
        context = self.create_context(rbe=True, max_concurrency=64)
        with mock.patch.object(main_build, "get_cpu_count", return_value=96):
            with self.mock_invocation_context():
                exec_info = main_build.new_ninja_build_command_execution(
                    context, ["-j", "128", "default"]
                )
                j_idx = exec_info.full_command.index("-j")
                self.assertEqual(exec_info.full_command[j_idx + 1], "128")


class CheckShellCommandTest(unittest.TestCase):
    @mock.patch.object(shutil, "which", return_value="/usr/bin/ls")
    def test_success(self, mock_which: mock.Mock) -> None:
        self.assertTrue(main_build.check_shell_command("ls"))
        mock_which.assert_called_once_with("ls")

    @mock.patch.object(shutil, "which", return_value=None)
    def test_failure(self, mock_which: mock.Mock) -> None:
        self.assertFalse(main_build.check_shell_command("nonexistent"))


class MainFunctionTest(MainBuildTestBase):
    def test_arg_parser_defaults(self) -> None:
        args = main_build._MAIN_ARG_PARSER.parse_args(
            ["--build-dir", "out/default", "ninja"]
        )
        self.assertIsNone(args.rbe)
        self.assertIsNone(args.resultstore)
        self.assertIsNone(args.tui)
        self.assertFalse(args.verbose)
        self.assertTrue(args.status)

    def test_arg_parser_no_status(self) -> None:
        args = main_build._MAIN_ARG_PARSER.parse_args(
            ["--build-dir", "out/default", "--no-status", "ninja"]
        )
        self.assertFalse(args.status)

    def test_main_catches_config_error(self) -> None:
        mock_args = default_args()
        mock_args.func = mock.Mock()
        with mock.patch.object(
            main_build._MAIN_ARG_PARSER, "parse_known_args"
        ) as mock_parse:
            mock_parse.return_value = (mock_args, [])
            mock_args.func.side_effect = main_build.BuildConfigurationError(
                "test error"
            )
            with mock.patch.object(main_build.FuchsiaBuildContext, "from_args"):
                with mock.patch.object(builtins, "print") as mock_print:
                    rc = main_build.main(
                        ["--build-dir", "out/default", "ninja"]
                    )
                    self.assertEqual(rc, 1)
                    mock_print.assert_called_with(
                        "[main_build.py] Error: test error", file=sys.stderr
                    )

    def test_main_catches_keyboard_interrupt(self) -> None:
        mock_args = default_args()
        mock_args.func = mock.Mock()
        with mock.patch.object(
            main_build._MAIN_ARG_PARSER, "parse_known_args"
        ) as mock_parse:
            mock_parse.return_value = (mock_args, [])
            mock_args.func.side_effect = KeyboardInterrupt
            with mock.patch.object(
                main_build.FuchsiaBuildContext, "from_args"
            ) as mock_from_args:
                with mock.patch.object(builtins, "print") as mock_print:
                    rc = main_build.main(
                        ["--build-dir", "out/default", "ninja"]
                    )
                    self.assertEqual(rc, 130)
                    mock_print.assert_called_with(
                        "[main_build.py] Received KeyboardInterrupt, exiting (130)",
                        file=sys.stderr,
                    )

    def test_main_catches_build_interrupted_error(self) -> None:
        mock_args = default_args()
        mock_args.func = mock.Mock()
        with mock.patch.object(
            main_build._MAIN_ARG_PARSER, "parse_known_args"
        ) as mock_parse:
            mock_parse.return_value = (mock_args, [])
            mock_args.func.side_effect = signal_utils.BuildInterruptedError(
                137, signal.SIGKILL
            )
            with mock.patch.object(
                main_build.FuchsiaBuildContext, "from_args"
            ) as mock_from_args:
                with mock.patch.object(builtins, "print") as mock_print:
                    rc = main_build.main(
                        ["--build-dir", "out/default", "ninja"]
                    )
                    self.assertEqual(rc, 137)
                    mock_print.assert_called_with(
                        "[main_build.py] Interrupted by SIGKILL, exiting (137)",
                        file=sys.stderr,
                    )


class BuildCommandSignalTest(MainBuildTestBase):
    @mock.patch.object(signal_utils, "SignalManagedProcess")
    def test_signal_forwarding_no_tui(self, mock_managed: mock.Mock) -> None:
        """Verify that without TUI, we use a separate process group."""
        context = self.create_context(tui=False)
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)

        exec_info = main_build.BuildCommandExecution(
            full_command=["sleep", "10"],
            env={"FOO": "BAR"},
            invocation=invocation,
        )

        mock_instance = mock_managed.return_value
        mock_instance.run.return_value = 0

        _ = exec_info._run_without_locking()

        mock_managed.assert_called_once_with(
            exec_info.full_command,
            env=exec_info.env,
            separate_pgrp=True,
            verbose=False,
        )
        mock_instance.run.assert_called_once()

    @mock.patch.object(signal_utils, "SignalManagedProcess")
    def test_signal_forwarding_with_tui(self, mock_managed: mock.Mock) -> None:
        """Verify that with TUI, we do NOT use a separate process group."""
        context = self.create_context(tui=True)
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)

        exec_info = main_build.BuildCommandExecution(
            full_command=["sleep", "10"],
            env={"FOO": "BAR"},
            invocation=invocation,
        )

        mock_instance = mock_managed.return_value
        mock_instance.run.return_value = 0

        _ = exec_info._run_without_locking()

        mock_managed.assert_called_once_with(
            exec_info.full_command,
            env=exec_info.env,
            separate_pgrp=False,
            verbose=False,
        )
        mock_instance.run.assert_called_once()

    @mock.patch.object(signal_utils, "SignalManagedProcess")
    def test_wait_resilience_to_interrupt(
        self, mock_managed: mock.Mock
    ) -> None:
        """Verify that wait() resilience is handled by SignalManagedProcess."""
        context = self.create_context()
        with self.mock_invocation_context():
            invocation = main_build.BuildInvocation(context)

        exec_info = main_build.BuildCommandExecution(
            full_command=["sleep", "10"],
            env={},
            invocation=invocation,
        )

        mock_instance = mock_managed.return_value
        # Simulate that SignalManagedProcess.run() handles the interrupt
        # and returns the exit status.
        mock_instance.run.return_value = 130

        result = exec_info._run_without_locking()
        self.assertEqual(result.return_code, 130)
        mock_instance.run.assert_called_once()


class ContextPropertiesAndLoggingTest(MainBuildTestBase):
    def test_context_properties(self) -> None:
        # Create context without fint-params
        config = main_build.FuchsiaBuildConfig(
            rbe=False,
            resultstore="none",
            profile=False,
            tui=False,
            verbose=False,
            dry_run=False,
            auth_mode="auto",
        )
        context = main_build.FuchsiaBuildContext(
            source_dir=pathlib.Path("/tmp/fuchsia"),
            out_dir=pathlib.Path("/tmp/out"),
            build_dir=pathlib.Path("/tmp/out/default"),
            env={"USER": "fake-user"},
            config=config,
        )

        # 1. Verify fint_build_py resolved path
        self.assertEqual(
            context.fint_build_py,
            pathlib.Path("/tmp/fuchsia/tools/integration/fint/fint_build.py"),
        )

        # Verify that the resolved Python binary path is absolute.
        self.assertTrue(main_build.PYTHON_BIN.is_absolute())

        # 2. Verify fint_build_cmd is empty when not specified
        self.assertEqual(list(context.fint_build_cmd()), [])

        # 3. Verify fint_build_cmd is fully populated when specified
        context.config.fint_params_path = pathlib.Path("/tmp/static.proto")
        expected_cmd = [
            str(main_build.PYTHON_BIN),
            "-S",
            "-u",
            "/tmp/fuchsia/tools/integration/fint/fint_build.py",
            "--static",
            "/tmp/static.proto",
            "--",
        ]
        self.assertEqual(
            [str(arg) for arg in context.fint_build_cmd()],
            expected_cmd,
        )

        # 4. Verify fint_build_cmd forwards context path when specified
        context.config.fint_context_path = pathlib.Path("/tmp/context.proto")
        expected_cmd_with_context = [
            str(main_build.PYTHON_BIN),
            "-S",
            "-u",
            "/tmp/fuchsia/tools/integration/fint/fint_build.py",
            "--static",
            "/tmp/static.proto",
            "--context",
            "/tmp/context.proto",
            "--",
        ]
        self.assertEqual(
            [str(arg) for arg in context.fint_build_cmd()],
            expected_cmd_with_context,
        )

    def test_fint_build_cmd_verbose(self) -> None:
        """Verifies that fint_build_cmd forwards the --verbose flag when verbose is True."""
        config = main_build.FuchsiaBuildConfig(
            rbe=False,
            resultstore="none",
            profile=False,
            tui=False,
            verbose=True,
            dry_run=False,
            auth_mode="auto",
        )
        context = main_build.FuchsiaBuildContext(
            source_dir=pathlib.Path("/tmp/fuchsia"),
            out_dir=pathlib.Path("/tmp/out"),
            build_dir=pathlib.Path("/tmp/out/default"),
            env={"USER": "fake-user"},
            config=config,
        )
        context.config.fint_params_path = pathlib.Path("/tmp/static.proto")
        expected_cmd = [
            str(main_build.PYTHON_BIN),
            "-S",
            "-u",
            "/tmp/fuchsia/tools/integration/fint/fint_build.py",
            "--static",
            "/tmp/static.proto",
            "--verbose",
            "--",
        ]
        self.assertEqual(
            [str(arg) for arg in context.fint_build_cmd()],
            expected_cmd,
        )

    def test_msg_logging(self) -> None:
        f_stdout = io.StringIO()
        with contextlib.redirect_stdout(f_stdout):
            main_build.msg("hello stdout")
        self.assertEqual(f_stdout.getvalue(), "[main_build.py] hello stdout\n")

        f_stderr = io.StringIO()
        with contextlib.redirect_stderr(f_stderr):
            main_build.msg("hello stderr", file=sys.stderr)
        self.assertEqual(f_stderr.getvalue(), "[main_build.py] hello stderr\n")

    def test_output_metadata_json(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = pathlib.Path(tmpdir)
            output_json_path = tmp_path / "metadata.json"
            build_dir = tmp_path / "build"
            out_dir = tmp_path / "out"
            log_dir = out_dir / "_build_logs/build_name/invocation_logs"
            reproxy_log_dir = log_dir / "reproxy_logs"

            # Create required directories and files
            main_build.mkdir(build_dir)
            main_build.mkdir(out_dir)
            main_build.mkdir(reproxy_log_dir)

            fuchsia_gn_trace = build_dir / "fuchsia_gn_trace.json"
            main_build.write_text(fuchsia_gn_trace, "[]")

            # Create dynamic mock context file containing artifact_dir
            context_proto = tmp_path / "context.proto"
            artifact_dir_path = tmp_path / "artifacts"
            main_build.mkdir(artifact_dir_path)
            main_build.write_text(
                context_proto, f'artifact_dir: "{artifact_dir_path}"\n'
            )

            # Create mock build_artifacts.json
            build_artifacts_file = artifact_dir_path / "build_artifacts.json"
            main_build.write_text(build_artifacts_file, "{}")

            # Create mock reproxy files
            main_build.write_text(
                reproxy_log_dir / "bootstrap.INFO", "bootstrap"
            )
            main_build.write_text(reproxy_log_dir / "reproxy.INFO", "reproxy")
            main_build.write_text(
                reproxy_log_dir / "reproxy_log.pb", "reproxy_log_pb"
            )
            main_build.write_text(
                reproxy_log_dir / "rbe_metrics.txt", "metrics"
            )
            main_build.write_text(reproxy_log_dir / "reproxy_run.rrpl", "rrpl")

            # Create mock build_profile files
            build_profile_dir = log_dir / "build_profile"
            main_build.mkdir(build_profile_dir)
            main_build.write_text(
                build_profile_dir / "system_profile.json", "{}"
            )
            main_build.write_text(
                build_profile_dir / "hardware_profile.json", "{}"
            )

            config = main_build.FuchsiaBuildConfig(
                rbe=True,
                resultstore="all",
                profile=True,
                tui=True,
                verbose=False,
                dry_run=False,
                auth_mode="auto",
                fint_params_path=None,
                fint_context_path=context_proto,
                output_metadata_json=output_json_path,
            )

            context = main_build.FuchsiaBuildContext(
                source_dir=pathlib.Path("/tmp"),
                out_dir=out_dir,
                build_dir=build_dir,
                env={"USER": "fake-user"},
                config=config,
            )

            invocation = main_build.BuildInvocation(context)
            with mock.patch.object(
                main_build.BuildInvocation,
                "log_dir",
                new_callable=mock.PropertyMock,
                return_value=log_dir,
            ), mock.patch.object(
                main_build.FuchsiaBuildContext,
                "fint_artifact_dir",
                new_callable=mock.PropertyMock,
                return_value=artifact_dir_path,
            ):
                invocation.write_metadata_json(output_json_path)

            # Verify contents of written JSON
            self.assertTrue(output_json_path.exists())
            with open(output_json_path, "r") as f:
                data = json.load(f)

            self.assertEqual(
                data["fint_build_artifacts"],
                str(build_artifacts_file.resolve()),
            )
            self.assertEqual(data["gn_trace"], str(fuchsia_gn_trace.resolve()))
            self.assertEqual(
                data["rbe"]["log_dir"], str(reproxy_log_dir.resolve())
            )
            self.assertEqual(
                data["rbe"]["diagnostic_logs"]["bootstrap.INFO"],
                str((reproxy_log_dir / "bootstrap.INFO").resolve()),
            )
            self.assertEqual(
                data["rbe"]["diagnostic_logs"]["reproxy.INFO"],
                str((reproxy_log_dir / "reproxy.INFO").resolve()),
            )
            self.assertEqual(
                data["rbe"]["diagnostic_logs"]["rbe_metrics.txt"],
                str((reproxy_log_dir / "rbe_metrics.txt").resolve()),
            )
            self.assertIn(
                str((reproxy_log_dir / "reproxy_run.rrpl").resolve()),
                data["rbe"]["cas_upload_candidates"],
            )
            self.assertEqual(
                data["rbe"]["reproxy_log_pb"],
                str((reproxy_log_dir / "reproxy_log.pb").resolve()),
            )
            self.assertEqual(
                data["build_profile"]["system_profile"],
                str((build_profile_dir / "system_profile.json").resolve()),
            )
            self.assertEqual(
                data["build_profile"]["hardware_profile"],
                str((build_profile_dir / "hardware_profile.json").resolve()),
            )

    @mock.patch.object(subprocess, "check_output")
    def test_fint_artifact_dir_success(
        self, mock_check_output: mock.Mock
    ) -> None:
        mock_check_output.return_value = "/mock/resolved/artifacts\n"
        config = main_build.FuchsiaBuildConfig(
            rbe=True,
            resultstore="all",
            profile=True,
            tui=True,
            verbose=False,
            dry_run=False,
            auth_mode="auto",
            fint_params_path=pathlib.Path("/tmp/static.proto"),
            fint_context_path=pathlib.Path("/tmp/context.proto"),
            output_metadata_json=None,
        )
        context = main_build.FuchsiaBuildContext(
            source_dir=pathlib.Path("/tmp/fuchsia"),
            out_dir=pathlib.Path("/tmp/fuchsia/out/default"),
            build_dir=pathlib.Path("/tmp/fuchsia/out/default"),
            env={"USER": "fake-user"},
            config=config,
        )
        self.assertEqual(
            context.fint_artifact_dir, pathlib.Path("/mock/resolved/artifacts")
        )
        mock_check_output.assert_called_once_with(
            [
                str(main_build.PYTHON_BIN),
                "-S",
                "-u",
                "/tmp/fuchsia/tools/integration/fint/fint_build.py",
                "--context",
                "/tmp/context.proto",
                "--print-artifact-dir",
            ],
            text=True,
            stderr=subprocess.PIPE,
        )

    @mock.patch.object(subprocess, "check_output")
    def test_fint_artifact_dir_failure(
        self, mock_check_output: mock.Mock
    ) -> None:
        mock_check_output.side_effect = subprocess.CalledProcessError(2, "cmd")
        config = main_build.FuchsiaBuildConfig(
            rbe=True,
            resultstore="all",
            profile=True,
            tui=True,
            verbose=False,
            dry_run=False,
            auth_mode="auto",
            fint_params_path=pathlib.Path("/tmp/static.proto"),
            fint_context_path=pathlib.Path("/tmp/context.proto"),
            output_metadata_json=None,
        )
        context = main_build.FuchsiaBuildContext(
            source_dir=pathlib.Path("/tmp/fuchsia"),
            out_dir=pathlib.Path("/tmp/fuchsia/out/default"),
            build_dir=pathlib.Path("/tmp/fuchsia/out/default"),
            env={"USER": "fake-user"},
            config=config,
        )
        self.assertIsNone(context.fint_artifact_dir)

    def test_write_metadata_json_no_profile_dir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = pathlib.Path(tmp_dir)
            log_dir = tmp_path / "logs"
            out_dir = tmp_path / "out"
            build_dir = tmp_path / "out/default"
            output_json_path = tmp_path / "metadata.json"

            main_build.mkdir(log_dir)
            main_build.mkdir(build_dir)
            main_build.mkdir(out_dir)

            context = self.create_context(
                env={"USER": "fake-user"},
                rbe=False,
                resultstore="none",
                profile=True,
                tui=False,
                verbose=False,
                dry_run=False,
                output_metadata_json=output_json_path,
            )
            # Force the context out_dir and build_dir paths to use the temp directory
            context.out_dir = out_dir
            context.build_dir = build_dir

            invocation = main_build.BuildInvocation(context)
            with mock.patch.object(
                main_build.BuildInvocation,
                "log_dir",
                new_callable=mock.PropertyMock,
                return_value=log_dir,
            ):
                invocation.write_metadata_json(output_json_path)
                with open(output_json_path, "r") as f:
                    data = json.load(f)
                self.assertNotIn("build_profile", data)

    def test_write_metadata_json_empty_profile_dir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = pathlib.Path(tmp_dir)
            log_dir = tmp_path / "logs"
            out_dir = tmp_path / "out"
            build_dir = tmp_path / "out/default"
            output_json_path = tmp_path / "metadata.json"

            main_build.mkdir(log_dir)
            main_build.mkdir(build_dir)
            main_build.mkdir(out_dir)

            build_profile_dir = log_dir / "build_profile"
            main_build.mkdir(build_profile_dir)

            context = self.create_context(
                env={"USER": "fake-user"},
                rbe=False,
                resultstore="none",
                profile=True,
                tui=False,
                verbose=False,
                dry_run=False,
                output_metadata_json=output_json_path,
            )
            # Force the context out_dir and build_dir paths to use the temp directory
            context.out_dir = out_dir
            context.build_dir = build_dir

            invocation = main_build.BuildInvocation(context)
            with mock.patch.object(
                main_build.BuildInvocation,
                "log_dir",
                new_callable=mock.PropertyMock,
                return_value=log_dir,
            ):
                invocation.write_metadata_json(output_json_path)
                with open(output_json_path, "r") as f:
                    data = json.load(f)
                self.assertNotIn("build_profile", data)

    def test_write_metadata_json_partial_profiles(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = pathlib.Path(tmp_dir)
            log_dir = tmp_path / "logs"
            out_dir = tmp_path / "out"
            build_dir = tmp_path / "out/default"
            output_json_path = tmp_path / "metadata.json"

            main_build.mkdir(log_dir)
            main_build.mkdir(build_dir)
            main_build.mkdir(out_dir)

            build_profile_dir = log_dir / "build_profile"
            main_build.mkdir(build_profile_dir)

            context = self.create_context(
                env={"USER": "fake-user"},
                rbe=False,
                resultstore="none",
                profile=True,
                tui=False,
                verbose=False,
                dry_run=False,
                output_metadata_json=output_json_path,
            )
            # Force the context out_dir and build_dir paths to use the temp directory
            context.out_dir = out_dir
            context.build_dir = build_dir

            invocation = main_build.BuildInvocation(context)
            with mock.patch.object(
                main_build.BuildInvocation,
                "log_dir",
                new_callable=mock.PropertyMock,
                return_value=log_dir,
            ):
                # 1. Only system_profile.json exists
                system_profile = build_profile_dir / "system_profile.json"
                main_build.write_text(system_profile, "{}")
                invocation.write_metadata_json(output_json_path)
                with open(output_json_path, "r") as f:
                    data = json.load(f)
                self.assertIn("build_profile", data)
                self.assertIn("system_profile", data["build_profile"])
                self.assertNotIn("hardware_profile", data["build_profile"])

                # 2. Both exist
                hardware_profile = build_profile_dir / "hardware_profile.json"
                main_build.write_text(hardware_profile, "{}")
                invocation.write_metadata_json(output_json_path)
                with open(output_json_path, "r") as f:
                    data = json.load(f)
                self.assertIn("build_profile", data)
                self.assertIn("system_profile", data["build_profile"])
                self.assertIn("hardware_profile", data["build_profile"])


if __name__ == "__main__":
    unittest.main()
