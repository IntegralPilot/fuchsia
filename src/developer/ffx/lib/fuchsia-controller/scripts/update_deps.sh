#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Creates a new virtual environment and pip installs fuchsia controller
# for testing, coverage, and linting. This file was adapted from
# //src/testing/end_to_end/honeydew/scripts/...


FCT_SRC="$FUCHSIA_DIR/src/developer/ffx/lib/fuchsia-controller"

VENV_ROOT_PATH="$FCT_SRC/.venvs"
VENV_NAME="fuchsia_python_venv_deps"
VENV_PATH="$VENV_ROOT_PATH/$VENV_NAME"

set -e

# https://stackoverflow.com/questions/1871549/determine-if-python-is-running-inside-virtualenv
INSIDE_VENV=$(fuchsia-vendored-python -c 'import sys; print ("0" if (sys.base_prefix == sys.prefix) else "1")')
if [[ "$INSIDE_VENV" == "1" ]]; then
    echo "Inside a virtual environment. Deactivate it and then run this script..."
    exit 1
fi

# Create a virtual environment using `fuchsia-vendored-python`
STARTING_DIR=`pwd`
mkdir -p $VENV_ROOT_PATH

if [ -d $VENV_PATH ]
then
    echo "Directory '$VENV_PATH' already exists. Deleting it..."
    rm -rf $VENV_PATH
fi
echo "Creating a new virtual environment @ '$VENV_PATH'..."
fuchsia-vendored-python -m venv $VENV_PATH

# activate the virtual environment
echo "Activating the virtual environment..."
source $VENV_PATH/bin/activate

cd $FCT_SRC

echo "Installing tooling dependencies..."
python -m pip install --require-hashes -r base-tooling-requirements.txt

echo "Regenerating requirements.txt..."
pip-compile pyproject.toml --extra test --extra guidelines --resolver backtracking --generate-hashes

# deactivate venv
deactivate
echo "Rerunning install..."
./scripts/install.sh

cd $STARTING_DIR