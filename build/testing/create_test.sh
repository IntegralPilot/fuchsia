#!/bin/bash

# Copyright 2020 The Fuchsia Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT

# It's inconvenient to create scripts with executable permissions directly in
# GN, so this is a script that creates scripts. The generated scripts are
# wrappers for tests that call host tools with arguments.

set -o errexit

declare -r OUTFILE="${1}"
shift 1

declare -r TOOL="${1}"
shift 1

declare TOOL_ARG_LISTER=""
declare -a TOOL_ARGS
declare -a ENV_VARS
for arg in "$@"
do
  if [[ $arg == env* ]]; then
      ENV_VARS+=("${arg}")
  elif [[ $arg == test_lister_path=* ]]; then
      TOOL_ARG_LISTER="${arg#test_lister_path=}"
  else
      TOOL_ARGS+=("'${arg}'")
  fi
done

echo "#!/bin/bash" > "$OUTFILE"
# TODO(betramlalusha): revert this change once debugging is done.
echo "set -e" >> "$OUTFILE"
echo "" >> "$OUTFILE"
if [[ -n "$TOOL_ARG_LISTER" ]]; then
  echo 'if [[ "$1" == "--list_host_python_unittests" ]]; then' >> "$OUTFILE"
  echo "  exec ${TOOL} ${TOOL_ARG_LISTER} --list_tests ${TOOL_ARGS[0]}" >> "$OUTFILE"
  echo 'else' >> "$OUTFILE"
fi

echo "  echo starting tool ${TOOL}" >> "$OUTFILE"
echo "  exec ${ENV_VARS[*]} ${TOOL} ${TOOL_ARGS[*]} \"\$@\"" >> "$OUTFILE"
echo "  echo executing tool ${TOOL} failed" >> "$OUTFILE"

if [[ -n "$TOOL_ARG_LISTER" ]]; then
  echo 'fi' >> "$OUTFILE"
fi
chmod a+x "$OUTFILE"