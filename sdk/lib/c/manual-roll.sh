#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -u -e -o pipefail

readonly LIBC_DIR="$(dirname "$0")"
cd "$LIBC_DIR/../../.."         # Fuchsia // source root.

readonly FX=scripts/fx
readonly MANIFEST_FILE=manifests/third_party/all

if [ $# -gt 0 ] && [ "$1" = --dry-run ]; then
  shift
  readonly DRY_RUN=true
else
  readonly DRY_RUN=false
fi

if [ $# -gt 0 ] && [ "$1" = --commit ]; then
  shift
  readonly COMMIT=true
else
  readonly COMMIT=false
fi

if [ $# -eq 0 ]; then
  readonly REFSPEC=origin/main
elif [ $# -eq 1 ]; then
  readonly REFSPEC="$1"
else
  echo >&2 "Usage: $0 [REFSPEC]
Applies the same REFSPEC (default: origin/main) to each LLVM-based repo.
To use distinct revisions, update each local checkout and then use HEAD here."
  exit 1
fi

declare -r -A REPOS=(
  [third_party/llvm-libc/src]=llvm-project/libc
  [third_party/scudo/src]=scudo
  [third_party/scudo/gwp_asan]=gwp-asan
)

# Get all the repos current before looking at them.
for dir in "${!REPOS[@]}"; do
  (set -x; git -C "$dir" remote update) &
done
# Update all in parallel, then wait for all to finish.
wait

declare -a SUMMARY=()
declare -A NEW_REV=() OLD_REV=() REMOTE_REV=() REV_DIFF=() LOG=() URL=()
for dir in "${!REPOS[@]}"; do
  project="${REPOS["$dir"]}"
  NEW_REV["$project"]="$(git -C "$dir" rev-parse "$REFSPEC")"
  OLD_REV["$project"]="$("$FX" jiri manifest -element="$project" \
    -template='{{.Revision}}' <(git show JIRI_HEAD:"$MANIFEST_FILE"))"
  REMOTE_REV["$project"]=$("$FX" jiri  project -list-remote-projects -template='{{.Revision}}' "$project")
  if [ "${OLD_REV["$project"]}" != "${REMOTE_REV["$project"]}" ]; then
    echo >&2 "*** $dir local JIRI_HEAD $MANIFEST_FILE doesn't match ToT ${REMOTE_REV["$project"]}"
    if ! $DRY_RUN; then
      echo >&2 "*** punting to --dry-run; do jiri update before starting roll!"
      DRY_RUN=true
    fi
  fi
  if [ "${NEW_REV["$project"]}" != "${OLD_REV["$project"]}" ]; then
    REV_DIFF["$dir"]="$(git -C "$dir" rev-parse --verify --short "${OLD_REV["$project"]}")..$(git -C "$dir" rev-parse --verify --short ${NEW_REV["$project"]})"
    LOG["$dir"]="$(git -C "$dir" log --pretty=oneline \
      --abbrev-commit "${REV_DIFF["$dir"]}")"
    SUMMARY+=("$project" "${REV_DIFF["$dir"]}")
  fi
  URL["$dir"]="$("$FX" jiri manifest -element="$project" \
    -template='{{.Remote}}' "$MANIFEST_FILE")"
done

declare -a JIRI_CMD=("$FX" jiri edit)
for project in "${!NEW_REV[@]}"; do
  JIRI_CMD+=(-project "${project}=${NEW_REV["$project"]}")
done
JIRI_CMD+=("$MANIFEST_FILE")
if $DRY_RUN; then
  echo >&2 "+" "${JIRI_CMD[@]}"
else
  (set -x
   "${JIRI_CMD[@]}"
   git diff origin/main -- "$MANIFEST_FILE"
  )
fi

commitlog() {
  echo "[libc] Roll ${SUMMARY[*]}"
  for dir in "${!LOG[@]}"; do
    echo "
//$dir ${URL["$dir"]}/+log/${REV_DIFF["$dir"]}"
    echo "${LOG["$dir"]}"
  done
  echo '
Cq-Include-Trybots: luci.turquoise.global.try:run-postsubmit-tryjobs'
}

if [ ${#SUMMARY[*]} -eq 0 ]; then
  echo 'Nothing to do!'
  exit
fi

if $COMMIT && ! $DRY_RUN; then
  (LOG="$(commitlog)"
   set -x
   git commit -a -m"$LOG"
   jiri update -local-manifest-project=fuchsia
  )
fi
