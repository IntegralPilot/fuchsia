# Build a custom Rust toolchain for Fuchsia

This guide explains how to build a Rust compiler for use with the Fuchsia. This
is useful if you need to build Fuchsia with a patched compiler, or a compiler
built with custom options. Building a custom Rust toolchain is not always
necessary for building Fuchsia with a different version of Rust; see
[Build Fuchsia with a custom Rust toolchain](/docs/development/build/fuchsia_custom_rust.md)
for details.

## Prerequisites

Prior to building a custom Rust toolchain for Fuchsia, you need to do the following:

1. If you haven't already, clone the Rust source. The
   [Guide to Rustc Development] is a good resource to reference whenever you're
   working on the compiler.

   ```posix-terminal
   DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var> '}} # parent of your Rust directory
   git clone --recurse-submodules https://github.com/rust-lang/rust.git $DEV_ROOT/rust
   ```

1. Run the following command to install cmake, ninja, and optionally sccache:

   ```posix-terminal
   sudo apt-get install cmake ninja-build sccache
   ```

   Note: `sccache` is optional but helpful. It caches both C/C++ and Rust
   compilations, significantly speeding up clean and rebuild times. If you
   choose not to use it, omit `sccache` from the command above and omit all
   lines in follow-up steps that use `sccache`.

1. Run the following command to obtain the infra sources:

   ```posix-terminal
   DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var> '}} # parent of your Rust directory

   mkdir -p "$DEV_ROOT/infra" && \
   ( \
     builtin cd "$DEV_ROOT/infra" && \
     jiri init && \
     jiri import -overwrite -name=fuchsia/manifest infra \
         https://fuchsia.googlesource.com/manifest && \
     jiri update \
   )
   ```

   Note: Running `jiri update` from the `infra` directory ensures that you
   have the most recent configurations and tools.

1. Run the following command to use `cipd` to get a Fuchsia core IDK, a Linux
   sysroot, a recent version of clang, and the correct beta compiler for
   building Fuchsia's Rust toolchain:

   ```posix-terminal
   DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var>' }}
   HOST_TRIPLE={{ '<var>' }}x86_64-unknown-linux-gnu{{ '</var>' }}
   CIPD_DIR=$DEV_ROOT/cipd

   mkdir -p $CIPD_DIR
   cat << "EOF" > ${CIPD_DIR}/cipd.ensure
   fuchsia/third_party/clang/${platform} latest
   fuchsia/third_party/cmake/${platform} integration
   fuchsia/third_party/ninja/${platform} integration
   fuchsia/third_party/make/${platform} version:4.3
   infra/3pp/tools/go/${platform} version:3@1.24.1

   @Subdir linux
   fuchsia/third_party/sysroot/linux integration

   @Subdir ubuntu20.04
   fuchsia/third_party/sysroot/focal latest

   @Subdir sdk
   fuchsia/sdk/core/${platform} latest

   @Subdir breakpad
   fuchsia/tools/breakpad/${platform} integration
   EOF

   STAGE0_DATE=$(sed -nr 's/^compiler_date=(.*)/\1/p' ${DEV_ROOT}/rust/src/stage0)
   STAGE0_VERSION=$(sed -nr 's/^compiler_version=(.*)/\1/p' ${DEV_ROOT}/rust/src/stage0)
   STAGE0_COMMIT_HASH=$( \
     curl -s "https://static.rust-lang.org/dist/${STAGE0_DATE}/channel-rust-${STAGE0_VERSION}.toml" \
     | python3 -c 'import tomllib, sys; print(tomllib.load(sys.stdin.buffer)["pkg"]["rust"]["git_commit_hash"])')

   echo "@Subdir stage0" >> ${CIPD_DIR}/cipd.ensure
   echo "fuchsia/third_party/rust/host/\${platform} rust_revision:${STAGE0_COMMIT_HASH}" >> ${CIPD_DIR}/cipd.ensure
   echo "fuchsia/third_party/rust/target/${HOST_TRIPLE} rust_revision:${STAGE0_COMMIT_HASH}" >> ${CIPD_DIR}/cipd.ensure

   $DEV_ROOT/infra/fuchsia/prebuilt/tools/cipd ensure \
     --root "${CIPD_DIR}" \
     --ensure-file "${CIPD_DIR}/cipd.ensure"
   ```

   Note: these versions are not pinned, so every time you run the `cipd ensure`
   command, you will get an updated version. As of writing, however, this
   matches the recipe behavior.

   Downloading the Fuchsia-built stage0 compiler is optional, but useful for
   recreating builds in CI. If the stage0 is not available you may instruct
   the Rust build to download and use the upstream stage0 compiler by omitting
   those lines from your `cipd.ensure` file and removing the `--stage0`
   arguments to `generate_config.py` below.

