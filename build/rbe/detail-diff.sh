#!/bin/bash
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A diff wrapper that analyzes differences with a variety of tools.
# Exit status reflects whether or not any differences were found
# including those from text dumps of binaries.

set -o pipefail

readonly script="$0"
# assume script is always with path prefix, e.g. "./$script"
readonly script_dir="${script%/*}"

source "$script_dir"/common-setup.sh

readonly project_root="$default_project_root"
readonly project_root_rel="$(relpath . "$project_root")"

readonly clang_dir_local="$project_root_rel"/prebuilt/third_party/clang/"$HOST_PLATFORM"

# Tools
readonly objdump="$clang_dir_local"/bin/llvm-objdump
readonly readelf="$clang_dir_local"/bin/llvm-readelf
readonly dwarfdump="$clang_dir_local"/bin/llvm-dwarfdump
readonly nm="$clang_dir_local"/bin/llvm-nm
readonly jq="$project_root_rel/prebuilt/third_party/jq/$HOST_PLATFORM/bin/jq"

# Configurable options
diff_limit=25
# Set the following with --left-suffix and --right-suffix
# to make the diff reports more meaningful in their context.
left_suffix=left
right_suffix=right

function usage() {
  cat <<EOF
Compares two files in human-readable detail.
usage: $script [options] left-file right-file
options:
  -n LINES : display first N lines of detailed differences
    [default: $diff_limit]
  -l SUFFIX : left file suffix when diff-ing output from another tool
    [default: $left_suffix]
  -r SUFFIX : right file suffix when diff-ing output from another tool
    [default: $right_suffix]
EOF
}

prev_opt=
positional_args=()
for opt
do
  # handle --option arg
  if [[ -n "$prev_opt" ]]
  then
    eval "$prev_opt"=\$opt
    prev_opt=
    shift
    continue
  fi

  # Extract optarg from --opt=optarg
  optarg=
  case "$opt" in
    -*=*) optarg="${opt#*=}" ;;  # remove-prefix, shortest-match
  esac

  case "$opt" in
    -h) usage; exit ;;
    -l) prev_opt=left_suffix ;;
    -l=*) left_suffix="$optarg" ;;
    -r) prev_opt=right_suffix ;;
    -r=*) right_suffix="$optarg" ;;
    -n) prev_opt=diff_limit ;;
    -n=*) diff_limit="$optarg" ;;
    --) shift ; break ;;
    -*) echo "Unknown $0 option: $opt" ; usage ; exit 1 ;;
    *) positional_args+=( "$opt" ) ;;
  esac
  shift
done

positional_args+=( "$@" )
set -- "${positional_args[@]}"
if [[ "$#" < 2 ]]
then
  echo "Requires 2 positional arguments, but missing at least 1."
  usage
  exit 1
fi
# positional arguments are $1 and $2

# Diff two files, run through a command: diff -u <(command $1) <(command $2)
# Usage: diff_with command [options] -- input1 input2
function diff_with() {
  local tool
  local inputs
  tool=()
  for token in "$@"
  do
    case "$token" in
      --) shift; break ;;
      *) tool+=("$token") ;;
    esac
    shift
  done

  # The rest of "$@" are input files.
  test "$#" = 2 || {
    echo "diff_with: Expected two inputs, but got $#."
    exit 1
  }

  # Some tools' output include the full name of the file
  # being examined, and behavior may depend on the file extension.
  # So use $1 as the canonical name for tool operation on both files.
  tool_suffix="$(basename "${tool[0]}")"

  "${tool[@]}" "$1" > "$1.$tool_suffix.$left_suffix"
  # Use the same name for the other file with a temporary move.
  mv "$1"{,.bkp}
  mv "$2" "$1"
  "${tool[@]}" "$1" > "$1.$tool_suffix.$right_suffix"
  # Restore the original names.
  mv "$1" "$2"
  mv "$1"{.bkp,}

  echo "diff -u <(${tool[@]} $1) <(${tool[@]} $2)"
  diff -u "$1.$tool_suffix.$left_suffix" "$1.$tool_suffix.$right_suffix"
}

function json_diff() {
  # format nicely using jq or jsonformat5
  diff_with "$jq" . -- "$1" "$2"
}

function zip_diff() {
  # compare the table of contents, including timestamps
  diff_with unzip -l -- "$1" "$2"
}

function binary_diff() {
  # Intended for binaries (rlibs, executables).
  # needs -o pipefail to propagate exit statuses
  local diff_status=0
  echo "objdump-diff (first $diff_limit lines):"
  diff_with "$objdump" --full-contents -- "$1" "$2" | head -n "$diff_limit" || { diff_status=$? ;}
  echo

  echo "readelf-diff (first $diff_limit lines):"
  diff_with "$readelf" -a -- "$1" "$2" | head -n "$diff_limit" || { diff_status=$? ;}
  echo

  echo "dwarfdump-diff (first $diff_limit lines):"
  diff_with "$dwarfdump" -a -- "$1" "$2" | head -n "$diff_limit" || { diff_status=$? ;}
  echo

  echo "nm-diff (first $diff_limit lines):"
  diff_with "$nm" -- "$1" "$2" | head -n "$diff_limit" || { diff_status=$? ;}
  echo

  if which strings
  then
    echo "strings-diff (first $diff_limit lines):"
    diff_with strings -- "$1" "$2" | head -n "$diff_limit" || { diff_status=$? ;}
  fi
  return "$diff_status"
}

function rustc_linker_map_diff() {
  # Workaround for linker map differences pointing to temp directories.
  # See b/427157992.
  # TODO: revert back to plain text diff after
  # https://github.com/rust-lang/rust/issues/142989 is resolved.
  diff_with sed \
    -e '/\.rustc$/,/\.symtab$/s|/tmp/rustc.*/|/tmp/rustcXXXXXXX/|' \
    -e '/\.rustc$/,/\.symtab$/s|lib.unstripped/rustc[^/]*/|lib.unstripped/rustcXXXXXXX/|' \
    -- "$1" "$2" | head -n "$diff_limit"
}

# main
case "$1" in
  *rust*.so.map)
    rustc_linker_map_diff "$1" "$2"
    ;;
  *.d | *.map | *.ll)
    echo "text diff (first $diff_limit lines):"
    diff -u "$1" "$2" | head -n "$diff_limit"
    ;;
  *.json)
    json_diff "$1" "$2" | head -n "$diff_limit"
    ;;
  # TODO: .bc LLVM bitcode
  *.a | *.o | *.so | *.rlib)
    binary_diff "$1" "$2"
    ;;
  *.zip)
    zip_diff "$1" "$2" | head -n "$diff_limit"
    ;;
  *)
    filetype="$(file "$1" | head -n 1 | sed -e "s|^$1: ||")"
    case "$filetype" in
      *executable* | *"shared object"* | *"ar archive"* | *ELF*relocatable* )
        binary_diff "$1" "$2"
        ;;
      *)
        # Unknown type, default to text.
        diff -u "$1" "$2" | head -n "$diff_limit"
    esac
    ;;
esac

exit "$?"
