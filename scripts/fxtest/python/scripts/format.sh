#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

# Formats the fxtest code as per coding guidelines
# This file was adapted from //src/testing/end_to_end/honeydew/scripts/...

FXTEST_SRC="$FUCHSIA_DIR/scripts/fxtest/rewrite"

VENV_ROOT_PATH="$FXTEST_SRC/.venvs"
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
OLD_PYTHONPATH=$PYTHONPATH
PYTHONPATH="$FUCHSIA_DIR"/third_party/pylibs/mypy_extensions/src:"$FUCHSIA_DIR"/third_party/pylibs/typing_extensions/src/src:"$FUCHSIA_DIR"/third_party/pylibs/mypy/src:$PYTHONPATH

# Execute the Mypy command with python path
MYPY_CMD="python3 -S -m mypy --config-file $FUCHSIA_DIR/pyproject.toml $FXTEST_SRC"
$MYPY_CMD >/dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "Code is 'mypy' compliant"
else
    echo
    echo "ERROR: Code is not 'mypy' compliant!"
    echo "ERROR: Please run below command sequence, fix all the issues and then rerun this script"
    echo "*************************************"
    echo "$ source $VENV_PATH/bin/activate"
    echo "$ PYTHONPATH=$PYTHONPATH $MYPY_CMD"
    echo "*************************************"
    echo
    exit 1
fi
PYTHONPATH=$OLD_PYTHONPATH
