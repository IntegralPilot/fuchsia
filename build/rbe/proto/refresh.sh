#!/bin/bash -e
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# See usage() for description.

script="$0"
script_dir="$(dirname "$script")"
project_root="$(readlink -f "$script_dir"/../../..)"

# relative to $project_root:
readonly PROTOBUF_SRC=third_party/protobuf/src/src
# This is where the prebuilt protobuf python wheel is installed.
readonly PROTOBUF_WHEEL=prebuilt/third_party/protobuf-py3

function usage() {
  cat <<EOF
Usage: $0 [options]

This script updates the public protos needed for reproxy logs collection.

options:
  --reclient-srcdir DIR : location of re-client source
     If none is provided, then this will checkout the source into a temp dir.
  --remote-apis-srcdir DIR : location of remote-apis source
     If none is provided, then this will checkout the source into a temp dir.
  --googleapis-srcdir DIR : location of googleapis source
     If none is provided, then this will checkout the source into a temp dir.

EOF
  notice
}

function notice() {
  cat <<EOF
This populates the Fuchsia source tree with the following (gitignore'd) files:

  build/rbe/proto/:
    api/log/log.proto
    api/log/log_pb2.py
    api/stats/stats.proto
    api/stats/stats_pb2.py
    go/api/command/command.proto
    go/api/command/command_pb2.py
    and more...
EOF
}

RECLIENT_SRCDIR=
REMOTE_APIS_SRCDIR=
GOOGLEAPIS_SRCDIR=

yes_to_all=0

prev_opt=
# Extract script options before --
for opt
do
  # handle --option arg
  if test -n "$prev_opt"
  then
    eval "$prev_opt"=\$opt
    prev_opt=
    shift
    continue
  fi
  # Extract optarg from --opt=optarg
  case "$opt" in
    *=?*) optarg=$(expr "X$opt" : '[^=]*=\(.*\)') ;;
    *=) optarg= ;;
  esac

  case "$opt" in
    --help | -h ) usage; exit ;;
    --reclient-srcdir=*) RECLIENT_SRCDIR="$optarg" ;;
    --reclient-srcdir) prev_opt=RECLIENT_SRCDIR ;;
    --remote-apis-srcdir=*) REMOTE_APIS_SRCDIR="$optarg" ;;
    --remote-apis-srcdir) prev_opt=REMOTE_APIS_SRCDIR ;;
    --googleapis-srcdir=*) GOOGLEAPIS_SRCDIR="$optarg" ;;
    --googleapis-srcdir) prev_opt=GOOGLEAPIS_SRCDIR ;;
    -y ) yes_to_all=1 ;;
    *) echo "Unknown option: $opt" ; usage ; exit 1 ;;
  esac
  shift
done

readonly DESTDIR="$script_dir"

# Prompt.
test "$yes_to_all" = 1 || {
  notice
  echo
  echo -n "Proceed? [y/n] "
  read proceed
  test "$proceed" = "y" || test "$proceed" = "Y" || {
    echo "Stopping."
    exit
  }
}

# TODO(fangism): choose a deterministic cache dir,
# and pull instead of re-cloning every time.
tmpdir="$(mktemp -d -t rbe_proto_refresh.XXXX)"

# Some of the symlinks used in this flow are relative to $project_root,
# so we forcibly cd there first, and then all commands that follow
# can assume the same root-relative paths.
cd "$project_root"

# The protobuf python wheel should be in prebuilt already.
readonly any_protobuf_wheel_file=google/protobuf/descriptor_pb2.py
[[ -f "$PROTOBUF_WHEEL/$any_protobuf_wheel_file" ]] || {
  echo "Expected a $any_protobuf_wheel_file from prebuilt, but is missing."
  exit 1
}

# If reclient-srcdir is not provided, checkout in a tempdir
test -n "$RECLIENT_SRCDIR" || {
  echo "Fetching re-client source."
  pushd "$tmpdir"
  git clone sso://team/foundry-x/re-client
  popd
  RECLIENT_SRCDIR="$tmpdir"/re-client
}

