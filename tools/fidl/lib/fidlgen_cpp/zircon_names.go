// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen_cpp

import (
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type zxName = struct {
	typeName string
	prefix   string
}

var zirconNames = map[string]zxName{
	"Rights": {
		typeName: "zx_rights_t",
		prefix:   "ZX_RIGHT",
	},
	"ObjType": {
		typeName: "zx_obj_type_t",
		prefix:   "ZX_OBJ_TYPE",
	},
}

var zirconTimes = map[string]zxName{
	"InstantMono": {
		typeName: "fidl::basic_time<ZX_CLOCK_MONOTONIC>",
		prefix:   "",
	},
	"InstantBoot": {
		typeName: "fidl::basic_time<ZX_CLOCK_BOOT>",
		prefix:   "",
	},
	"InstantMonoTicks": {
		typeName: "fidl::basic_ticks<ZX_CLOCK_MONOTONIC>",
		prefix:   "",
	},
	"InstantBootTicks": {
		typeName: "fidl::basic_ticks<ZX_CLOCK_BOOT>",
		prefix:   "",
	},
}

func isZirconLibrary(li fidlgen.LibraryIdentifier) bool {
	return len(li) == 1 && li[0] == fidlgen.Identifier("zx")
}

func zirconName(ci fidlgen.CompoundIdentifier) name {
	if ci.Member != "" {
		if zn, ok := zirconValueMember(ci.Name, ci.Member); ok {
			return zn
		}
	} else {
		if zn, ok := zirconType(ci.Name); ok {
			return zn
		}
		if zn, ok := zirconConst(ci.Name); ok {
			return zn
		}
	}

	panic(fmt.Sprintf("Unknown zircon identifier: %s", ci.Encode()))
}

func zirconType(id fidlgen.Identifier) (name, bool) {
	n := string(id)
	if zn, ok := zirconNames[n]; ok {
		return makeName(zn.typeName), true
	}

	return name{}, false
}

func zirconTime(ci fidlgen.CompoundIdentifier) (name, bool) {
	if isZirconLibrary(ci.Library) {
		n := string(ci.Name)
		if zt, ok := zirconTimes[n]; ok {
			return makeName(zt.typeName), true
		}
	}
	return name{}, false
}

func zirconValueMember(id fidlgen.Identifier, mem fidlgen.Identifier) (name, bool) {
	n := string(id)
	m := string(mem)
	if zn, ok := zirconNames[n]; ok {
		return makeName(fmt.Sprintf("%s_%s", zn.prefix, strings.ToUpper(m))), true
	}

	return name{}, false
}

func zirconConst(id fidlgen.Identifier) (name, bool) {
	n := string(id)
	if n == strings.ToUpper(n) {
		// All-caps names like `CHANNEL_MAX_MSG_BYTES`` get a ZX_ prefix.
		return makeName(fmt.Sprintf("ZX_%s", n)), true
	}

	return name{}, false
}
