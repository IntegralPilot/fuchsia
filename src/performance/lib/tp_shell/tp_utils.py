# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Trace Processor wrapper utilities for Perfetto trace analysis."""

import hashlib
import importlib.resources
import logging
import os
import ssl
import tempfile
import urllib.request
import weakref
from types import TracebackType
from typing import Any

import perfetto.trace_processor.api
from perfetto.tools.download_trace import resolve_trace_url
from perfetto.trace_processor.api import TraceProcessor as PerfettoTP
from perfetto.trace_processor.api import TraceProcessorConfig
from perfetto.trace_processor.platform import PlatformDelegate
from perfetto.trace_uri_resolver import util as resolver_util
from perfetto.trace_uri_resolver.path import PathUriResolver
from perfetto.trace_uri_resolver.registry import ResolverRegistry
from perfetto.trace_uri_resolver.resolver import TraceUriResolver

_LOGGER = logging.getLogger(__name__)


class HttpUriResolver(TraceUriResolver):
    """URI Resolver that streams trace data from HTTP/HTTPS endpoints and permalinks."""

    PREFIX = "http"

    def __init__(self, uri: str) -> None:
        self.uri = uri

    @classmethod
    def from_trace_uri(cls, uri: str) -> "HttpUriResolver":
        return cls(uri)

    def resolve(self) -> list[TraceUriResolver.Result]:
        if "perfetto.dev" in self.uri:
            url = resolve_trace_url(self.uri)
        else:
            url = self.uri
        context = ssl._create_unverified_context()
        # Add a default timeout as a backstop to avoid permanently blocking when loading
        # the trace file.
        response = urllib.request.urlopen(url, context=context, timeout=120)
        return [
            TraceUriResolver.Result(
                trace=resolver_util.read_generator(response),
                metadata={"_url": url},
            )
        ]


class HttpsUriResolver(HttpUriResolver):
    PREFIX = "https"


class FuchsiaPlatformDelegate(PlatformDelegate):
    """PlatformDelegate that points directly to the host's prebuilt trace_processor_shell binary.

    This delegate is used to override the default PlatformDelegate in the
    third_party Perfetto Python SDK, ensuring that it uses our prebuilt, in-tree
    version of trace_processor_shell instead of trying to download or resolve it
    from the network or default paths.
    """

    host_tp_shell_path: str

    def __init__(self, host_tp_shell_path: str) -> None:
        super().__init__()
        self.host_tp_shell_path = host_tp_shell_path

    def get_shell_path(
        self, bin_path: str | None = None, fetch_latest: bool = False
    ) -> str:
        return self.host_tp_shell_path

    def get_resource(self, file: str) -> bytes:
        return (
            importlib.resources.files("perfetto.trace_processor")
            .joinpath(file)
            .read_bytes()
        )

    def default_resolver_registry(self) -> ResolverRegistry:
        return ResolverRegistry(
            resolvers=[PathUriResolver, HttpUriResolver, HttpsUriResolver]
        )


