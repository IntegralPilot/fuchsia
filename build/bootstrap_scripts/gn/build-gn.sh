#!/usr/bin/env bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Helper script to rebuild GN, supports using Fuchsia prebuilt toolchain + sysroot
# as well as linking against RPMalloc for better performance. See --help.

set -e

die () {
  echo >&2 "ERROR: $@"
  exit 1
}


ALLOCATOR=
ALLOCATOR_LINK_ONLY=
BINPREFIX=
HELP=
TARGET_FLAGS=
SYSROOT_FLAGS=
DEBUG=
GCC=
LTO=
ASAN=
CCACHE=$(which ccache 2>/dev/null || true)
NINJA=ninja
INSTALL_TO=
WINDOWS=
SANITIZE=
EXTRA_CFLAGS=()
EXTRA_LDFLAGS=()

DEFAULT_RPMALLOC_GIT_URL="https://fuchsia.googlesource.com/third_party/github.com/mjansson/rpmalloc"
DEFAULT_RPMALLOC_BRANCH='+upstream/develop'
DEFAULT_RPMALLOC_REVISION='6b34d956911bb778ec6b99e4dbff9e956c5dc467'

RPMALLOC_GIT_URL=
RPMALLOC_BRANCH=
RPMALLOC_REVISION=

JEMALLOC_RELEASE=
DEFAULT_JEMALLOC_GIT_URL=https://github.com/jemalloc/jemalloc.git
DEFAULT_JEMALLOC_TAG=5.3.0

# Set the BINPREFIX variable depending on the value of
# --binprefix and --gcc.
#
# $1: binprefix value, can be a directory, or a file prefix.
# $2: true if GCC must be used.
set_binprefix () {
  local prefix="${1}"
  local is_gcc="${2}"
  if [[ ! -d "${prefix}" ]]; then
    # Not a directory, assume it is a toolchain prefix
    # e.g. `/path/to/toolchain/bin/x64_64-gnu-`.
    BINPREFIX="${prefix}"
  else
    # If a directory, append trailing slash to binprefix.
    BINPREFIX="${prefix%/}/"
  fi
}

for OPT; do
  case "${OPT}" in
    --help)
      HELP=true
      ;;
    --binprefix=*)
      set_binprefix "${OPT#--binprefix=}" "${GCC}"
      ;;
    --sysroot=*)
      SYSROOT_FLAGS="--sysroot=${OPT#--sysroot=}"
      ;;
    --ninja-*)
      NINJA="${OPT#--ninja=}"
      ;;
    --windows-env=*)
      WINDOWS_ENV="${OPT#--windows-env=}"
      ;;
    --gcc)
      if [[ -n "$BINPREFIX" ]]; then
        die "Option --gcc must appear before --binprefix or --fuchsia-dir!"
      fi
      GCC=true
      ;;
    --fuchsia-dir=*)
      FUCHSIA_DIR="${OPT#--fuchsia-dir=}"
      if [[ -n "${GCC}" ]]; then
        set_binprefix "${FUCHSIA_DIR}/prebuilt/third_party/gcc/linux-x64/bin/x86_64-elf-"
      else
        set_binprefix "${FUCHSIA_DIR}/prebuilt/third_party/clang/linux-x64/bin"
      fi
      SYSROOT_FLAGS="--sysroot=${FUCHSIA_DIR}/prebuilt/third_party/sysroot/linux"
      NINJA="${FUCHSIA_DIR}/prebuilt/third_party/ninja/linux-x64/ninja"
      ;;
    --ninja=*)
      NINJA="${OPT#--ninja=}"
      ;;
    --target=*)
      TARGET_FLAGS="${OPT}"
      ;;
    --no-ccache)
      CCACHE=
      ;;
    --sanitize=*)
      SANITIZE="${OPT#--sanitize=}"
      ;;
    --allocator=*)
      ALLOCATOR="${OPT#--allocator=}"
      ;;
    --allocator-link-only)
      ALLOCATOR_LINK_ONLY=true
      ;;
    --rpmalloc-git-url=*)
      RPMALLOC_GIT_URL="${OPT#--rpmalloc-git-url=}"
      ;;
    --rpmalloc-branch=*)
      RPMALLOC_BRANCH="${OPT#--rpmalloc-branch=}"
      ;;
    --rpmalloc-revision=*)
      RPMALLOC_REVISION="${OPT#--rpmalloc-revision=}"
      ;;
    --jemalloc-tag=*)
      JEMALLOC_TAG="${OPT#--jemalloc-tag=}"
      ;;
    --jemalloc-git-url=*)
      JEMALLOC_GIT_URL="${OPT#--jemalloc-git-url=}"
      ;;
    --debug)
      DEBUG=true
      ;;
    --asan)
      SANITIZE=address
      ;;
    --lto)
      LTO=true
      ;;
    --install-to=*)
      INSTALL_TO="${OPT#--install-to=}"
      ;;
    --extra-cflags=*)
      EXTRA_CFLAGS+=("${OPT#--extra-cflags=}")
      ;;
    --extra-ldflags=*)
      EXTRA_LDFLAGS+=("${OPT#--extra-ldflags=}")
      ;;
    -*)
      die "Unknown option $OPT, see --help."
      ;;
    *)
      die "This script does not take parameters [$OPT]. See --help."
      ;;
  esac