1. Run the following command to build zlib.

   ```posix-terminal
   DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var> '}} # parent of your Rust directory

   CIPD_DIR=$DEV_ROOT/cipd
   ZLIB_DIR="${DEV_ROOT}/zlib"
   ZLIB_BUILD_DIR="${DEV_ROOT}/build/zlib"
   ZLIB_INSTALL_DIR="${DEV_ROOT}/install/zlib"
   HOST_SYSROOT="${CIPD_DIR}/linux"

   if [[ ! -e  "$ZLIB_DIR" ]]; then
     git clone https://fuchsia.googlesource.com/third_party/zlib $ZLIB_DIR
   fi

   cd $ZLIB_DIR
   git pull --ff-only

   if [[ ! -e "${ZLIB_BUILD_DIR}" ]]; then
     mkdir -p "${ZLIB_BUILD_DIR}"
   fi

   if [[ ! -e "${ZLIB_INSTALL_DIR}" ]]; then
     mkdir -p "${ZLIB_INSTALL_DIR}"
   fi

   ${CIPD_DIR}/bin/cmake \
     -S $ZLIB_DIR \
     -B $ZLIB_BUILD_DIR \
     -G Ninja \
     -DCMAKE_C_COMPILER_LAUNCHER=sccache \
     -DCMAKE_CXX_COMPILER_LAUNCHER=sccache \
     -DCMAKE_BUILD_TYPE=Release \
     -DCMAKE_MAKE_PROGRAM=${CIPD_DIR}/bin/ninja \
     -DCMAKE_INSTALL_PREFIX= \
     -DCMAKE_C_COMPILER=${CIPD_DIR}/bin/clang \
     -DCMAKE_CXX_COMPILER=${CIPD_DIR}/bin/clang++ \
     -DCMAKE_ASM_COMPILER=${CIPD_DIR}/bin/clang \
     -DCMAKE_LINKER=${CIPD_DIR}/bin/ld.lld \
     -DCMAKE_SYSROOT=${HOST_SYSROOT} \
     -DCMAKE_AR=${CIPD_DIR}/bin/llvm-ar \
     -DCMAKE_NM=${CIPD_DIR}/bin/llvm-nm \
     -DCMAKE_OBJCOPY=${CIPD_DIR}/bin/llvm-objcopy \
     -DCMAKE_OBJDUMP=${CIPD_DIR}/bin/llvm-objdump \
     -DCMAKE_RANLIB=${CIPD_DIR}/bin/llvm-ranlib \
     -DCMAKE_READELF=${CIPD_DIR}/bin/llvm-readelf \
     -DCMAKE_STRIP=${CIPD_DIR}/bin/llvm-strip \
     -DCMAKE_SHARED_LINKER_FLAGS=-Wl,--undefined-version \
     -DCMAKE_C_FLAGS=--target=x86_64-linux-gnu \
     -DCMAKE_CXX_FLAGS=--target=x86_64-linux-gnu \
     -DCMAKE_ASM_FLAGS=--target=x86_64-linux-gnu

   ${CIPD_DIR}/bin/ninja \
     -C $ZLIB_BUILD_DIR

   DESTDIR=$ZLIB_INSTALL_DIR ${CIPD_DIR}/bin/ninja \
     -C "$ZLIB_BUILD_DIR" \
     install
   ```

