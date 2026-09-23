# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#### CATEGORY=Run, inspect and debug
#### EXECUTABLE=${HOST_TOOLS_DIR}/perf-analyze
### Standalone Performance Analysis Tool
## usage: fx perf-analyze [--format <json|markdown|text>] <query|analyze> [args...]
##
## Subcommands:
##   query    Execute SQL queries against a trace file using Perfetto TraceProcessor
##   analyze  Execute specialized analysis plugins (e.g. binder, cpu)
##
## Options:
##   --format <json|markdown|text>  Output format (default: text)
##   --verbose                      Enable verbose debug logging
##   --no-cache                     Disable local caching of permalink URL traces