done

if [[ -n "$HELP" ]]; then
  PROGNAME="$(basename $0)"
  cat <<EOF
Usage: ${PROGNAME} [options]

Rebuild the GN binary.

This script must be invoked from the GN source directory, but
can be installed anywhere. If you have a Fuchsia workspace, using
the --fuchsia-dir=DIR option is recommended.

Valid options:

  --help                 Print this message.
  --binprefix=DIR        Specify directory with Clang (or GCC) toolchain binaries.
  --gcc                  Use GCC instead of Clang
  --sysroot=DIR          Specify sysroot directory.
  --ninja=BINARY         Specify ninja binary (default is 'ninja').
  --fuchsia-dir=DIR      Specify Fuchsia directory where to find Clang, ninja and sysroot prebuilts.
  --target=ARCH          Specify clang target triple.
  --debug                Build debug version of the binary.
  --lto                  Use LTO and ICF for smaller binary.
  --asan                 Use Address Sanitizer (same as --sanitize=address).
  --sanitize=MODE        Use specific sanitizer mode.
  --no-ccache            Disable ccache usage.
  --extra-cflags         Specify extra compiler flags.
  --extra-ldflags        Specify extra linker flags.
  --install-to=PATH      Copy generated binary to PATH on success.
  --windows-env=FILE     Path to an envsetup.sh script that sets up the environment
                         for a Windows cross-build with clang-cl. This shall
                         set CC, CXX, AR, LINK and LIBS appropriately. This will
                         be sourced directly!

  GN is _significantly_ faster when linked against a specialized allocator
  library, both rpmalloc and jemalloc are supported:

  --allocator=rpmalloc
  --allocator=jemalloc   Select either rpmalloc or jemalloc as the allocator.

  --allocator-link-only  Do not rebuild allocator library from source, use
                         a prebuilt version from a previous script invocation
                         instead to save time during development.

  --rpmalloc-git-url=URL rpmalloc source git URL
                         [$DEFAULT_RPMALLOC_GIT_URL]

  --rpmalloc-revision=REVISION
                         rpmalloc revision [$DEFAULT_RPMALLOC_REVISION]
  --rpmalloc-branch=NAME
                         rpmalloc branch name [$DEFAULT_RPMALLOC_BRANCH]

  --jemalloc-git-url=URL
                         jemalloc source git URL
                         [$DEFAULT_JEMALLOC_GIT_URL]
  --jemalloc-tag=TAG
                         jemalloc git tag [$DEFAULT_JEMALLOC_TAG]

EOF
  exit 0
fi

if [[ -n "${DEBUG}" && -n "${LTO}" ]]; then
  die "Only use one of --debug or --lto!"
fi

GN_BUILD_GEN_ARGS="--no-strip"
if [[ -n "$DEBUG" ]]; then
  echo "Generating debug version of the binary"
  GN_BUILD_GEN_ARGS="${GN_BUILD_GEN_ARGS} --debug"
elif [[ -n "$LTO" ]]; then
  echo "Using link-time optimization and identical-code-folding"
  GN_BUILD_GEN_ARGS="${GN_BUILD_GEN_ARGS} --use-icf --use-lto"
fi

