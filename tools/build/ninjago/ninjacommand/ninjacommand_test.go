// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ninjacommand

import (
	"testing"
)

func TestCategory(t *testing.T) {
	for _, tc := range []struct {
		command string
		want    string
	}{
		{
			want: "unknown",
		},
		{
			command: "'touch' a",
			want:    "touch",
		},
		{
			command: `"rm" a`,
			want:    "rm",
		},
		{
			command: "touch obj/third_party/cobalt/src/lib/client/rust/cobalt-client.inputs.stamp",
			want:    "touch",
		},
		{
			command: "ln -f ../../zircon/third_party/ulib/musl/include/libgen.h zircon_toolchain/obj/zircon/public/sysroot/sysroot/include/libgen.h 2>/dev/null || (rm -rf zircon_toolchain/obj/zircon/public/sysroot/sysroot/include/libgen.h && cp -af ../../zircon/third_party/ulib/musl/include/libgen.h zircon_toolchain/obj/zircon/public/sysroot/sysroot/include/libgen.h)",
			want:    "ln,rm,cp",
		},
		{
			command: "/usr/bin/env ../../build/gn_run_binary.sh ../../prebuilt/third_party/clang/linux-x64/bin host_x64/fidlgen_rust --json fidling/gen/sdk/fidl/fuchsia.wlan.product.deprecatedconfiguration/fuchsia.wlan.product.deprecatedconfiguration.fidl.json --output-filename fidling/gen/sdk/fidl/fuchsia.wlan.product.deprecatedconfiguration/fidl_fuchsia_wlan_product_deprecatedconfiguration.rs --rustfmt /home/jayzhuang/fuchsia/prebuilt/third_party/rust/linux-x64/bin/rustfmt",
			want:    "fidlgen_rust",
		},
		{
			command: "/usr/bin/env ../../prebuilt/third_party/python3/linux-x64/bin/python3.8 ../../build/sdk/compute_atom_api.py --output /usr/local/google/home/jayzhuang/fuchsia/out/default/fidling/gen/sdk/fidl/fuchsia.media.target/fuchsia.media.target_sdk.api --file fidl/fuchsia.media.target /usr/local/google/home/jayzhuang/fuchsia/out/default/fidling/gen/sdk/fidl/fuchsia.media.target/fuchsia.media.target.normalized",
			want:    "compute_atom_api.py",
		},
		{
			command: "../../prebuilt/third_party/python3/linux-x64/bin/python3.8 action_tracer.py --some test args to ignore -- real_command && touch",
			want:    "real_command,touch",
		},
		{
			command: "../../prebuilt/third_party/python3/linux-x64/bin/python3.8 output_cacher.py --some args --to ignore -- ../../prebuilt/third_party/python3/linux-x64/bin/python3.8 action_tracer.py --yet more -args to ignore -- ../../prebuilt/third_party/python3/linux-x64/bin/python3.8 build_everything.py",
			want:    "build_everything.py",
		},
		{
			command: "../../build/rbe/reclient_cxx.sh --working-subdir=out/default --exec_strategy=remote_local_fallback --preserve_unchanged_output_mtime --  ../../prebuilt/third_party/clang/linux-x64/bin/clang++ -MD -MF host_x64/obj/third_party/protobuf/src/google/protobuf/util/internal/libprotobuf_full.json_objectwriter.cc.o.d -DTOOLCHAIN_VERSION=SkDvAQt_IN7-4-_K2xShZJxH9sfemvCs0bioG10wxIEC -D_LIBCPP_DISABLE_VISIBILITY_ANNOTATIONS -D_LIBCPP_REMOVE_TRANSITIVE_INCLUDES -DGOOGLE_PROTOBUF_NO_RTTI -DHAVE_PTHREAD -I../.. -Ihost_x64/gen -I../../third_party/protobuf/src -fcolor-diagnostics -fcrash-diagnostics-dir=clang-crashreports -fcrash-diagnostics=all -ffp-contract=off --sysroot=../../prebuilt/third_party/sysroot/linux --target=x86_64-unknown-linux-gnu -ffile-compilation-dir=. -no-canonical-prefixes -fomit-frame-pointer -fdata-sections -ffunction-sections -O0 -Xclang -debug-info-kind=constructor -g3 -grecord-gcc-switches -gdwarf-5 -gz=zstd -Wall -Wextra -Wconversion -Wextra-semi -Wimplicit-fallthrough -Wnewline-eof -Wstrict-prototypes -Wwrite-strings -Wno-sign-conversion -Wno-unused-parameter -Wnonportable-system-include-path -Wno-missing-field-initializers -Wno-extra-qualification -Wno-cast-function-type-strict -Wno-cast-function-type-mismatch -Wno-unknown-warning-option -fvisibility=hidden -Werror -Wa,--fatal-warnings --sysroot=../../prebuilt/third_party/sysroot/linux --target=x86_64-unknown-linux-gnu -fPIE -Wno-deprecated-pragma -Wno-enum-enum-conversion -Wno-extra-semi -Wno-float-conversion -Wno-implicit-float-conversion -Wno-implicit-int-conversion -Wno-implicit-int-float-conversion -Wno-invalid-noreturn -Wno-missing-field-initializers -Wno-sign-compare -Wno-unused-function -Wno-unused-private-field -Wno-deprecated-declarations -Wno-c++98-compat-extra-semi -Wno-shorten-64-to-32 -fvisibility-inlines-hidden -stdlib=libc++ -std=c++17 -fno-exceptions -fno-rtti -ftemplate-backtrace-limit=0 -stdlib=libc++ -c ../../third_party/protobuf/src/google/protobuf/util/internal/json_objectwriter.cc -o host_x64/obj/third_party/protobuf/src/google/protobuf/util/internal/libprotobuf_full.json_objectwriter.cc.o",
			want:    "clang++",
		},
	} {
		if got := ComputeCommandCategories(tc.command); got != tc.want {
			t.Errorf("ComputeCommandCategories() = %s, want: %s, command: #%v", got, tc.want, tc.command)
		}
	}
}
