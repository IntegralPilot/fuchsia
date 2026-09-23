# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Perfetto Trace Processor wrapper library."""

from perfetto.trace_processor.api import TraceProcessorConfig
from tp_shell.tp_utils import FuchsiaPlatformDelegate, PerfettoTraceProcessor

__all__ = [
    "FuchsiaPlatformDelegate",
    "PerfettoTraceProcessor",
    "TraceProcessorConfig",
]
