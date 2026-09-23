#!/bin/bash
# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# The bash_test_framework.sh script provides two ways to define a mock equivalent
# of a required executable: call 'btf::make_mock', or add the mocked executable path
# to the BT_MOCKED_TOOLS array in the host test script. This script provides the
# mock implementation, and provides options to tailor the behavior, results, and/or
# side effects when the mocked executable is invoked.
#
# When invoked, instead of running the real program, this program writes its state
# (such as, the arguments passed to the command) such that 'source'ing the state
# file will make the state information available to the caller.
#
# The mock state data (script) is written to a file with the same path as the script
# plus (if executed only once) a ".mock_state" extension (for example,
# "${executable_path}.mock_state"); however, if the mock executable is executed more
# than once, multiple files will be written, with the extension ".mock_state.<index>"
# (with index starting at "1", for example, "${executable_path}.mock_state.1",
# "${executable_path}.mock_state.2").
#
# After sourcing the ".mock_state(.n)" file, an array variable, "BT_MOCK_ARGS", will be
# set (or overwritten if a prior value was set) to the command name (the ${script})
# followed by the arguments passed to the mocked tool by the last caller. Also, if a
# ".mock_side_effects" file was provided, the variable "BT_SIDE_EFFECT_STATUS" will be
# set to the status returned from sourcing the ".mock_side_effects" file.
#
# Also (simply for convenience), if a "${executable_path}.mock_status" file was present,
# sourcing the "${executable_path}.mock_state(.n)" script will also result in the same
# status returned to the original caller.
#
# To generate a return status other than 0 (success), write the desired status int
# value to "${executable_path}.mock_status" before executing the mock.
#
# To generate a stdout result, similarly, write the desired output to
# "${executable_path}.mock_stdout" before executing the mock; and to generate stderr
# output, write the desired stderr output to "${executable_path}.mock_stderr".
#
# Additional side effects (actions to be taken by the mock script that have some
# actual effect, such as creating a file, or running another program) can be
# executed as well. Write desired actions in bash syntax to
# "${executable_path}.mock_side_effects", to be executed by 'source'ing the file.
# Any and all arguments passed to the mocked executable are forwarded to the
# sourced mock_side_effects script.
#
# Side effects run after writing stdout and stderr, allowing for a possible side
# effect that you may want the mock program to run forever (such as an infinite
# loop with a long sleep). Alternatively, your side effect program can write
# its own output.
#
# Supporting multiple calls to the same script with different outputs and
# side effects is supported by using index suffixed files, such as
# "${executable_path}.mock_stdout.1" or "${executable_path}.mock_side_effects.2"
# which will only be used for the n-th script invocation. The suffix-less version
# of the file, if available, will be used as a fallback otherwise.
#
# Limitations:
#   - Input from stdin is ignored. The only way to change the behavior is to
#     create the .mock_status, .mock_stdout, and/or .mock_stderr files.
#   - stdout results are written first, in entirety, followed by stderr results
#     (if supplied)

declare script="${BASH_SOURCE[0]}"
declare state_file="${script}.mock_state"
declare -i run_index=1
if [[ -e "${state_file}" ]]; then
  # Command was executed more than once. Use numeric suffixes.
  mv "${state_file}" "${state_file}.1"
  state_file="${state_file}.2"
  run_index=2
elif [[ -e "${state_file}.1" ]]; then
  declare -i index
  declare -i max_index=1
  for file in "${state_file}".*; do
    index=${file##*.}
    max_index=$(( index > max_index ? index : max_index ))
  done
  run_index=$((max_index+1))
  state_file="${state_file}.${run_index}"
fi

stdout_file="${script}.mock_stdout.${run_index}"
if [[ ! -e "${stdout_file}" ]]; then
  stdout_file="${script}.mock_stdout"
fi
if [[ -e "${stdout_file}" ]]; then
    cat "${stdout_file}"
fi

stderr_file="${script}.mock_stderr.${run_index}"
if [[ ! -e "${stderr_file}" ]]; then
  stderr_file="${script}.mock_stderr"
fi
if [[ -e "${stderr_file}" ]]; then
  >&2 cat "${stderr_file}"
fi

declare had_side_effect=false
declare -i side_effect_status=0
declare side_effect_file="${script}.mock_side_effects.${run_index}"
if [[ ! -e "${side_effect_file}" ]]; then
  side_effect_file="${script}.mock_side_effects"
fi
if [[ -e "${side_effect_file}" ]]; then
  # shellcheck source=/dev/null
  source "${side_effect_file}" "$@"
  side_effect_status=$?
  had_side_effect=true
fi

declare -i status=0
declare status_file="${script}.mock_status.${run_index}"
if [[ ! -e "${status_file}" ]]; then
  status_file="${script}.mock_status"
fi
if [[ -e "${status_file}" ]]; then
  status=$(cat "${status_file}")
elif ${had_side_effect}; then
  status=${side_effect_status}
fi

echo "#!/bin/bash" >>"${state_file}"

# Write the args into the state file.
#
# This is split into three steps, the middle of which writes the Bash array
# literal. The array is written using printf and %q to quote or escape the
# elements of the $@ array. This is important for a number of reasons:
#
# * Using escaped double quotes around $@ causes all of the arguments to be
#   concatenated into a single space-separated string.
# * Using escaped double quotes isn't safe if any item in the array contains a
#   double quotation mark.
# * Using printf allows all strings to be safely included in the array.
# * Using printf prevents variable expansion when the status file is sourced as
#   a script.
{
  printf "BT_MOCK_ARGS=( "
  printf "%q " "${script}" "$@"
  printf ")\n"
  if ${had_side_effect}; then
    echo "declare -i BT_MOCK_SIDE_EFFECT_STATUS=${side_effect_status}"
  fi
  echo "return ${status}"
} >> "${state_file}"

# If script was sourced, use 'return', otherwise use 'exit'
(return 0 2>/dev/null) && return ${status} || exit ${status}
