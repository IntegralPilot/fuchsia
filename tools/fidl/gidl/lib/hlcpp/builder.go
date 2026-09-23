// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package hlcpp

import (
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

func BuildBytes(bytes []byte) string {
	var builder strings.Builder
	builder.WriteString("std::vector<uint8_t>{")
	for i, b := range bytes {
		if i%8 == 0 {
			builder.WriteString("\n")
		}
		builder.WriteString(fmt.Sprintf("0x%02x,", b))
	}
	builder.WriteString("\n}")
	return builder.String()
}

func handleType(subtype fidlgen.HandleSubtype) string {
	switch subtype {
	case fidlgen.HandleSubtypeChannel:
		return "ZX_OBJ_TYPE_CHANNEL"
	case fidlgen.HandleSubtypeEvent:
		return "ZX_OBJ_TYPE_EVENT"
	default:
		panic(fmt.Sprintf("unsupported handle subtype: %s", subtype))
	}
}

func buildHandleDef(def ir.HandleDef) string {
	switch def.Subtype {
	case fidlgen.HandleSubtypeChannel:
		return fmt.Sprintf("fidl::test::util::CreateChannel(%d)", def.Rights)
	case fidlgen.HandleSubtypeEvent:
		return fmt.Sprintf("fidl::test::util::CreateEvent(%d)", def.Rights)
	default:
		panic(fmt.Sprintf("unsupported handle subtype: %s", def.Subtype))
	}
}

func BuildHandleDefs(defs []ir.HandleDef) string {
	if len(defs) == 0 {
		return ""
	}
	var builder strings.Builder
	builder.WriteString("std::vector<zx_handle_t>{\n")
	for i, d := range defs {
		builder.WriteString(buildHandleDef(d))
		// Write indices corresponding to the .gidl file handle_defs block.
		builder.WriteString(fmt.Sprintf(", // #%d\n", i))
	}
	builder.WriteString("}")
	return builder.String()
}

func BuildHandleInfoDefs(defs []ir.HandleDef) string {
	if len(defs) == 0 {
		return ""
	}
	var builder strings.Builder
	builder.WriteString("std::vector<zx_handle_info_t>{\n")
	for i, d := range defs {
		builder.WriteString(fmt.Sprintf(`
// #%d
zx_handle_info_t{
	.handle = %s,
	.type = %s,
	.rights = %d,
	.unused = 0u,
},
`, i, buildHandleDef(d), handleType(d.Subtype), d.Rights))
	}
	builder.WriteString("}")
	return builder.String()
}

func buildRawHandleImpl(handles []ir.Handle, handleType, handleExtractOp string) string {
	if len(handles) == 0 {
		return fmt.Sprintf("std::vector<%s>{}", handleType)
	}
	var builder strings.Builder
	builder.WriteString(fmt.Sprintf("std::vector<%s>{\n", handleType))
	for i, h := range handles {
		builder.WriteString(fmt.Sprintf("handle_defs[%d]%s,", h, handleExtractOp))
		if i%8 == 7 {
			builder.WriteString("\n")
		}
	}
	builder.WriteString("}")
	return builder.String()
}

func BuildRawHandles(handles []ir.Handle) string {
	return buildRawHandleImpl(handles, "zx_handle_t", "")
}

func BuildRawHandlesFromHandleInfos(handles []ir.Handle) string {
	return buildRawHandleImpl(handles, "zx_handle_t", ".handle")
}

func BuildRawHandleInfos(handles []ir.Handle) string {
	return buildRawHandleImpl(handles, "zx_handle_info_t", "")
}

func BuildRawHandleDispositions(handle_dispositions []ir.HandleDisposition) string {
	if len(handle_dispositions) == 0 {
		return fmt.Sprintf("std::vector<zx_handle_disposition_t>{}")
	}
	var builder strings.Builder
	builder.WriteString(fmt.Sprintf("std::vector<zx_handle_disposition_t>{"))
	for _, h := range handle_dispositions {
		builder.WriteString(fmt.Sprintf(`
{
	.operation = ZX_HANDLE_OP_MOVE,
	.handle = handle_defs[%d],
	.type = %d,
	.rights = %d,
	.result = ZX_OK,
},`, h.Handle, h.Type, h.Rights))
	}
	builder.WriteString("}")
	return builder.String()
}
