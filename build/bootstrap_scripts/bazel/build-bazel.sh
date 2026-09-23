#!/usr/bin/env bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A script to rebuild Bazel from sources for Fuchsia, see --help for details.

set -euo pipefail

readonly SCRIPT_NAME="$(basename "${BASH_SOURCE[0]}")"
readonly SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null && pwd )"

# Detect the 'pv' program in the environment. This will be useful to
# print a timer in non-verbose mode during the different build steps.
PV="$(which pv 2>/dev/null || true)"

# Detect whether stderr is a terminal, if this is the case and 'pv' is available
# the it will be used to print a timer + progress updates during the build in
# non-verbose mode.
IS_TERMINAL=
if [[ -t 2 ]]; then
  IS_TERMINAL=true
fi

VERBOSE=0
if [[ -z "$IS_TERMINAL" ]]; then
  # When not in terminal mode, force verbose mode.
  VERBOSE=1
fi

function run_internal() {
  if [[ "$VERBOSE" -ge 1 ]]; then
    echo "CMD: $*"
  fi
  "$@" 2>&1
}

function run_log_internal() {
  if [[ -n "${LOG_FILE}" ]]; then
    run_internal "$@" | tee --append "${LOG_FILE}"
  else
    run_internal "$@"
  fi
}

# Log file path for non-verbose mode.
LOG_FILE=

# Run a command and log its output if necessary.
# If the 'pv' program is installed, use it to print progress.
# $1: Command description for 'pv' timer.
# $2+: The command to run.
function run_step() {
  local description="$1"
  shift

  if [[ "${VERBOSE}" -eq 0 && -n "${IS_TERMINAL}" && -n "${PV}" ]]; then
    run_log_internal "$@" | "${PV}" -tpl -N"  ${description}" -X
  else
    run_log_internal "$@"
  fi
}

# Print error message then exit script with error status.
function die() {
  echo >&2 "ERROR: $*"
  exit 1
}