class PerfettoTraceProcessor:
    """A wrapper around Perfetto's official Python TraceProcessor API.

    This class provides a way to interact with Perfetto's TraceProcessor API,
    handling the setup and teardown of the trace_processor_shell backend.
    The trace file is parsed only once, and backend process is torn down
    when the processor is closed.

    Attributes:
        trace_path: Path to the trace file to ingest.
        tp_shell_path: Path to the trace_processor_shell binary.
        debug: If True, prints SQL queries.
    """

    trace_path: str
    tp_shell_path: str
    debug: bool
    _tp: PerfettoTP
    _finalizer: weakref.finalize
    _tp_shell_context: Any

    def __init__(
        self,
        trace_path: str,
        tp_shell_path: str | None = None,
        debug: bool = False,
        cache: bool = True,
    ) -> None:
        """Initializes PerfettoTraceProcessor.

        Args:
            trace_path: Path to the trace file to ingest.
            tp_shell_path: Optional path to the trace_processor_shell binary.
            debug: If True, prints SQL queries.
            cache: If True and trace_path is a URL, caches trace locally (default: True).
        """
        self._tp_shell_context = None

        if tp_shell_path is None:
            try:
                # The logic here involving _tp_shell_context and its __enter__/__exit__
                # methods is necessary to correctly manage the lifecycle of the extracted
                # trace_processor_shell binary when loaded from package resources.
                # importlib.resources.as_file returns a context manager that extracts
                # the resource to a temporary location. We must call __enter__() to get
                # the path and ensure __exit__() is called for cleanup.
                resource = importlib.resources.files("tp_shell.bin").joinpath(
                    "trace_processor_shell"
                )
                self._tp_shell_context = importlib.resources.as_file(resource)
                # Enter the extraction context to obtain a real filesystem path
                extracted_path = self._tp_shell_context.__enter__()
                tp_shell_path = str(extracted_path)
                # Ensure the extracted file is executable (necessary in ZIP contexts)
                os.chmod(tp_shell_path, 0o755)
            except Exception as e:
                if self._tp_shell_context is not None:
                    try:
                        self._tp_shell_context.__exit__(None, None, None)
                    except Exception:
                        pass
                raise FileNotFoundError(
                    "trace_processor_shell was not found in the packaged resources. "
                    "The binary must either be explicitly specified or packaged as a data source dependency."
                ) from e

        self.tp_shell_path = os.path.abspath(tp_shell_path)
        if not os.path.exists(self.tp_shell_path):
            raise FileNotFoundError(
                f"Trace processor shell not found: {self.tp_shell_path}"
            )
        self.debug = debug

        delegate = FuchsiaPlatformDelegate(self.tp_shell_path)

        if trace_path.startswith("http://") or trace_path.startswith(
            "https://"
        ):
            if cache:
                registry = delegate.default_resolver_registry()
                self.trace_path = os.path.abspath(
                    self._resolve_and_cache_url(trace_path, registry)
                )
            else:
                self.trace_path = trace_path
        else:
            self.trace_path = os.path.abspath(trace_path)
            if not os.path.exists(self.trace_path):
                raise FileNotFoundError(
                    f"Trace file not found: {self.trace_path}"
                )

        # Override Perfetto's PlatformDelegate to point directly to our prebuilt shell binary
        perfetto.trace_processor.api.PLATFORM_DELEGATE = lambda: delegate

        # Configure TraceProcessor to use a unique port to avoid conflicts
        config = TraceProcessorConfig(unique_port=True)

        _LOGGER.info(
            f"Initializing Perfetto TraceProcessor for trace: {self.trace_path}"
        )
        try:
            self._tp = PerfettoTP(trace=self.trace_path, config=config)
        except Exception:
            if self._tp_shell_context is not None:
                try:
                    self._tp_shell_context.__exit__(None, None, None)
                except Exception:
                    pass
            raise

        # Register a finalizer to ensure the subprocess is cleaned up even if close() isn't called.
        self._finalizer = weakref.finalize(
            self, self._cleanup, self._tp, self._tp_shell_context
        )

    @staticmethod
    def _cleanup(tp: PerfettoTP, context: Any = None) -> None:
        """Safely tears down the Perfetto TraceProcessor shell process and deletes temporary extraction files."""
        _LOGGER.info("Tearing down Perfetto TraceProcessor shell process...")
        try:
            tp.close()
        except Exception as e:
            _LOGGER.error(f"Error closing trace processor: {e}")

        if context is not None:
            _LOGGER.info(
                "Cleaning up temporary trace_processor_shell extraction..."
            )
            try:
                context.__exit__(None, None, None)
            except Exception as e:
                _LOGGER.error(f"Error cleaning up extraction context: {e}")

    def run_query(self, query: str) -> list[dict[str, Any]]:
        """Runs a SQL query against the trace and returns the result as a list of dicts."""
        if not hasattr(self, "_finalizer") or not self._finalizer.alive:
            raise RuntimeError("Trace processor is closed.")

        if self.debug:
            _LOGGER.debug(
                f"--- DEBUG SQL QUERY ---\n{query.strip()}\n-----------------------"
            )

        try:
            result_iterator = self._tp.query(query)
            # Row.__dict__ on instance contains exactly the dynamic attributes (columns)
            return [row.__dict__ for row in result_iterator]
        except Exception as e:
            _LOGGER.error(f"Error running query: {e}")
            raise

    def get_tables(self) -> set[str]:
        """Returns the set of table and view names available in the trace database."""
        rows = self.run_query(
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
        )
        return {row["name"] for row in rows}

    def __enter__(self) -> "PerfettoTraceProcessor":
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> None:
        self.close()

    def close(self) -> None:
        """Tears down the trace processor shell process."""
        if hasattr(self, "_finalizer") and self._finalizer.alive:
            self._finalizer()

    @staticmethod
    def _resolve_and_cache_url(url: str, registry: ResolverRegistry) -> str:
        cache_key = hashlib.sha256(url.encode("utf-8")).hexdigest()
        cache_dir = os.path.join(tempfile.gettempdir(), "perf_analyze_cache")
        cached_file = os.path.join(cache_dir, f"{cache_key}.fxt")

        if os.path.exists(cached_file) and os.path.getsize(cached_file) > 0:
            _LOGGER.info("Using cached trace: %s", cached_file)
            return cached_file

        results = registry.resolve(url)
        if not results:
            raise ValueError(f"Could not resolve URL: {url}")

        os.makedirs(cache_dir, exist_ok=True)
        _LOGGER.info(
            "Downloading trace URL %s to cache %s...", url, cached_file
        )
        temp_file = f"{cached_file}.tmp.{os.getpid()}"
        try:
            with open(temp_file, "wb") as out:
                for chunk in results[0].generator:
                    out.write(chunk)
            os.replace(temp_file, cached_file)
        except Exception:
            if os.path.exists(temp_file):
                os.remove(temp_file)
            raise
        return cached_file