if [[ -n "${WINDOWS_ENV}" ]]; then
  if [[ ! -f "${WINDOWS_ENV}" ]]; then
    die "Missing file: ${WINDOWS_ENV}"
  fi
  source "${WINDOWS_ENV}"
  GN_BUILD_GEN_ARGS="${GN_BUILD_GEN_ARGS} --platform msvc --no-last-commit-position"
elif [[ -n "${GCC}" ]]; then
  CC="${BINPRPEFIX}gcc"
  CXX="${BINPREFIX}g++"
  AR="${BINPREFIX}ar"
else
  CC="${BINPREFIX}clang"
  CXX="${BINPREFIX}clang++"
  AR="${BINPREFIX}llvm-ar"
fi
if [[ ! -f "${AR}" ]]; then
  unset AR
fi
CFLAGS="${TARGET_FLAGS} ${SYSROOT_FLAGS} ${EXTRA_CFLAGS[@]}"
LDFLAGS="${TARGET_FLAGS} ${SYSROOT_FLAGS} -static-libstdc++ ${EXTRA_LDFLAGS[@]}"

if [[ -n "$SANITIZE" ]]; then
  CFLAGS="${CFLAGS} -fsanitize=$SANITIZE"
  LDFLAGS="${LDFLAGS} -fsanitize=$SANITIZE"
fi

if [[ -n "${CCACHE}" ]]; then
  echo "Using ccache program: ${CCACHE}"
  CC="${CCACHE} $CC"
  CXX="${CCACHE} $CXX"
fi

export CC CXX AR CFLAGS CXXFLAGS LDFLAGS

if [[ -z "${XDG_CACHE_HOME}" ]]; then
  XDG_CACHE_HOME="${HOME}/.cache"
fi

# Download and rebuild the rpmalloc library from source, according
# to global options, then copy the result to specified location.
#
# $1: output file path for rpmalloc library
download_and_build_rpmalloc () {
  echo "Rebuilding rpmalloc library from scratch"

  if [[ -z "${RPMALLOC_GIT_URL}" ]]; then
    RPMALLOC_GIT_URL="${DEFAULT_RPMALLOC_GIT_URL}"
  fi
  if [[ -z "${RPMALLOC_BRANCH}" ]]; then
    RPMALLOC_BRANCH="${DEFAULT_RPMALLOC_BRANCH}"
  elif [[ -z "${RPMALLOC_REVISION}" ]]; then
    # If --rpmalloc-branch is used, use FETCH_HEAD as the default revision
    DEFAULT_RPMALLOC_REVISION="FETCH_HEAD"
  fi
  if [[ -z "${RPMALLOC_REVISION}" ]]; then
    RPMALLOC_REVISION="${DEFAULT_RPMALLOC_REVISION}"
  fi

  RPMALLOC_ARCH=x86-64
  RPMALLOC_OS=linux

  RPMALLOC_CONFIG=release
  if [[ -n "${DEBUG}" ]]; then
    RPMALLOC_CONFIG=debug
  fi

  if [[ "$RPMALLOC_SO" == "macos" ]]; then
    RPMALLOC_LIBPATH=lib/$RPMALLOC_OS/$RPMALLOC_CONFIG/librpmalloc.a
  else
    RPMALLOC_LIBPATH=lib/$RPMALLOC_OS/$RPMALLOC_CONFIG/$RPMALLOC_ARCH/librpmalloc.a
  fi

  TMPDIR=$(mktemp -d /tmp/build-rpmalloc.XXXXX)
  (
    cd $TMPDIR
    git init
    git fetch --tags --quiet "$RPMALLOC_GIT_URL" "$RPMALLOC_BRANCH"
    git checkout "$RPMALLOC_REVISION"

    # Patch configure.py to replace -Werror with -Wno-error
    sed -i -e "s|-Werror|-Wno-error|g" build/ninja/clang.py
    RPMALLOC_LOG=${TMPDIR:-/tmp}/rpmalloc-build-$$.log

    export CC CXX AR CFLAGS="$CFLAGS -fPIE" LDFLAGS
    RPMALLOC_CONFIG_FLAGS="-c $RPMALLOC_CONFIG -a $RPMALLOC_ARCH"
    if [[ -n "$LTO" ]]; then
      RPMALLOC_CONFIG_FLAGS="$RPMALLOC_CONFIG_FLAGS --lto"
    fi
    python3 ./configure.py $RPMALLOC_CONFIG_FLAGS
    "${NINJA}" "$RPMALLOC_LIBPATH" >$RPMALLOC_LOG 2>&1 ||
    (echo "ERROR: When build rpmalloc:"; cat "$RPMALLOC_LOG"; exit 1)
  )

  local OUT="$1"
  mkdir -p "$(dirname "${OUT}")"
  cp -f "${TMPDIR}/${RPMALLOC_LIBPATH}" "${OUT}"
}