1. Run the following command to build zstd.

   ```posix-terminal
   DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var> '}} # parent of your Rust directory

   CIPD_DIR=$DEV_ROOT/cipd
   ZSTD_DIR=$DEV_ROOT/zstd
   ZSTD_BUILD_DIR=$DEV_ROOT/build/zstd
   ZSTD_INSTALL_DIR=$DEV_ROOT/install/zstd
   HOST_SYSROOT="${CIPD_DIR}/linux"

   if [[ ! -e "$ZSTD_DIR" ]]; then
     git clone https://fuchsia.googlesource.com/third_party/zstd $ZSTD_DIR
   fi

   cd $ZSTD_DIR
   git pull --ff-only

   if [[ ! -e "$ZSTD_BUILD_DIR" ]]; then
     mkdir -p "$ZSTD_BUILD_DIR"
   fi

   if [[ ! -e "$ZSTD_INSTALL_DIR" ]]; then
     mkdir -p "$ZSTD_INSTALL_DIR"
   fi

   $CIPD_DIR/bin/cmake \
     -S ${ZSTD_DIR}/build/cmake \
     -B $ZSTD_BUILD_DIR \
     -G Ninja \
     -DCMAKE_C_COMPILER_LAUNCHER=sccache \
     -DCMAKE_CXX_COMPILER_LAUNCHER=sccache \
     -DCMAKE_BUILD_TYPE=Release \
     -DCMAKE_MAKE_PROGRAM=$CIPD_DIR/bin/ninja \
     -DCMAKE_INSTALL_PREFIX= \
     -DCMAKE_C_COMPILER=$CIPD_DIR/bin/clang \
     -DCMAKE_CXX_COMPILER=$CIPD_DIR/bin/clang++ \
     -DCMAKE_ASM_COMPILER=$CIPD_DIR/bin/clang \
     -DCMAKE_LINKER=$CIPD_DIR/bin/ld.lld \
     -DCMAKE_SYSROOT=$HOST_SYSROOT \
     -DCMAKE_AR=$CIPD_DIR/bin/llvm-ar \
     -DCMAKE_NM=$CIPD_DIR/bin/llvm-nm \
     -DCMAKE_OBJCOPY=$CIPD_DIR/bin/llvm-objcopy \
     -DCMAKE_OBJDUMP=$CIPD_DIR/bin/llvm-objdump \
     -DCMAKE_RANLIB=$CIPD_DIR/bin/llvm-ranlib \
     -DCMAKE_READELF=$CIPD_DIR/bin/llvm-readelf \
     -DCMAKE_STRIP=$CIPD_DIR/bin/llvm-strip \
     -DZSTD_BUILD_SHARED=OFF \
     -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
     -DCMAKE_C_FLAGS=--target=x86_64-linux-gnu \
     -DCMAKE_CXX_FLAGS=--target=x86_64-linux-gnu \
     -DCMAKE_ASM_FLAGS=--target=x86_64-linux-gnu

   $CIPD_DIR/bin/ninja \
     -C $ZSTD_BUILD_DIR

   DESTDIR=$ZSTD_INSTALL_DIR $CIPD_DIR/bin/ninja \
     -C $ZSTD_BUILD_DIR \
     install
   ```

[Guide to Rustc Development]: https://rustc-dev-guide.rust-lang.org/building/how-to-build-and-run.html


## Configure Rust for Fuchsia

