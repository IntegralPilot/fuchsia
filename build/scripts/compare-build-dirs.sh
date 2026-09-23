#!/bin/bash
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

function usage() {
  cat <<EOF
usage: $0 [options] DIR1 DIR2 [RELPATH]

Compares two build directories for artifact differences.
Comparison logic tries to account for the file type in
choosing a diff strategy.

If RELPATH is provided, then compare DIR1/RELPATH vs. DIR2/RELPATH.
The joined path may reference a directory or file.

Run this from the Fuchsia source checkout root dir ($FUCHSIA_DIR),
because it references some tools from the source tree.

Example: Compare two clean builds with same output dir:
  fx set ...
  fx clean-build
  cp -p -r out/default out/default.bkp
  fx clean-build
  cp -p -r out/default out/default.bkp2
  $0 out/default.bkp out/default.bkp2

Valid options:
  --help     Print this message.
  --verbose  Increase verbosity.
EOF
}

readonly script="$0"
# assume script is always with path prefix, e.g. "./$script"
readonly script_dir="${script%/*}"  # dirname
# readonly script_basename="${script##*/}"  # basename

# shellcheck source=/dev/null
source "$script_dir"/../../build/rbe/common-setup.sh

# shellcheck disable=SC2154  # default_project_root comes from common-setup.sh
project_root="$default_project_root"

readonly detail_diff="$project_root"/build/rbe/detail-diff.sh
readonly detail_diff_opts=(
  "-l=first"
  "-r=second"
)

DEBUG_LEVEL=1

# Print debug message if DEBUG is not empty.
function debug() {
  [[ "${DEBUG_LEVEL}" -ge 1 ]] && echo >&2 "DEBUG: $*"
}

function debug2() {
  [[ "${DEBUG_LEVEL}" -ge 2 ]] && echo >&2 "DEBUG2: $*"
}

# json formatting (as pipe): jq . < stdin > stdout
readonly jq="$project_root"/prebuilt/third_party/jq/"$HOST_PLATFORM"/bin/jq

function json_format() {
  debug2 "json_format $1"
  "$jq" . < "$1" > "$1".formatted
}

# GLOBAL MUTABLES
# Accumulate paths to unexpected differences here.
# bash: array variables are not POSIX
unexpected_diffs=()
unexpected_matches=()
# These files match or differ, but we didn't know what to expect.
unclassified_diffs=()
unclassified_matches=()
# These files were skipped.
# It might be useful to compare these after ALL
# other differences have been eliminated.
skipped_files=()
# Ignored files are unimportant will never be compared.
ignored_files=()
unspecified_files=()

function diff_json() {
  debug "diff_json $1 $2"
  json_format "$1" || { echo "Failed to format $1" ; return 1;}
  json_format "$2" || { echo "Failed to format $2" ; return 1;}
  diff -u "$1".formatted "$2".formatted
  # return with the exit code of diff
}

function diff_text() {
  debug "diff_text $1 $2"
  diff -u "$1" "$2"
}

function diff_binary() {
  # cmp is faster, as it terminates at first byte difference.
  debug "diff_binary $1 $2"
  cmp "$1" "$2" || "$detail_diff" "${detail_diff_opts[@]}" "$1" "$2"
}

function diff_zip() {
  # cmp is faster, as it terminates at first byte difference.
  debug "diff_zip $1 $2"
  cmp "$1" "$2" || "$detail_diff" "${detail_diff_opts[@]}" "$1" "$2"
}

function exe_unstripped_expect() {
  # Print "diff/match/unknown" depending on whether this file is known to be
  # reproducible across identical builds.
  local path="$1"
  # e.g. exe.unstripped/tool_name
  base="$(basename "$path")"
  case "$base" in
    archivist* | \
    base_resolver | \
    cobalt | \
    cobalt_app_unittests | \
    codec_runner_sw_aac | \
    component_manager | \
    component_manager_boot_env | \
    component_manager_test)
    # TODO(fangism): add many more cases of known diffs
          echo "diff" ;;
    *) echo "match" ;;
  esac
}