# Make a file path absolute
# $1: file path
# Output: absolute file path.
function make_absolute() {
  case "$1" in
    /*)
      echo "$1"
      ;;
    *)
      echo "${PWD}/$1"
      ;;
  esac
}

# Generate C++ toolchain wrapper scripts
# $1: wrapper_dir path
# $2: clang_bindir value, or empty string.
# $3: sysroot_dir value, or empty string.
function generate_toolchain_wrappers() {
  local wrapper_dir="$1"
  local clang_bindir="$2"
  local sysroot_dir="$3"
  local clang_prefix=""
  if [[ -n "${clang_bindir}" ]]; then
    clang_prefix="${clang_bindir}/"
  fi
  local sysroot_args=()
  if [[ -n "${sysroot_dir}" ]]; then
    sysroot_dir="$(cd "${sysroot_dir}" && pwd)"
    sysroot_args=(--sysroot="${sysroot_dir}")
  fi
  local tool tool_script
  mkdir -p "${wrapper_dir}"
  rm -rf "${wrapper_dir:?}"/*
  for tool in clang clang++ lld; do
    tool_script="${wrapper_dir}/${tool}"
    cat > "${tool_script}" <<EOF
#!/bin/bash
exec "${clang_prefix}${tool}" "${sysroot_args[@]}" "\$@"
EOF
    chmod a+x "${tool_script}"
  done
}

# Extract the Bazel version from a source or distribution directory.
# $1: Bazel source or distribution directory.
function extract_bazel_version_from_dir() {
  # Parse the top-level MODULE.bazel file, which must starts with something like:
  #
  # module(
  #   name = "bazel",
  #   version = "8.8.0",
  #   repo_name = "io_bazel",
  # )
  local module_path="$1/MODULE.bazel"
  [[ -f "${module_path}" ]] || die "Missing input file: ${module_path}"
  awk '$1 == "version" && $2 == "=" { print $3 }' "${module_path}" | head -n1 | tr -d '",'
}

# Parse command-line

function help_requirements() {
  cat <<EOF
Building Bazel from sources is a very long process that
requires:

- A directory pointing to the Bazel git sources,
  or to an expanded Bazel distribution archive, or the
  path to a Bazel distribution zip archive.

  Use one of --bazel-src-dir, --bazel-dist-dir and
  --bazel-dist-archive to indicate which one.

- A previous version of Bazel to build the new sources.
  Note that Bazel X.Y.Z often requires Bazel X.(Y-1) or
  Bazel (X-1).* to build properly.

  Use --boot-bazel to indicate its path.

- A local JDK, preferably selected through the JAVA_HOME
  environment variable.

- A C++ compiler to build various host tools embedded
  in the final 'bazel' binary. Using the Fuchsia
  toolchain is highly recommended.

- Network access during the build, as this will download
  over 4 GiB of artifacts from the Internet (with
  content hashes locked to fixed values from the source
  Bazel directory).

EOF
  exit 0
}

function usage() {
  cat <<EOF
Usage: ${SCRIPT_NAME} [options]

Build Bazel binary from source for Fuchsia.

Read --help-requirements for requirements.

This takes several minutes on a very powerful machine
and installs the final binary into a directory with
the following layout:

  \$INSTALL_DIR/
      bazel
      bazel-real
      install_base/
      README.fuchsia
      LICENSE

Valid options:

  --help
    Print this message

  --help-requirements
    Description of the requirements for building Bazel
    from sources with this script.

  --help-steps
    Detailed description of the build steps performed by
    this script. Mostly useful if things go wrong of if
    one needs to hack on this script.

  --install-dir=DIR
    Path to the installation directory that will
    receive the final files.

  --boot-bazel=PROGRAM
    Path to the Bazel binary used to bootstrap this build.
    Defaults to the value of the BAZEL environment variable,
    otherwise to 'bazel' in your PATH.

   --build-dir=BUILD_DIR
    Path to a build directory. If not provided, a temporary
    directory will be used (and cleaned up on exit).

  --bazel-src-dir=DIR
  --bazel-dist-dir=DIR
  --bazel-dist-archive=ARCHIVE
    One of these options must be provided to locate the Bazel
    sources or a pre-existing distribution archive.

    --bazel-src-dir points to a Bazel source repository.
    --bazel-dist-dir points to an expanded Bazel distribution archive.
    --bazel-dist-archive points to a Bazel distribution zip archive
         that will be expanded into a temporary directory.

  --bazel-version=VERSION
    Bazel version (e.g. "8.1.1"). Defaults to auto-detection from
    the content of the source or distribution directory, or the
    name of the distribution archive.

    NOTE: the final binary will use "VERSION-fuchsia" as its
    version label, to distinguish it from official release binaries.

  --clang-bindir=CLANG_BINDIR
    Optional. Use specific Clang installation to build C++ binaries.

  --sysroot-dir=SYSROOT_DIR
    Optional. Use specific sysroot to build C++ binaries.

  --fuchsia-dir=FUCHSIA_DIR
    Use the Fuchsia Clang toolchain and Linux sysroot to build C++ binaries.
    This implies the following flags, unless specified manually:

       --clang-dir=\$FUCHSIA_DIR/prebuilt/third_party/clang/linux-x64/bin
       --sysroot-dir=\$FUCHSIA_DIR/prebuilt/third_party/sysroot/linux
       --boot-bazel=\$FUCHSIA_DIR/prebuilt/third_party/bazel/linux-x64

    FUCHSIA_DIR should point to the root of a Fuchsia source directory.

  --skip-*
    These flags are only used when debugging or changing this script.
    See --help-steps for details.

EOF
  exit 0
}

function help_steps() {
  cat <<EOF
Building Bazel from sources is a very long process which consists in
several steps:

- STEP 1: Distribution archive creation.

  This only happens when --bazel-src-dir=DIR is used. This creates
  a new distribution archive from the git sources, an operation that
  downloads over 4 GiB of prebuilts from the internet, and packs
  the sources and downloaded artifacts into a large zip archive.
  It also compiles a few

  This requires a local JDK and a boot Bazel binary (the latter
  will *not* be used by other steps).

- STEP 2: Distribution archive extraction.

  This does not happen if --bazel-dist-dir=DIR is used. Otherwise
  extract the distribution archive to a temporary directory. This
  only requires the 'unzip' program.

- STEP 3: Producing a stage 1 Bazel binary

  This builds a first 'bazel' binary that doesn't embed any JDK
  from scratch. This only requires a local JDK and this build
  should not download anything from the internet, as it will
  only use artifacts from the distribution archive.

- STEP 4: Producing the stage 2 Bazel binary

  The stage 1 'bazel' program is launched to build the final
  version of the program, which will contain an embedded JDK
  that matches official Bazel releases.

- STEP 5: Installation to Fuchsia-specific directory layout

  The stage 2 binary is copied to \$INSTALL_DIR as 'bazel-real'
  while a wrapper 'bazel' script, a LICENSE file, and a
  README.fuchsia files are added to. In addition, and
  'install_base' directory is created contained the
  expanded content of 'bazel-real', to reduce startup
  overhead and disk usage during the build.

Recommendations

  Use the --fuchsia-dir=DIR option to use the Fuchsia C++
  toolchain and Linux sysroot, they will produce optimized
  binaries that can run on older Linux images such as
  Ubuntu 18.04, as required for Fuchsia development.

Development tips:

  Use the --build-dir=DIR option if you need to work on this
  script (e.g. debugging build problems with newer Bazel
  sources).

  When using --build-dir, the following extra flags can be
  used during incremental builds to skip steps already
  performed in the previous call:

  --skip-dist
     Skip step 1 (creation of the distribution archive), and reuse
     the previous one. Ignored if --bazel-src-dir is not used.

  --skip-extract
     Skip step 2 (extraction of the distribution archive), and
     reuse the previous extracted directory. Ignored if
     --baazel-dist-dir is used.

  --skip-stage1
     Skip step 3 (building the stage 1 binary) and reuse
     the previous version.

  --skip-stage2
    Skip step 4 (building the stage 2 binary) and reuse the
    previous version for final installation.

EOF
  exit 0
}

BOOT_BAZEL=
BAZEL_SRC_DIR=
BAZEL_DIST_ARCHIVE=
BAZEL_DIST_DIR=
BAZEL_VERSION=
BUILD_DIR=
CLANG_BINPREFIX=
INSTALL_DIR=
LOG_FILE=
CLANG_BINDIR=
SYSROOT_DIR=
SKIP_DIST=
SKIP_EXTRACT=
SKIP_STAGE1=
SKIP_STAGE2=
JAVA_HOME="${JAVA_HOME:-}"

for OPT; do
  OPTARG="${OPT##--*=}"
  case "${OPT}" in
    --boot-bazel=*)
      BOOT_BAZEL="${OPTARG}"
      ;;
    --bazel-src-dir=*)
      BAZEL_SRC_DIR="${OPTARG}"
      ;;
    --bazel-dist-archive=*)
      BAZEL_DIST_ARCHIVE="${OPTARG}"
      ;;
    --bazel-dist-dir=*)
      BAZEL_DIST_DIR="${OPTARG}"
      ;;
    --bazel-version=*)
      BAZEL_VERSION="${OPTARG}"
      ;;
    --build-dir=*)
      BUILD_DIR="${OPTARG}"
      ;;
    --log-file=*)
      LOG_FILE="${OPTARG}"
      ;;
    --sysroot-dir=*)
      SYSROOT_DIR="${OPTARG}"
      ;;
    --clang-bindir=*)
      CLANG_BINDIR="${OPTARG}"
      ;;
    --fuchsia-dir=*)
      CLANG_BINDIR="${OPTARG}/prebuilt/third_party/clang/linux-x64/bin"
      SYSROOT_DIR="${OPTARG}/prebuilt/third_party/sysroot/linux"
      if [[ -z "${BOOT_BAZEL}" ]]; then
        BOOT_BAZEL="${OPTARG}/prebuilt/third_party/bazel/linux-x64/bazel"
      fi
      ;;
    --help)
      usage
      ;;
    --help-steps)
      help_steps
      ;;
    --install-dir=*)
      INSTALL_DIR="${OPTARG}"
      ;;
    --skip-dist)
      SKIP_DIST=true
      ;;
    --skip-extract)
      SKIP_DIST=true
      SKIP_EXTRACT=true
      ;;
    --skip-stage1)
      SKIP_DIST=true
      SKIP_EXTRACT=true
      SKIP_STAGE1=true
      ;;
    -v|--verbose)
      VERBOSE=$(( VERBOSE + 1 ))
      ;;
    -vv*)
      VERBOSE=$(( VERBOSE + 2 ))
      ;;
    -*)
     die "Invalid option, see --help."
     ;;
    *)
     die "This script doesn't take arguments, see --help."
  esac
done

[[ -n "${INSTALL_DIR}" ]] || die "Missing --install-dir=DIR argument, see --help."

if [[ -n "${BAZEL_SRC_DIR}" ]]; then
  [[ -z "${BAZEL_DIST_DIR}" && -z "${BAZEL_DIST_ARCHIVE}" ]] || die "Cannot use --bazel-dist-{dir,archive} with --bazel-src-dir."
  [[ -d "${BAZEL_SRC_DIR}" ]] || die "Not a directory: ${BAZEL_SRC_DIR}"
elif [[ -n "${BAZEL_DIST_DIR}" ]]; then
  [[ -d "${BAZEL_DIST_DIR}" ]] || die "Not a directory: ${BAZEL_DIST_DIR}"
  [[ -z "${BAZEL_DIST_ARCHIVE}" ]] || die "Do not set both --bazel-dist-dir and --bazel-dist-archive"
else
  [[ -n "${BAZEL_DIST_ARCHIVE}" ]] || die "One of --bazel-src-dir, --bazel-dir-dir or --bazel-dist-archive is required"
  [[ -f "${BAZEL_DIST_ARCHIVE}" ]] || die "Missing Bazel dist archive: ${BAZEL_DIST_ARCHIVE}"
fi

if [[ -n "${SYSROOT_DIR}" ]]; then
  [[ -d "${SYSROOT_DIR}" ]] || die "Not a directory: ${SYSROOT_DIR}"
  [[ -d "${SYSROOT_DIR}/usr/include" ]] || die "Not a sysroot directory: ${SYSROOT_DIR}"
fi

if [[ -n "${CLANG_BINDIR}" ]]; then
  [[ -f "${CLANG_BINDIR}/clang" ]] || die "Missing Clang binary: ${CLANG_BINDIR}/clang"
fi

TEMP_BUILD_DIR=

function cleanup_temp_build_dir {
  if [[ -n "$TEMP_BUILD_DIR" ]]; then
    # Print a newline first, since this can be called in case of Ctrl-C
    printf "\nCleaning up build directory %s\n" "${TEMP_BUILD_DIR}" >&2
    # Bazel generates read-only files in its output_base directory
    # which prevent `rm -rf` from working. Deal with that now.
    if [[ -d "${TEMP_BUILD_DIR}/output_base" ]]; then
      chmod -R +w  "${TEMP_BUILD_DIR}/output_base"
      rm -rf "${TEMP_BUILD_DIR}"
    fi
    TEMP_BUILD_DIR=
  fi
}

declare -i START_TIME=${SECONDS}

if [[ -z "${BUILD_DIR}" ]]; then
  TEMP_BUILD_DIR=/tmp/bazel-build-dir-$$
  mkdir -p "${TEMP_BUILD_DIR}"
  rm -rf "${TEMP_BUILD_DIR:?}"/*
  trap cleanup_temp_build_dir EXIT

  BUILD_DIR="${TEMP_BUILD_DIR}"
else
  mkdir -p "${BUILD_DIR}"
  BUILD_DIR="$(cd "${BUILD_DIR}" && pwd 2>/dev/null)"
fi

if [[ "$VERBOSE" -eq 0 ]]; then
  # Setup a default log file in non-verbose mode.
  LOG_FILE="${LOG_FILE:-"${BUILD_DIR}/build-log.txt"}"
fi

# Generate toolchain wrapper scripts is --clang-dir or --sysroot-dir are used.
# Adding them to the PATH ensures Bazel will use the right compiler and/or sysroot.
if [[ -n "${SYSROOT_DIR}" || -n "${CLANG_BINPREFIX}" ]]; then
  TOOLCHAIN_WRAPPER_DIR="${BUILD_DIR}/toolchain-wrappers"
  generate_toolchain_wrappers "${TOOLCHAIN_WRAPPER_DIR}" "${CLANG_BINDIR}" "${SYSROOT_DIR}"
  export PATH="${TOOLCHAIN_WRAPPER_DIR}:${PATH}"
  export CXX="clang++"
  export CC="clang"
  export LD="lld"
fi

if [[ -n "${JAVA_HOME}" ]]; then
  JAVA="${JAVA_HOME}/bin/java"
else
  JAVA="java"
fi

# Extract major Java version.
# Expected format of first line of `java --version` output:
# <vendor> <major>.<minor>.<patch> <other...>
JAVA_VERSION="$("${JAVA}" --version | head -n1 | awk '{print $2}' | cut -d. -f1)"

if [[ -z "${BAZEL_VERSION}" ]]; then
  if [[ -n "${BAZEL_SRC_DIR}" ]]; then
    BAZEL_VERSION="$(extract_bazel_version_from_dir "${BAZEL_SRC_DIR}")"
  elif [[ -n "${BAZEL_DIST_DIR}" ]]; then
    BAZEL_VERSION="$(extract_bazel_version_from_dir "${BAZEL_DIST_DIR}")"
  else
    BAZEL_DIST_NAME=$(basename "${BAZEL_DIST_ARCHIVE}")
    case "${BAZEL_DIST_NAME}" in
      bazel-*-dist.zip)
        BAZEL_VERSION="${BAZEL_DIST_NAME#bazel-}"
        BAZEL_VERSION="${BAZEL_VERSION%-dist.zip}"
        ;;
      *)
        die "Could not extract Bazel version from ${BAZEL_DIST_NAME}, use --bazel-version!"
        ;;
    esac
  fi
fi

BOOT_BAZEL_ARGS=(
  --output_base="${BUILD_DIR}"/output_base
  --output_user_root="${BUILD_DIR}"/output_user_root

  # Ensure Bazel doesn't create read-only artifacts in the output base
  # --experimental_writable_outputs
)

[[ -n "${BOOT_BAZEL}" ]] || {
  BOOT_BAZEL="${BAZEL:-bazel}"
}

BOOT_BAZEL_VERSION="$("${BOOT_BAZEL}" version 2>&1 | awk '$1 == "Build" && $2 == "label:" { print $3 }' || true)"

cat <<EOF
Preparing to build Bazel from sources:

BAZEL_VERSION:       ${BAZEL_VERSION}
BAZEL_SRC_DIR:       ${BAZEL_SRC_DIR}
BAZEL_DIST_ARCHIVE:  ${BAZEL_DIST_ARCHIVE}
BAZEL_DIST_DIR:      ${BAZEL_DIST_DIR}
INSTALL_DIR:         ${INSTALL_DIR}
BUILD_DIR:           ${BUILD_DIR}
JAVA_HOME:           ${JAVA_HOME}
JAVA_VERSION:        ${JAVA_VERSION}
BOOT_BAZEL           ${BOOT_BAZEL}
BOOT_BAZEL_VERSION   ${BOOT_BAZEL_VERSION}
BOOT_BAZEL_ARGS      ${BOOT_BAZEL_ARGS[*]}
CLANG_BINDIR:        ${CLANG_BINDIR}
SYSROOT_DIR:         ${SYSROOT_DIR}

EOF

if [[ -n "$LOG_FILE" ]]; then
  echo "Follow build with: tail -f $LOG_FILE"
fi
if [[ -n "$IS_TERMINAL" &&  -z "${PV}" ]]; then
  echo "ProTip: Install 'pv' program for terminal progress output during the build."
fi

# Ensure the build stores all artifacts in the build directory
# instead of trying to put them under $HOME/.cache/bazel/
_BAZEL_OUTPUT_USER_ROOT="${BUILD_DIR}/bazel_output_user_root"

if [[ -n "${BAZEL_SRC_DIR}" ]]; then
  DIST_ARCHIVE="${BUILD_DIR}/bazel-${BAZEL_VERSION}.zip"
  if [[ -n "$SKIP_DIST" ]]; then
    [[ -f "${DIST_ARCHIVE}" ]] || die "--skip-dist used but archive missing: ${DIST_ARCHIVE}"
    echo "Skipping dist archive creation."
  else
    echo "Creating distribution archive from source directory."
    (
      cd "${BAZEL_SRC_DIR}"
      run_step "Creating distribution archive" "${BOOT_BAZEL}" "${BOOT_BAZEL_ARGS[@]}" build  --compiler=clang --compilation_mode=opt --strip=always //:bazel-distfile

      cp -f bazel-bin/bazel-distfile.zip "${DIST_ARCHIVE}"
      # Bazel creates a read-only archive by default. Make it writable
      # after the copy.
      chmod ug+w "${DIST_ARCHIVE}"
    )
  fi
  BAZEL_DIST_ARCHIVE="${DIST_ARCHIVE}"
  BAZEL_SRC_DIR=
fi

if [[ -n "${BAZEL_DIST_ARCHIVE}" ]]; then
  DIST_DIR="${BUILD_DIR}/bazel-extracted-dist"
  if [[ -n "$SKIP_EXTRACT" ]]; then
    [[ -d "${DIST_DIR}" ]] || die "--skip-extract used, but dist dir missing: ${DIST_DIR}"
    echo "Skipping archive extraction"
  else
    rm -rf "${DIST_DIR:?}"/* && mkdir -p "${BUILD_DIR}"
    echo "Extracting archive to $DIST_DIR"
    run_step "Extracting distribution archive" unzip -q -o -d "${DIST_DIR}" "${BAZEL_DIST_ARCHIVE}"
  fi
else
  DIST_DIR="${BAZEL_DIST_DIR}"
fi

BAZEL_JAVAC_OPTS=()

BAZEL_JAVA_ARGS=(
  # Use the local JDK (pointed to by JAVA_HOME) to build Bazel Java artifacts.
  --tool_java_runtime_version=local_jdk

  # Use the local JDK to run Bazel
  --java_runtime_version=local_jdk
)

BAZEL_CC_ARGS=(
  # Optimize and strip embedded C++ binaries
  --compilation_mode=opt
  --compiler=clang
  --strip=always
)

if [[ "$JAVA_VERSION" -ge 23 ]]; then
  # Bazel JDK 23 changed the default for annotations processing from "enabled"
  # to "disabled" which breaks the Bazel build. The following is required to
  # pass an extra options to all javac invocations to solve this issue.
  BAZEL_JAVA_ARGS+=(--javacopt=-proc:full --host_javacopt=-proc:full)

  # The following variable is used by the direct javac invocations performed
  # by the "Building Bazel from scratch..." step.
  BAZEL_JAVAC_OPTS=(-proc:full)
fi

EMBED_LABEL="${BAZEL_VERSION}-fuchsia"

# Generate a first version minimalistic of Bazel from the sources using
# the boot Bazel program.
BAZEL_STAGE1="${BUILD_DIR}/bazel-${BAZEL_VERSION}-stage1"
if [[ -n "${SKIP_STAGE1}" ]]; then
  [[ -f "${BAZEL_STAGE1}" ]] || die "--skip-stage1 used but binary missing: ${BAZEL_STAGE1}"
  echo "Skipping building stage 1 Bazel program."
else
  echo "Building stage 1 Bazel program."
  (
    cd "${DIST_DIR}"
    run_step "Building stage 1" env BAZEL_JAVAC_OPTS="${BAZEL_JAVAC_OPTS[*]}" EXTRA_BAZEL_ARGS="${BAZEL_JAVA_ARGS[*]} ${BAZEL_CC_ARGS[*]}" EMBED_LABEL="${EMBED_LABEL}" bash ./compile.sh
    cp -rpf output/bazel "${BAZEL_STAGE1}"
  )
fi

STAGE1_BAZEL_ARGS=(
  # Reuse the same output base to save time.
  --output_base="${BUILD_DIR}"/output_base
  --output_user_root="${BUILD_DIR}"/output_user_root
)

# Now use the stage1 binary to rebuild the full Bazel binary which
# includes the embedded JDK. Trying to do that directly with the
# boot Bazel program can often fail. Note that this will download
# prebuilt Azul JDK images from the internet, as these are not
# included in the distribution image due to their large sizes.
# See $BAZEL_SRC/repositories.bzl for details.
BAZEL_STAGE2="${BUILD_DIR}/bazel-${BAZEL_VERSION}-fuchsia"
if [[ "$SKIP_STAGE2" ]]; then
  [[ -f "${BAZEL_STAGE2}" ]] || die "--skip-stage2 used, but binary missing: ${BAZEL_STAGE2}"
  echo "Skipping building stage 2 Bazel program."
else
  echo "Building stage 2 Bazel program."
  (
    cd "${DIST_DIR}"
    run_step "Building stage 2" "${BAZEL_STAGE1}" "${STAGE1_BAZEL_ARGS[@]}" build  "${BAZEL_CC_ARGS[@]}" --stamp --embed_label "${EMBED_LABEL}" //src:bazel_jdk_minimal
      cp -rpf bazel-bin/src/bazel_jdk_minimal "${BAZEL_STAGE2}"
  )
fi

echo "Installing new binary to: ${INSTALL_DIR}"

# The Bazel binary is also a self-extracting ZIP archive. To speed
# up invocations, unpack it directly into an 'install_base' directory
# and generate a wrapper script that ensures the real Bazel binary
# is invoked with --install_base to point to it.
INSTALL_BASE="${INSTALL_DIR}/install_base"
mkdir -p "${INSTALL_BASE}"
unzip -q -o -d "${INSTALL_BASE}" "${BAZEL_STAGE2}"

# Bazel unpacks all files with mode 750, so enforce that here too.
chmod -R 0750 "${INSTALL_BASE}"

cp -f "${BAZEL_STAGE2}" "${INSTALL_DIR}/bazel-real"

# Bazel requires all timestamps in the install_base to be 10 years in
# the future (this is a cheap way to check for integrity). However timestamps
# are not preserved in the CIPD archive uploaded by the recipe (or maybe
# by CIPD during extraction?), so do not try to adjust them here, this
# will be handled by the wrapper script.

# Copy the wrapper script, which expects the real Bazel binary to be
# named `bazel-real` (this matches Linux Debian), and the install_base
# directory to be named `install_base`.
cp -f "${SCRIPT_DIR}"/bazel_wrapper_script.bash "${INSTALL_DIR}/bazel"
chmod a+x "${INSTALL_DIR}/bazel-real" "${INSTALL_DIR}/bazel"

cp -f "${DIST_DIR}/LICENSE" "${INSTALL_DIR}/LICENSE"

# Generate README.fuchsia while injecting the right version.
sed -e 's|@BAZEL_VERSION@|'"${BAZEL_VERSION}"'|g' \
    "${SCRIPT_DIR}"/README.fuchsia.template > "${INSTALL_DIR}/README.fuchsia"

cleanup_temp_build_dir

declare -i DURATION=$(( SECONDS - START_TIME ))
declare -i MINUTES=$(( DURATION / 60 ))
declare -i SECS=$(( DURATION % 60 ))
echo "Done. Build took ${MINUTES}m${SECS}s."