1. Change into your Rust directory.
1. Run the following command to generate a configuration for the Rust toolchain:

   ```posix-terminal
   DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var>' }}

   CIPD_DIR="${DEV_ROOT}/cipd"
   ZLIB_INSTALL_DIR="${DEV_ROOT}/install/zlib"
   ZSTD_INSTALL_DIR="${DEV_ROOT}/install/zstd"
   HOST_SYSROOT="${CIPD_DIR}/linux"
   STAGE0_DIR="${CIPD_DIR}/stage0"
   WRAPPERS_DIR="${DEV_ROOT}/wrappers"

   mkdir -p "${WRAPPERS_DIR}"

   ( \
     export PATH="${DEV_ROOT}/infra/fuchsia/prebuilt/tools:$PATH" && \
     \
     $DEV_ROOT/infra/fuchsia/prebuilt/tools/vpython3 \
       $DEV_ROOT/infra/fuchsia/recipes/recipes/rust_toolchain.resources/generate_config.py \
         config_toml \
         --targets=aarch64-unknown-linux-gnu,x86_64-unknown-linux-gnu,thumbv6m-none-eabi,thumbv7m-none-eabi,riscv32imc-unknown-none-elf,riscv64gc-unknown-linux-gnu \
         --clang-prefix=$CIPD_DIR \
         --stage0="${STAGE0_DIR}" \
         --prefix="${DEV_ROOT}/install/fuchsia-rust" \
         --host-sysroot="${HOST_SYSROOT}" \
         --channel=nightly \
         --zlib-path="${ZLIB_INSTALL_DIR}" \
         --zstd-path="${ZSTD_INSTALL_DIR}" \
         --llvm-is-vanilla \
         --ccache=sccache \
         --temp-dir="${WRAPPERS_DIR}" \
         --override-windows-rc=false \
        | tee "${DEV_ROOT}/fuchsia-config.toml" && \
     \
     $DEV_ROOT/infra/fuchsia/prebuilt/tools/vpython3 \
         $DEV_ROOT/infra/fuchsia/recipes/recipes/rust_toolchain.resources/generate_config.py \
           environment \
           --targets=aarch64-unknown-linux-gnu,x86_64-unknown-linux-gnu,thumbv6m-none-eabi,thumbv7m-none-eabi,riscv32imc-unknown-none-elf,riscv64gc-unknown-linux-gnu \
           --source="${DEV_ROOT}/rust" \
           --stage0="${STAGE0_DIR}" \
           --clang-prefix="${CIPD_DIR}" \
           --sdk-dir="${CIPD_DIR}/sdk" \
           --linux-sysroot="${HOST_SYSROOT}" \
           --linux-riscv64-sysroot="${CIPD_DIR}/ubuntu20.04" \
           --temp-dir="${WRAPPERS_DIR}" \
           --override-windows-rc=false \
           --eval \
        | tee "${DEV_ROOT}/fuchsia-env.sh" \
   )
   ```

   Note: Passing `--temp-dir="${WRAPPERS_DIR}"` ensures compiler wrapper script
   paths remain deterministic across builds, preventing `sccache` cache misses.
   Passing `--ccache=sccache` enables caching for in-tree LLVM C/C++ builds. If
   you choose not to use `sccache`, omit `--ccache=sccache`.

1. (Optional) Run the following command to tell git to ignore the generated files:

   ```posix-terminal
   echo fuchsia-config.toml >> .git/info/exclude

   echo fuchsia-env.sh >> .git/info/exclude
   ```

1. (Optional) Customize `fuchsia-config.toml`.

## Build and install Rust