function diff_file_relpath() {

  # $1 is left dir
  # $2 is right dir
  # $3 is a relative path under both dirs, and is itself not a directory.
  #   This file name is reported in diagnostics.
  # one could also use an all-inclusive diff tool like https://diffoscope.org/
  local -r left="$1/$3"
  local -r right="$2/$3"
  local -r common_path="$3"
  local -r filebase="${common_path##*/}"  # basename
  local -r subdir="${common_path%/*}" # dirname

  # TODO(fangism): Some files are stored as blobs so content differences
  # appear as filename entry differences.  Skip these.  Perhaps silently?
  # Testing -r also allows for symlinks.
  test -r "$left" || {
    printf "%s does not exist\n" "$left"
    return
  }
  test -r "$right" || {
    printf "%s does not exist\n" "$right"
    return
  }

  local expect=""

  # Classify each category of files with expectations in each case below:
  #   expect={diff,match,unknown,ignore,skip}; diff...
  # Leave blank for ignored files (not compared).
  # "unknown" means unclassified and could contain a mix of matches/diffs.
  # Goal:
  #   * Identify and classify known differences (eliminate unknowns).
  #   * Gradually reduce sources of differences.
  #
  # TODO(fangism): different expectations in clean-vs-clean /
  # clean-vs-incremental modes.
  case "$filebase" in
    # The exit status of this case statement will be used to determine
    # whether or not the given file is an erroneous diff.
    #
    # Generally:
    #   diff_text for text files that are expected to match
    #   diff_binary for binaries or known large textual differences

    # depfiles
    *.d) expect="match"; diff_text "$left" "$right" ;;

    # C++ object files (binary)
    # Nondeterminism due to __TIME__-stamping has been eliminated
    # and is continuously verified by another builder.
    *.o) expect="match"; diff_binary "$left" "$right" ;;
    *.so) expect="match"; diff_binary "$left" "$right" ;;
    *.dylib) expect="match"; diff_binary "$left" "$right" ;;

    *.a) expect="match"; diff_binary "$left" "$right" ;;

    # Rust libraries (binary)
    # There may be unexpected rmeta mismatches.
    # See http//https://fxbug.dev/42079524, https://github.com/rust-lang/rust/issues/113584
    *.rlib) expect="match"; diff_binary "$left" "$right" ;;
    *.rmeta) expect="match"; diff_binary "$left" "$right" ;;

    # Generated code
    *.rs)
      case "$common_path" in
        gen/src/*/qmi-protocol.rs)
          expect="diff" ;;  # ordering diff
        *) expect="match" ;;
      esac
      diff_text "$left" "$right"
      ;;

    memory_metrics_registry.cb.h)
      expect="diff"; diff_text "$left" "$right" ;;  # ordering diff

    # The following groups of files have known huge diffs,
    # so omit details from the general report, and diff_binary.
    meta.far) expect="match"; diff_binary "$left" "$right" ;;
    contents) expect=unknown; diff_binary "$left" "$right" ;;
    all_blobs.json) expect=skip ;;  # too big right now

    blobs.json) expect="diff"; diff_binary "$left" "$right" ;;
      # Many of blobs.json do actually match

    blob.manifest) expect="diff"; diff_binary "$left" "$right" ;;  # many hashes
    blobs.manifest) expect=unknown; diff_binary "$left" "$right" ;;
    package_manifest.json) expect="match"; diff_binary "$left" "$right" ;;
    targets.json)
      case "$common_path" in
        gen/gopaths/*) expect="match"; diff_json "$left" "$right"  ;;
        amber-files/repository/targets.json) expect=skip ;; # too big right now
        */recovery_repository/targets.json) expect=skip ;;
        */repository/targets.json) expect=skip ;;
        */keys/targets.json) expect="match"; diff_json "$left" "$right"  ;;
        *) expect="diff" ;;  # diffs: many hashes
      esac
      ;;
    *.targets.json) expect=skip ;;  # diffs: sig, expires

    outputs.json) expect=skip ;;  # ordering differences from zircon/tools/zither
    snapshot.json) expect=skip ;;  # diffs: sig, expires, version (use diff_json)
    *.root.json) expect=skip ;;  # diffs: sig, expires
    *.snapshot.json) expect=skip ;;  # diffs: sig, expires, version
    timestamp.json) expect="ignore" ;;  # diffs: sig, expires, version
    elf_sizes.json) expect="diff"; diff_json "$left" "$right" ;;  # diffs: build_id
    recovery-eng_blobs.json) expect="diff"; diff_json "$left" "$right" ;;  # diffs: bytes, merkle, size (ordering)
    *.zbi.json) expect=unknown; diff_json "$left" "$right" ;;  # diffs: crc32, size
    *.vbmeta) expect="skip" ;; # Always timestamped in the digital signature
    product_bundle.json) expect="diff"; diff_json "$left" "$right" ;;  # update package hash, due to vbmeta
    update_packages.manifest.json) expect="diff"; diff_json "$left" "$right" ;;  # hashes
    ota_manifest) expect="diff"; diff_binary "$left" "$right" ;;  # vbmeta hash
    root.json)
      case "$common_path" in
        */repository/root.json) expect=skip ;; # diffs: sig, expires
        */recovery_repository/root.json) expect=skip ;; # diffs: sig, expires
        *) expect="match"; diff_json "$left" "$right"  ;;
      esac
      ;;
    # These files are known to have non-deterministic json field ordering due to a Bazel issue
    x64-emu-extra-large.json) expect=skip ;; # field ordering
    x64-emu-min.json) expect=skip ;; # field ordering
    x64-emu-recommended.json) expect=skip ;; # field ordering
    arm64-emu-extra-large.json) expect=skip ;; # field ordering
    arm64-emu-min.json) expect=skip ;; # field ordering
    arm64-emu-recommended.json) expect=skip ;; # field ordering

    images.json) expect=skip ;;  # too many diffs

    transfer.json) expect=skip ;;  # look like list of blob hashes
    *size_report.json | *size_report_product.json | *size_breakdown.txt | *size_budgets.json)
      expect=skip ;; # contains sizes of images which differ already and are expect=unknown.

    compile_commands.json) expect=skip ;;  # too many

    # printed types contain memory addresses
    test.stringarrays.api_summary.json ) expect="ignore" ;;
    test.experimentalzxctypes.api_summary.json) expect="ignore" ;;

    docs.goldens.json)
      # in fuchsia-idk-build-*/gen/zircon/vdso
      # looks like ordering differences
      expect="ignore" ;;

    # Diff formatted JSON for readability.
    *.json)
      case "$common_path" in
        host-tools/goroot/src/cmd/interna/test2json/testdata/*.json)
          expect=unknown ;;
        *) expect="match" ;;
      esac
      diff_json "$left" "$right"
      ;;
    *.json.formatted) expect="ignore" ;;  # This is remant from an earlier diff.

    # recovery things
    recovery-eng_additional_boot_args.txt) expect="diff"; diff_text "$left" "$right" ;;  # hashes
    recovery-eng_pkgsvr_index) expect="diff"; diff_text "$left" "$right" ;;  # hashes

    update_packages.manifest) expect="diff"; diff_text "$left" "$right" ;;  # hashes

    *.image_assembly_inputs) expect=skip ;;  # too many blob/hash differences
    *.image_assembler_all_inputs) expect=skip ;;  # too many blob/hash differences

    manifest.fini) expect=skip ;;  # looks like ordering differences

    # Bazel-related outputs
    java.log)
      case "$common_path" in
        gen/build/bazel/*/java.log) expect="ignore" ;;  # log with timestamps
        *) expect=unknown ;;
      esac
      ;;

    lock)
      case "$common_path" in
        gen/build/bazel/*/lock) expect="ignore" ;;  # contains PID
        *) expect=unknown ;;
      esac
      ;;

    command_port)
      case "$common_path" in
        gen/build/bazel/*/command_port) expect="ignore" ;;  # randomly chosen port
        *) expect=unknown ;;
      esac
      ;;

    request_cookie)
      case "$common_path" in
        gen/build/bazel/*/request_cookie) expect="ignore" ;;  # looks like hash
        *) expect=unknown ;;
      esac
      ;;

    response_cookie)
      case "$common_path" in
        gen/build/bazel/*/response_cookie) expect="ignore" ;;  # looks like hash
        *) expect=unknown ;;
      esac
      ;;

    server_info.rawproto)
      case "$common_path" in
        gen/build/bazel/*/server_info.rawproto) expect="ignore" ;;  # probably PID
        *) expect=unknown ;;
      esac
      ;;

    server.starttime) expect="ignore" ;;  # timestamp
    server.pid.txt) expect="ignore" ;;  # contains PID

    command.log)
      case "$common_path" in
        gen/build/bazel/*/command.log) expect="ignore" ;;  # subject to build parallelism and completion ordering
        *) expect=unknown ;;
      esac
      ;;

    ffx.log)
      case "$common_path" in
        gen/build/bazel/*/cache/logs/ffx.log) expect="ignore" ;;  # log with timestamps
        *) expect=unknown ;;
      esac
      ;;
    uuid)  # uuids are unique and random
      case "$subdir" in
        *.ffx/metrics) expect="ignore" ;;
      esac
      ;;

    volatile-status.txt) expect="ignore" ;;  # bears timestamp

    test.cache_status)
      case "$common_path" in
        bazel-out/*/test.cache_status) expect="diff"; diff_binary "$left" "$right" ;;
        *) expect=unknown ;;
      esac
      ;;

    test.log)
      case "$common_path" in
        bazel-out/*/test.log) expect="ignore" ;;  # reports test time duration
        *) expect=unknown ;;
      esac
      ;;

    test.xml)
      case "$common_path" in
        bazel-out/*/test.xml) expect="ignore" ;;  # reports test time duration
        *) expect=unknown ;;
      esac
      ;;

    action_cache_v15.blaze) expect="ignore" ;;
    filename_index_v15.blaze) expect="ignore" ;;
    command.profile.gz) expect="ignore" ;;

    workspace-events.log*) expect="ignore" ;;

    # numbered bazel action logs (unordered)
    stdout-*) expect="ignore" ;;
    stderr-*) expect="ignore" ;;

    # bazel runfile MANIFESTs contain absolute paths
    MANIFEST)
      case "$subdir" in
        *.runfiles) expect="ignore" ;;
      esac
      ;;

    *.runfiles_manifest) expect="ignore" ;;  # absolute paths

    # bazel repo-mapping files seem to differ:
    # rules_license only appears in remote builds.
    # Don't know why, but these are not important build artifacts.
    _repo_mapping)
      case "$subdir" in
        *.runfiles) expect="ignore" ;;
      esac
      ;;

    *.repo_mapping) expect="ignore" ;;

      # These list hashes of binaries that are already being
      # compared elsewhere, so this is redundant information.
    *.ids_txt) expect="ignore" ;;

    # Various binaries.
    *.blk) expect=unknown; diff_binary "$left" "$right" ;;
    *.vboot) expect=unknown; diff_binary "$left" "$right" ;;
    *.zbi) expect=unknown; diff_binary "$left" "$right" ;;

    # TODO(b/379173865): fidldoc.zip bears timestamps (nondeterministic)
    fidldoc.zip) expect="ignore" ;;

    # rustix build script compilation tests are attributed to absolute paths
    # in metadata, which causes false positives in bazel consistency checks
    rustix_test_can_compile) expect="ignore" ;;

    # python compiled files (nondeterministic)
    *.pyc) expect="ignore" ;;

    *.pyz | *.zip) expect="match"; diff_zip "$left" "$right" ;;
    # Most archives carry timestamp information of their contents.
    # One way to make this reproducible is to force a magic date/time
    # while archiving, which effectively removes time variance.
    *.tar | *.tar.gz | *.tgz) expect="diff"; diff_binary "$left" "$right" ;;

    # Ignore ninja logs, as they bear timestamps,
    # and are non-essential build artifacts.
    .ninja.log) expect="ignore" ;;

    # Ignore filesystem access trace files (fsatrace logs).
    # They may contain nondeterministic paths to /proc/PID
    *_trace.txt) expect="ignore" ;;

    # like exe.unstripped/*.map files
    # .map files are side-effect outputs of linking binaries, and not consumed
    # anywhere else important.
    # Many of these (but not all) reference mktemp paths (Rust toolchain).
    *.map) expect="ignore" ;;

    # Ignore stamp files.
    *.stamp) expect="ignore" ;;

    # Ignore temporary and backup files.
    *.tmp) expect="ignore" ;;
    *.bak) expect="ignore" ;;
    *.bkp) expect="ignore" ;;

    # All others.
    # Binary files diffs will still only be reported tersely.
    # TODO(b/377967111): fix python hermetic test nondeterminism:
    rtc_conformance_test_bin) expect="diff"; diff_binary "$left" "$right" ;;
    *)
      file_type="$(file "$left" | head -n 1 | cut -d: -f2-)"
      case "$file_type" in
        # Binary examples:
        # ELF 64-bit LSB pie executable, x86-64, version 1 (SYSV),... dynamically linked, interpreter ld.so.1, no section header
        # Mach-O universal binary with 2 architectures: [x86_64:Mach-O 64-bit executable x86_64] [arm64e:Mach-O 64-bit executable arm64e]
        *ELF*executable* | *Mach-O*binary*)
          case "$common_path" in
            exe.unstripped/*) expect="$(exe_unstripped_expect "$common_path")" ;;
            *) expect=unknown ;;
          esac
          diff_binary "$left" "$right"
          ;;
        # Assume non-binaries are text.
        *) expect="match"; diff_text "$left" "$right" ;;
      esac
  esac

  # Record unexpected and unclassified differences/matches.
  diff_status="$?"
  case "$expect" in
    match)
      test "$diff_status" = 0 || unexpected_diffs+=("$common_path") ;;
    diff)
      test "$diff_status" != 0 || unexpected_matches+=("$common_path") ;;
    unknown)
      if test "$diff_status" = 0
      then unclassified_matches+=("$common_path")
      else unclassified_diffs+=("$common_path")
      fi
      ;;
    ignore) ignored_files+=("$common_path") ;;
    skip) skipped_files+=("$common_path") ;;
    *) unspecified_files+=("$common_path") ;;
  esac
}

function diff_select() {
  # $1 and $2 are two directories, e.g. "out/default"
  # $3 is the relative path down from $1 and $2, e.g. "subdir/" or "".
  debug2 "$(printf "diff_select()\n   dir1=%s\n   dir2=%s\n  path=%s\n" "$1" "$2" "$3")"
  local relpath="$3"
  local fullpath="$1/$3"
  if test -d "$fullpath"
  then diff_dir_recursive "$1" "$2" "$relpath/"
    # TODO(fangism): what about test -L for symlinks?
  else diff_file_relpath "$1" "$2" "$relpath"
  fi
}

function diff_dir_recursive() {
  # $1 and $2 are two directories, e.g. "out/default"
  # $3 is the relative path down from $1 and $2, e.g. "subdir/" or "".
  # For dual-traversal, arbitrarily use $2's subdirs.
  # echo "Comparing: $sub"
  local sub="$3"  # sub-dir or file

  # Ignore some dirs.
  case "$sub" in
    # Ignore files whose names are content-hash like.
    amber-files/repository/blobs/) return ;;
    amber-files/repository/targets/) return ;;

    # bazel runfiles dirs use absolute symlinks, so ignore those
    *.runfiles) return ;;
    *) ;;  # continue
  esac

  # Silence empty dirs.
  if ! ls "$2/$sub"*
  then return  # empty dir
  fi > /dev/null 2>&1

  for f in "$2/$sub"*
  do
    filebase="${f##*/}"  # basename
    relpath="$sub$filebase"
    diff_select "$1" "$2" "$relpath"
  done
}