# Download and rebuild the jemalloc library from source, according
# to global options, then copy the result to specified location.
#
# $1: output file path for rpmalloc library
download_and_build_jemalloc () {
  local OUT="$1"
  mkdir -p "$(dirname "${OUT}")"
  echo "Rebuilding jemalloc from scratch"
  JEMALLOC_SRC=$(mktemp -d /tmp/build-jemalloc.XXXXXX)
  JEMALLOC_LOG=${TMPDIR:-/tmp}/jemalloc-build-$$.log
  echo "Log file at: $JEMALLOC_LOG"
  if [[ -z "$JEMALLOC_TAG" ]]; then
    JEMALLOC_TAG=$DEFAULT_JEMALLOC_TAG
  fi
  if [[ -z "$JEMALLOC_GIT_URL" ]]; then
    JEMALLOC_GIT_URL="${DEFAULT_JEMALLOC_GIT_URL}"
  fi
  JEMALLOC_CFLAGS="$CFLAGS -Wno-error"
  JEMALLOC_LDFLAGS="$LDFLAGS"
  if [[ -n "$LTO" ]]; then
    JEMALLOC_CFLAGS="$JEMALLOC_CFLAGS -flto"
    JEMALLOC_LDFLAGS="$JEMALLOC_LDFLAGS -flto"
  fi
  (
    cd "${JEMALLOC_SRC}"
    git init
    git fetch "${JEMALLOC_GIT_URL}" refs/tags/"${JEMALLOC_TAG}" --depth=1
    git checkout FETCH_HEAD
    CFLAGS="$JEMALLOC_CFLAGS" \
    CXXFLAGS="$JEMALLOC_CFLAGS" \
    LDFLAGS="$JEMALLOC_LDFLAGS" \
    ./autogen.sh \
      --disable-shared \
      --enable-static \
      --disable-libdl \
      --disable-syscall \
      --disable-stats
    make -j$(nproc)
  ) > $JEMALLOC_LOG 2>&1
  cp "${JEMALLOC_SRC}/lib/libjemalloc.a" "${OUT}"
}

ALLOCATOR_LIB=
if [[ -n "${ALLOCATOR}" ]]; then
  case "${ALLOCATOR}" in
    rpmalloc)
      ALLOCATOR_LIB="${XDG_CACHE_HOME}/gn-build/librpmalloc.a"
      if [[ -z "$ALLOCATOR_LINK_ONLY" ]]; then
        download_and_build_rpmalloc "${ALLOCATOR_LIB}"
      fi
      LDFLAGS+=" -lpthread -ldl -static-libstdc++"
      ;;
    jemalloc)
      ALLOCATOR_LIB="${XDG_CACHE_HOME}/gn-build/libjemalloc.a"
      if [[ -z "$ALLOCATOR_LINK_ONLY" ]]; then
        download_and_build_jemalloc "${ALLOCATOR_LIB}"
      fi
      ;;
    *)
      die "Invalid --allocator value ($ALLOCATOR), expected one of: rpmalloc, jemalloc"
      ;;
  esac
  if [[ ! -f "${ALLOCATOR_LIB}" ]]; then
    die "${ALLOCATOR} library is missing, please invoke without --allocator-link-only once to generate it!"
  fi
  echo "Using $ALLOCATOR library: $ALLOCATOR_LIB."
  GN_BUILD_GEN_ARGS="$GN_BUILD_GEN_ARGS --link-lib=\"$ALLOCATOR_LIB\""
fi

build/gen.py $GN_BUILD_GEN_ARGS
"${NINJA}" -C out
if [[ -n "${WINDOWS_ENV}" ]]; then
  wine out/gn_unittests.exe
else
  out/gn_unittests
fi

if [[ -n "${INSTALL_TO}" ]]; then
  mkdir -p "$(dirname "${INSTALL_TO}")"
  cp out/gn "${INSTALL_TO}"
fi

