#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A Jiri hook used to extract the pydantic_core wheel file to
# a hard-coded destination directory to make it directly usable
# before the build.
set -e

# Where to find the source wheel file. Exact name is versioned and unknown here.
SOURCE_SUBDIR="prebuilt/third_party/pydantic-core-wheel"

# Where to extract the wheel file.
DEST_SUBDIR="prebuilt/third_party/pydantic-core"

function die {
  echo >&2 "ERROR: $*"
  exit 1
}

# Assume this script lives under //tools/build/scripts/
_SCRIPT_DIR=$(dirname "${BASH_SOURCE[0]}")
FUCHSIA_DIR="$(readlink -f "${_SCRIPT_DIR}/../../..")"
[[ -f "${FUCHSIA_DIR}/.jiri_manifest" ]] ||
    die "Cannot locate proper FUCHSIA_DIR, got: ${FUCHSIA_DIR}"

# The source and destination directories are hard-coded here.
SOURCE_DIR="$FUCHSIA_DIR/$SOURCE_SUBDIR"
echo "Using source directory: $SOURCE_DIR"

# Locate the wheel file from the source directory.
SRC_FILE=$(ls "$SOURCE_DIR"/*.whl 2>/dev/null | head -n1)
[[ -n "$SRC_FILE" ]] || die "No .whl found in source directory: ${SOURCE_DIR}"

echo "Found wheel file: $SRC_FILE"

# Extract version number from the wheel file name, as this
# will be required for the README.fuchsia file.
SRC_FILENAME=$(basename "${SRC_FILE}")
SRC_VERSION=$(echo "${SRC_FILENAME}" | sed -E -e 's|pydantic_core-([0-9.]+)-.*|\1|')
[[ -n "${SRC_VERSION}" ]] || die "Could not find version in filename: ${SRC_FILENAME}"
echo "Found version: [$SRC_VERSION]"

# A file used to verify that the extracted content is correct.
CHECK_FILE=pydantic_core/__init__.py

DEST_DIR="${FUCHSIA_DIR}/${DEST_SUBDIR}"
echo "Using destination directory: ${DEST_DIR}"
mkdir -p "${DEST_DIR}"
rm -rf "${DEST_DIR:?}"/*

echo "Extracting archive"
unzip -o -q "${SRC_FILE}" -d "${DEST_DIR}" || die "Could not unzip ${SRC_FILE}"
[[ -f "${DEST_DIR}/${CHECK_FILE}" ]] ||
    die "Missing file from extracted archive: ${DEST_DIR}/${CHECK_FILE}"

# Find the compiled shared library (.so) file in the extracted package.
SO_FILE=$(ls "${DEST_DIR}/pydantic_core"/*.so 2>/dev/null | xargs -n1 basename | head -n1)
[[ -n "${SO_FILE}" ]] || die "Could not find native .so library in ${DEST_DIR}/pydantic_core"
echo "Found native shared library: ${SO_FILE}"

echo "Modifying __init__.py to support dynamic loading of shared library"
INIT_FILE="${DEST_DIR}/${CHECK_FILE}"
TEMP_INIT="${INIT_FILE}.tmp"

cat > "${TEMP_INIT}" <<EOF
from __future__ import annotations

import os
import sys
from importlib.abc import Loader
import importlib.util
import importlib.machinery
from pathlib import Path

def _init():
    finder = importlib.machinery.PathFinder()

    # Use the build directory set by the fx environment if available, as will be the case when 'fx
    # debug cli' uses this module. Fallback to PWD if not set, which will be the case in
    # infrastructure where tests are run using this module.
    build_dir_env = os.environ.get('FUCHSIA_BUILD_DIR_FROM_FX')
    build_dir = Path(build_dir_env) if build_dir_env else Path('.')

    # Relative path from root_build_dir.
    shlib_dir = build_dir / 'host_x64/gen/prebuilt/third_party/pydantic-core/pydantic_core'

    search_path = [str(shlib_dir)] + sys.path
    spec = finder.find_spec('_pydantic_core', path=search_path)
    if spec is None:
        raise Exception(f'Could not find _pydantic_core in {search_path}')
    mod = importlib.util.module_from_spec(spec)
    mod.__name__ = 'pydantic_core._pydantic_core'
    sys.modules['pydantic_core._pydantic_core'] = mod
    assert isinstance(spec.loader, Loader)
    spec.loader.exec_module(mod)

_init()
EOF

# Append original content, skipping 'from __future__' lines.
grep -v "^from __future__" "${INIT_FILE}" >> "${TEMP_INIT}"
mv "${TEMP_INIT}" "${INIT_FILE}"

echo "Generating BUILD.gn file"
cat > "${DEST_DIR}/BUILD.gn" <<EOF
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import("//build/python/python_library.gni")
import("//build/testing/host_test_data.gni")

if (is_host) {
  # Copy the extracted files and the DSO to the target gen directory so they can
  # be used by python_library.
  copy("copy_pydantic_core_files") {
    sources = [
      "pydantic_core/__init__.py",
      "pydantic_core/core_schema.py",
      "pydantic_core/${SO_FILE}",
    ]
    outputs = [
      "\$target_gen_dir/pydantic_core/{{source_file_part}}",
    ]
  }

  python_library("pydantic_core") {
    enable_mypy = false
    sources = [
      "__init__.py",
      "core_schema.py",
    ]
    source_root = "\$target_gen_dir/pydantic_core"

    deps = [
      ":copy_pydantic_core_files",
      "//third_party/pylibs/typing_extensions",
    ]
  }

  # Test targets should depend on this target to ensure that the library's
  # binary components are available at runtime.
  host_test_data("test_data") {
    sources = [
      "\$target_gen_dir/pydantic_core/__init__.py",
      "\$target_gen_dir/pydantic_core/core_schema.py",
      "\$target_gen_dir/pydantic_core/${SO_FILE}",
    ]

    deps = [ ":copy_pydantic_core_files" ]
  }
}
EOF

echo "Writing README.fuchsia file"
cat > "${DEST_DIR}/README.fuchsia" <<EOF
Name: pydantic_core
URL: https://chrome-infra-packages.appspot.com/p/infra/python/wheels/pydantic_core/
Version: ${SRC_VERSION}

License File: pydantic_core-${SRC_VERSION}.dist-info/licenses/LICENSE
 -> License File Format: Single License
 -> License Classifications: MIT

Description:
Prebuilt Pydantic Core library, which includes a native extension.
This directory contains the extracted wheel file.
The extraction script is in \$FUCHSIA_DIR/tools/build/scripts, and invoked
as a Jiri hook.

To update to a new version:

1) Modify the "version" attribute of the "<package>" element called
   "pydantic_core" in \$FUCHSIA_DIR/manifests/prebuilts

2) Run: \$FUCHSIA_DIR/manifests/update-lockfiles.sh
   This verifies that the version exists and updates critical Jiri files.

3) Run: jiri update -local-manifest-project=fuchsia
   This installs the new wheel file to \$FUCHSIA_DIR/${SOURCE_SUBDIR}
   then runs a hook to update \$FUCHSIA_DIR/${DEST_SUBDIR}

4) Verify that everything works

5) Update new CL to Gerrit with your modifications.
EOF

echo "Done"
exit 0
