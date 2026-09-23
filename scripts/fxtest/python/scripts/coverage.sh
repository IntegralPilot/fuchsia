#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This file was adapted from //src/testing/end_to_end/honeydew/scripts/...

set -e

# Formats the fx test code as per coding guidelines

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


echo "Running coverage tool..."
coverage \
    run \
    --source $FXTEST_SRC \
    -m unittest discover \
    --start-directory $FXTEST_SRC \
    --pattern "*_test.py"

echo "Generating coverage stats..."
coverage report -m

if [ "$1" = "--html" ] && [ ! -z "$2" ]
then
    if [ ! -d "$2" ]
    then
        echo "Error: Directory "$2" must exist HTML output"
    else
        echo "Writing HTML to $2"
        coverage html -d $2
    fi
fi

rm -rf .coverage
