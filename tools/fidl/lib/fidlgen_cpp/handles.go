// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen_cpp

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type HandleInformation struct {
	ObjectType string
	Rights     string
}

func (c *compiler) fieldHandleInformation(val fidlgen.Type) *HandleInformation {
	return FieldHandleInformation(val, c.decls)
}

func FieldHandleInformation(val fidlgen.Type, decls fidlgen.DeclInfoMap) *HandleInformation {
	for val.ElementType != nil {
		val = *val.ElementType
	}
	if val.Kind == fidlgen.EndpointType {
		return &HandleInformation{
			ObjectType: "ZX_OBJ_TYPE_CHANNEL",
			Rights:     "ZX_DEFAULT_CHANNEL_RIGHTS",
		}
	}
	if val.Kind == fidlgen.HandleType {
		subtype, ok := handleSubtypeConsts[val.HandleSubtype]
		if !ok {
			panic(fmt.Sprintf("unknown handle type for const: %v", val))
		}
		return &HandleInformation{
			ObjectType: fmt.Sprintf("ZX_OBJ_TYPE_%s", subtype),
			Rights:     fmt.Sprintf("0x%x", val.HandleRights),
		}
	}
	return nil
}

var handleSubtypeConsts = map[fidlgen.HandleSubtype]string{
	fidlgen.HandleSubtypeBti:          "BTI",
	fidlgen.HandleSubtypeChannel:      "CHANNEL",
	fidlgen.HandleSubtypeClock:        "CLOCK",
	fidlgen.HandleSubtypeCounter:      "COUNTER",
	fidlgen.HandleSubtypeDebugLog:     "DEBUGLOG",
	fidlgen.HandleSubtypeEvent:        "EVENT",
	fidlgen.HandleSubtypeEventpair:    "EVENTPAIR",
	fidlgen.HandleSubtypeException:    "EXCEPTION",
	fidlgen.HandleSubtypeFifo:         "FIFO",
	fidlgen.HandleSubtypeGuest:        "GUEST",
	fidlgen.HandleSubtypeInterrupt:    "INTERRUPT",
	fidlgen.HandleSubtypeIob:          "IOB",
	fidlgen.HandleSubtypeIommu:        "IOMMU",
	fidlgen.HandleSubtypeJob:          "JOB",
	fidlgen.HandleSubtypeMsi:          "MSI",
	fidlgen.HandleSubtypeNone:         "NONE",
	fidlgen.HandleSubtypePager:        "PAGER",
	fidlgen.HandleSubtypePmt:          "PMT",
	fidlgen.HandleSubtypePort:         "PORT",
	fidlgen.HandleSubtypeProcess:      "PROCESS",
	fidlgen.HandleSubtypeProfile:      "PROFILE",
	fidlgen.HandleSubtypeResource:     "RESOURCE",
	fidlgen.HandleSubtypeSocket:       "SOCKET",
	fidlgen.HandleSubtypeStream:       "STREAM",
	fidlgen.HandleSubtypeSuspendToken: "SUSPEND_TOKEN",
	fidlgen.HandleSubtypeThread:       "THREAD",
	fidlgen.HandleSubtypeTimer:        "TIMER",
	fidlgen.HandleSubtypeVcpu:         "VCPU",
	fidlgen.HandleSubtypeVmar:         "VMAR",
	fidlgen.HandleSubtypeVmo:          "VMO",
}

// Header names for to use for handles where the name isn't the same as HandleSubtype.
// For any subtype not in this list, string(HandleSubtype) is used instead.
var handleHeaderNames = map[fidlgen.HandleSubtype]string{
	fidlgen.HandleSubtypeSuspendToken: "lib/zx/suspend_token.h",
}

// Get the correct header to include in order to use the given handle subtype.
func handleHeaderName(h fidlgen.HandleSubtype) string {
	if header, ok := handleHeaderNames[h]; ok {
		return header
	}
	return fmt.Sprintf("lib/zx/%s.h", string(h))
}
