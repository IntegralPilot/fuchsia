#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

# Formats the code as per coding guidelines
# This file was adapted from //src/testing/end_to_end/honeydew/scripts/...

FCT_SRC="$FUCHSIA_DIR/src/developer/ffx/lib/fuchsia-controller"

VENV_ROOT_PATH="$FCT_SRC/.venvs"
VENV_NAME="fuchsia_python_venv"
VENV_PATH="$VENV_ROOT_PATH/$VENV_NAME"

if [ -d $VENV_PATH ]
then
    echo "Activating the virtual environment..."
    source $VENV_PATH/bin/activate
else
    echo "Directory '$VENV_PATH' does not exists. Run the 'install.sh' script first..."
    exit 1
fi

cd $FUCHSIA_DIR

echo "Formatting code..."
fx format-code

echo "Checking types..."
mypy \
    $FCT_SRC \
    --config-file=${FCT_SRC}/pyproject.toml
