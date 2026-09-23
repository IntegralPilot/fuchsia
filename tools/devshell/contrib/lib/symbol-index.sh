# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This library is used by:
# * pprof
#
# This file is not self-contained! ../../lib/vars.sh must be sourced before this file.

# This library implements helper functions for locating local symbol files
# referenced by symbol-index.
#
# Since the tools served by this library do not support reading the ids.txt
# format directly, this library transparently converts ids.txt files into
# equivalent build-id directories.
#
# The resulting build-id trees are cached in $_SYMBOL_INDEX_IDS_TXT_CACHE_DIR.
# In detail, for each ids.txt file, this library creates a subdirectory whose
# name is the SHA1 digest of its path, containing:
# - a stamp file, which is compared to the source ids.txt file to detect changes
# - the symbol files, in the usual build-id layout
readonly _SYMBOL_INDEX_IDS_TXT_CACHE_DIR="$FX_CACHE_DIR/symbol-index-ids-txt-cache"

# Returns the SHA1 digest of the given string.
function _fx-symbol-index-string-digest {
  if hash sha1sum 2> /dev/null ; then  # Linux
    echo "$1" | sha1sum | cut -d' ' -f1
  elif hash shasum 2> /dev/null ; then  # OSX
    echo "$1" | shasum -a 1 | cut -d' ' -f1
  else
    fx-error 'Neither "sha1sum" nor "shasum" are available in your PATH'
    exit 1
  fi
}

# Build a build-id directory from a ids.txt file, caching the result.
function _fx-symbol-index-ids-txt-to-build-id {
  local ids_txt_file="$1"
  local cache_key="$(_fx-symbol-index-string-digest "$ids_txt_file")"
  local cache_dir="$_SYMBOL_INDEX_IDS_TXT_CACHE_DIR/$cache_key"
  local stamp_file="$cache_dir/stamp"
  local build_id_dir="$cache_dir/.build-id"

  # Do we really need to rebuild the cache according to the stamp's mtime?
  if [[ "$ids_txt_file" -nt "$stamp_file" ]]; then
    fx-info "Rebuilding build-id directory from $ids_txt_file into $build_id_dir"
    rm -rf "$cache_dir"
    if ! "$FUCHSIA_DIR/scripts/build_id_conv.py" \
      --input "$ids_txt_file" --stamp "$stamp_file" --build-id-mode symlink \
      --output-format .build-id "$build_id_dir"; then
      fx-warn "Failed to convert $ids_txt_file into a build-id directory"
      return 1
    fi
  fi

  # Return the path of the generated build-id directory.
  echo "$build_id_dir"
}

# Lists the build-id directories and ids.txt files referenced by symbol-index.
#
# Their paths are printed one per line, prefixed by their type and a colon:
# - "B:<PATH>" for build-id directories
# - "I:<PATH>" for ids.txt files
function _fx-symbol-index-list-contents-with-type {
  ffx debug symbol-index list --aggregated | \
    fx-command-run jq --raw-output '"B:"+.build_id_dirs[].path,"I:"+.ids_txts[].path'
}

# Lists all the directories containing local symbols, transparently converting
# ids.txt files into build-id directories.
function fx-symbol-index-get-build-id-dirs {
  _fx-symbol-index-list-contents-with-type | while IFS=: read -r type path; do
    case "$type" in
      B)
        # Propagate each build-id directory as-is.
        echo "$path"
        ;;
      I)
        # Convert each ids.txt file and propagate the resulting build-id
        # directory.
        _fx-symbol-index-ids-txt-to-build-id "$path"
        ;;
    esac
  done
}