ARGS=()
while [[ "$#" -ne 0 ]]; do
  case "$1" in
    --verbose)
      DEBUG_LEVEL=$(( DEBUG_LEVEL + 1 ))
      ;;
    --help)
      usage
      exit 0
      ;;
    -*)
      echo >&2 "ERROR: Unknown option, see --help."
      exit 1
      ;;
    *)
     ARGS+=("$1")
     ;;
  esac
  shift
done

case "${#ARGS[@]}" in
  2)
    diff_dir_recursive "${ARGS[@]}"
    ;;
  3)
    diff_select "${ARGS[@]}"
    ;;
  *)
    usage
    exit 2
esac

# Summarize findings:
echo "======== COMPARISON SUMMARY ========"
exit_status=0

if test "${#unexpected_diffs[@]}" != 0
then
  echo "UNEXPECTED DIFFS: (action: fix source of difference)"
  for path in "${unexpected_diffs[@]}"
  do echo "  $path"
  done
  echo "end of UNEXPECTED DIFFS"
  echo
  exit_status=1
else
  echo "No UNEXPECTED DIFFS found (good)"
fi

# Good news: these files matched
if test "${#unexpected_matches[@]}" != 0
then
  echo "UNEXPECTED MATCHES: (action: make these expect=\"match\" now?)"
  for path in "${unexpected_matches[@]}"
  do echo "  $path"
  done
  echo "end of UNEXPECTED MATCHES"
  echo
  exit_status=1