1. Change into your Rust source directory.
1. Run the following command to build and install Rust plus the Fuchsia runtimes spec:

   ```posix-terminal
   DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var>' }}

   rm -rf "${DEV_ROOT}/install/fuchsia-rust"
   mkdir -p "${DEV_ROOT}/install/fuchsia-rust"

   # Copy and paste the following subshell to build and install Rust, as needed.
   # The subshell avoids polluting your environment with fuchsia-specific rust settings.
   ( \
     export RUSTC_WRAPPER="sccache" && \
     export SCCACHE_CACHE_SIZE="50G" && \
     export CFLAGS="-I${DEV_ROOT}/install/zlib/include -I${DEV_ROOT}/install/zstd/include" && \
     export CXXFLAGS="-I${DEV_ROOT}/install/zlib/include -I${DEV_ROOT}/install/zstd/include" && \
     export LDFLAGS="-L${DEV_ROOT}/install/zlib/lib -L${DEV_ROOT}/install/zstd/lib" && \
     export RUSTFLAGS="-Clink-arg=-L${DEV_ROOT}/install/zlib/lib -Clink-arg=-L${DEV_ROOT}/install/zstd/lib" && \
     \
     source "${DEV_ROOT}/fuchsia-env.sh" && \
     ./x.py install \
       --config "${DEV_ROOT}/fuchsia-config.toml" \
       --skip-stage0-validation \
    ) && \
    rm -rf "${DEV_ROOT}/install/fuchsia-rust/lib/.build-id" && \
    "${DEV_ROOT}/infra/fuchsia/prebuilt/tools/vpython3" \
     "${DEV_ROOT}/infra/fuchsia/recipes/recipes/rust_toolchain.resources/generate_config.py" \
       runtime \
     | "${DEV_ROOT}/infra/fuchsia/prebuilt/tools/vpython3" \
         "${DEV_ROOT}/infra/fuchsia/recipes/recipe_modules/toolchain/resources/runtimes.py" \
           --dir "${DEV_ROOT}/install/fuchsia-rust/lib" \
           --build-id-repo debug/.build-id \
           --readelf "${CIPD_DIR}/bin/llvm-readelf" \
           --dump_syms "${CIPD_DIR}/breakpad/dump_syms/dump_syms" \
           --objcopy "${CIPD_DIR}/bin/llvm-objcopy" \
           --dist dist \
     > "${DEV_ROOT}/install/fuchsia-rust/lib/runtime.json"
   ```

   Note: Incremental compilation (`-i`) is omitted because it is incompatible
   with `sccache`. If you choose not to use `sccache`, omit `RUSTC_WRAPPER` and
   `SCCACHE_CACHE_SIZE`, and you may optionally pass `-i` to enable incremental
   compilation.

### Build only (optional)

If you want to skip the install step, for instance during development of Rust
itself, you can do so with the following command (omit `RUSTC_WRAPPER` and
`SCCACHE_CACHE_SIZE` if not using `sccache`).

```posix-terminal
DEV_ROOT={{ '<var>' }}DEV_ROOT{{ '</var>' }}

( \
  export RUSTC_WRAPPER="sccache" && \
  export SCCACHE_CACHE_SIZE="50G" && \
  export CFLAGS="-I${DEV_ROOT}/install/zlib/include -I${DEV_ROOT}/install/zstd/include" && \
  export CXXFLAGS="-I${DEV_ROOT}/install/zlib/include -I${DEV_ROOT}/install/zstd/include" && \
  export LDFLAGS="-L${DEV_ROOT}/install/zlib/lib -L${DEV_ROOT}/install/zstd/lib" && \
  export RUSTFLAGS="-Clink-arg=-L${DEV_ROOT}/install/zlib/lib -Clink-arg=-L${DEV_ROOT}/install/zstd/lib" && \
  \
  source "${DEV_ROOT}/fuchsia-env.sh" && \
  \
  ./x.py build \
  --config \
    "${DEV_ROOT}/fuchsia-config.toml" \
    --skip-stage0-validation \
)
```

### Troubleshooting

If you are getting build errors, try deleting the Rust build directory:

```posix-terminal
rm -rf fuchsia-build
```

Then re-run the command to build Rust.

## Building Fuchsia with a custom Rust toolchain

With a newly compiled custom Rust toolchain, you're ready to use it to build
Fuchsia. Directions on how to do so are available in a [dedicated guide].

[dedicated guide]: /docs/development/build/fuchsia_custom_rust.md