echo "Installing protos from $RECLIENT_SRCDIR to $DESTDIR"
mkdir -p "$DESTDIR"/api/log
grep -v "bq_table.proto" "$RECLIENT_SRCDIR"/api/log/log.proto | \
  grep -v "option.*gen_bq_schema" > "$DESTDIR"/api/log/log.proto
mkdir -p "$DESTDIR"/api/stat
cp "$RECLIENT_SRCDIR"/api/stat/stat.proto "$DESTDIR"/api/stat/
mkdir -p "$DESTDIR"/api/stats
cp "$RECLIENT_SRCDIR"/api/stats/stats.proto "$DESTDIR"/api/stats/


test -n "$REMOTE_APIS_SRCDIR" || {
  echo "Fetching bazelbuild/remote-apis source."
  pushd "$tmpdir"
  git clone https://github.com/bazelbuild/remote-apis.git
  popd
  REMOTE_APIS_SRCDIR="$tmpdir"/remote-apis
}

echo "Installing protos from $REMOTE_APIS_SRCDIR to $DESTDIR"
readonly re_proto_subdir=build/bazel
mkdir -p "$DESTDIR"/"$re_proto_subdir"
cp -r "$REMOTE_APIS_SRCDIR"/"$re_proto_subdir"/* "$DESTDIR"/"$re_proto_subdir"/


test -n "$GOOGLEAPIS_SRCDIR" || {
  echo "Fetching googleapis/googleapis source."
  pushd "$tmpdir"
  git clone https://github.com/googleapis/googleapis.git
  popd
  GOOGLEAPIS_SRCDIR="$tmpdir"/googleapis
}

echo "Installing protos from $GOOGLEAPIS_SRCDIR to $DESTDIR"
mkdir -p "$DESTDIR"/google
cp -r "$GOOGLEAPIS_SRCDIR"/google/{api,longrunning,rpc} "$DESTDIR"/google/


echo "Fetching proto from http://github.com/bazelbuild/remote-apis-sdks"
mkdir -p "$DESTDIR"/go/api/command
curl https://raw.githubusercontent.com/bazelbuild/remote-apis-sdks/master/go/api/command/command.proto > "$DESTDIR"/go/api/command/command.proto

cd "$project_root"
# Disable build metrics to avoid upload_reproxy_logs.sh before it is usable.
protoc=(env FX_REMOTE_BUILD_METRICS=0 fx host-tool protoc)

echo "Compiling protobufs with protoc: ${protoc[@]}"
# Caveat: if fx build-metrics is already enabled with RBE, this fx build may
# attempt to process and upload metrics before it is ready, and fail.

# Walk the proto imports recursively to find what needs to be compiled.
function walk_proto_imports() {
  echo "$1"
  local this="$1"
  shift
  local list
  list=($(grep "^import" "$this" | cut -d'"' -f2))
  for f in "${list[@]}"
  do
    if test -f "$f"
    then walk_proto_imports "$f"
    else echo "$f"  # assume that $f is in a different proto path
    fi
  done
}

pushd build/rbe/proto  # under $project_root
_proto_compile_list=(
  # top-level protos:
  $(walk_proto_imports rbe_metrics.proto)
  $(walk_proto_imports api/log/log.proto)
)
popd

proto_compile_list=(
  $(echo "${_proto_compile_list[@]}" | tr ' ' '\n' | sort -u)
)

# NOTE: These generated *_pb2.py are NOT checked-in.
# If this list gets too long, we could parallelize the protoc.
echo "Compiling $DESTDIR protos to Python"
for proto in "${proto_compile_list[@]}"
do
  case "$proto" in
    google/protobuf/*)
      # Don't compile.
      # These compiled protos live in $PROTOBUF_WHEEL.
      echo "  $proto (expected in $PROTOBUF_SRC) ..."
      [[ -f "$PROTOBUF_SRC/$proto" ]] || {
        echo "  WARNING: $PROTOBUF_SRC/$proto should exist, but is missing."
      }
      ;;
    *)  # Everything else expected to be built in-place
      echo "  $proto (to $DESTDIR) ..."
      "${protoc[@]}" \
        -I="$DESTDIR" \
        -I="$PROTOBUF_SRC" \
        --python_out="$DESTDIR" \
        "$DESTDIR"/"$proto"
      ;;
  esac
done

echo "Done."