else
  echo "No UNEXPECTED MATCHES found (good)"
fi

# Make sure all cases are covered.
if test "${#unspecified_files[@]}" != 0
then
  echo "UNSPECIFIED FILES: (action: classify into {match,diff,unknown,ignore,skip})"
  for path in "${unspecified_files[@]}"
  do echo "  $path"
  done
  echo "end of UNSPECIFIED FILES"
  echo
  exit_status=1
fi

[[ "$exit_status" == 0 ]] ||
  echo "ACTION: investigate the above, perhaps file a go/fuchsia-build-bug."
echo "The rest of this report is FYI only, no action is required."

# This group mismatched, but we didn't know what to expect.
if test "${#unclassified_diffs[@]}" != 0
then
  echo "UNCLASSIFIED DIFFS: (action: classify them as expect=\"diff\")"
  for path in "${unclassified_diffs[@]}"
  do echo "  $path"
  done
  echo "end of UNCLASSIFIED DIFFS"
  echo
  # Leave exit status as it were.
fi

# This group matched, but we didn't know what to expect.
if test "${#unclassified_matches[@]}" != 0
then
  echo "UNCLASSIFIED MATCHES: (action: classify them as expect=\"match\")"
  for path in "${unclassified_matches[@]}"
  do echo "  $path"
  done
  echo "end of UNCLASSIFIED MATCHES"
  echo
  # Leave exit status as it were.
fi

if test "${#skipped_files[@]}" != 0
then
  echo "SKIPPED FILES: (action: compare these after all other differences have been resolved)"
  for path in "${skipped_files[@]}"
  do echo "  $path"
  done
  echo "end of SKIPPED FILES"
  echo
  # Leave exit status as it were.
fi

# Don't bother reporting "${ignored_files[@]}", the list is long and not useful.

echo "Exiting with status $exit_status"
exit "$exit_status"
