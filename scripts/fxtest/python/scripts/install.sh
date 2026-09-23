#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Creates a new virtual environment and pip installs fx test for testing, coverage, and linting.
# This file was adapted from //src/testing/end_to_end/honeydew/scripts/...

FXTEST_SRC="$FUCHSIA_DIR/scripts/fxtest/rewrite"

VENV_ROOT_PATH="$FXTEST_SRC/.venvs"
VENV_NAME="fuchsia_python_venv"
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

# upgrade the `pip` module
echo "Upgrading pip module..."
python -m pip install --upgrade pip

# install fxtest
echo "Installing 'fxtest' module..."
cd $FXTEST_SRC
python -m pip install --editable ".[test,guidelines]"

cd $STARTING_DIR

echo -e "Installation successful...\n"
