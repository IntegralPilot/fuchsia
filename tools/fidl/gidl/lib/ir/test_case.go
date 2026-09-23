// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ir

import (
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type All struct {
	EncodeSuccess []EncodeSuccess
	DecodeSuccess []DecodeSuccess
	EncodeFailure []EncodeFailure
	DecodeFailure []DecodeFailure
	Benchmark     []Benchmark
}

type SourceLocation struct {
	Filename string
	Line     int
}

type EncodeSuccess struct {
	Name              string
	Value             RecordLike
	Encodings         []HandleDispositionEncoding
	HandleDefs        []HandleDef
	BindingsAllowlist *[]Language
	BindingsDenylist  *[]Language
	// CheckHandleRights is true for standalone "encode_success" tests providing
	// "handle_dispositions", but false for bidirectional "success" tests
	// because they provide only "handles" with no rights information.
	CheckHandleRights bool
	SourceLocation
}

type DecodeSuccess struct {
	Name              string
	Value             RecordLike
	Encodings         []Encoding
	HandleDefs        []HandleDef
	BindingsAllowlist *[]Language
	BindingsDenylist  *[]Language
	SourceLocation
}

type EncodeFailure struct {
	Name              string
	Value             RecordLike
	HandleDefs        []HandleDef
	Err               ErrorCode
	BindingsAllowlist *[]Language
	BindingsDenylist  *[]Language
	BindingsSkip      *[]Language
	SourceLocation
}

type DecodeFailure struct {
	Name              string
	Type              string
	Encodings         []Encoding
	HandleDefs        []HandleDef
	Err               ErrorCode
	BindingsAllowlist *[]Language
	BindingsDenylist  *[]Language
	SourceLocation
}

type Benchmark struct {
	Name                     string
	Value                    RecordLike
	HandleDefs               []HandleDef
	BindingsAllowlist        *[]Language
	BindingsDenylist         *[]Language
	EnableSendEventBenchmark bool
	EnableEchoCallBenchmark  bool
	SourceLocation
}

type Language string

const (
	LanguageCpp               Language = "cpp"
	LanguageDart              Language = "dart"
	LanguageDriverCpp         Language = "driver_cpp"
	LanguageDriverLlcpp       Language = "driver_llcpp"
	LanguageDynfidl           Language = "dynfidl"
	LanguageFuchsiaController Language = "fuchsia_controller"
	LanguageFuzzerCorpus      Language = "fuzzer_corpus"
	LanguageGo                Language = "go"
	LanguageHlcpp             Language = "hlcpp"
	LanguageLlcpp             Language = "llcpp"
	LanguagePython            Language = "python"
	LanguageReference         Language = "reference"
	LanguageRust              Language = "rust"
	LanguageRustCodec         Language = "rust_codec"
	LanguageRustNext          Language = "rust_next"
)

func AllLanguages() []Language {
	return []Language{
		LanguageCpp,
		LanguageDart,
		LanguageDriverCpp,
		LanguageDriverLlcpp,
		LanguageDynfidl,
		LanguageFuchsiaController,
		LanguageFuzzerCorpus,
		LanguageGo,
		LanguageHlcpp,
		LanguageLlcpp,
		LanguagePython,
		LanguageReference,
		LanguageRust,
		LanguageRustNext,
	}
}

// Languages which are denied unless present in bindings_allowlist.
var defaultDenyLanguages = map[Language]struct{}{
	LanguageReference: {},
}

type HandleDef struct {
	Subtype fidlgen.HandleSubtype
	Rights  fidlgen.HandleRights
}

var supportedHandleSubtypes = map[fidlgen.HandleSubtype]struct{}{
	fidlgen.HandleSubtypeChannel: {},
	fidlgen.HandleSubtypeEvent:   {},
}

func HandleSubtypeByName(s string) (fidlgen.HandleSubtype, bool) {
	subtype := fidlgen.HandleSubtype(s)
	_, ok := supportedHandleSubtypes[subtype]
	if ok {
		return subtype, true
	}
	return "", false
}

// handleRightsByName is initialized in two phases, constants here, and combined
// rights in `init`.
var handleRightsByName = map[string]fidlgen.HandleRights{
	"none":        fidlgen.HandleRightsNone,
	"same_rights": fidlgen.HandleRightsSameRights,

	"duplicate":      fidlgen.HandleRightsDuplicate,
	"transfer":       fidlgen.HandleRightsTransfer,
	"read":           fidlgen.HandleRightsRead,
	"write":          fidlgen.HandleRightsWrite,
	"execute":        fidlgen.HandleRightsExecute,
	"map":            fidlgen.HandleRightsMap,
	"get_property":   fidlgen.HandleRightsGetProperty,
	"set_property":   fidlgen.HandleRightsSetProperty,
	"enumerate":      fidlgen.HandleRightsEnumerate,
	"destroy":        fidlgen.HandleRightsDestroy,
	"set_policy":     fidlgen.HandleRightsSetPolicy,
	"get_policy":     fidlgen.HandleRightsGetPolicy,
	"signal":         fidlgen.HandleRightsSignal,
	"signal_peer":    fidlgen.HandleRightsSignalPeer,
	"wait":           fidlgen.HandleRightsWait,
	"inspect":        fidlgen.HandleRightsInspect,
	"manage_job":     fidlgen.HandleRightsManageJob,
	"manage_process": fidlgen.HandleRightsManageProcess,
	"manage_thread":  fidlgen.HandleRightsManageThread,
	"apply_profile":  fidlgen.HandleRightsApplyProfile,
}

func init() {
	combinedHandleRights := func(rightsNames ...string) fidlgen.HandleRights {
		var combinedRights fidlgen.HandleRights
		for _, rightsName := range rightsNames {
			rights, ok := HandleRightsByName(rightsName)
			if !ok {
				panic("bug in specifying combined rights: unknown name")
			}
			combinedRights |= rights
		}
		return combinedRights
	}
	handleRightsByName["basic"] = combinedHandleRights("transfer", "duplicate", "wait", "inspect")
	handleRightsByName["io"] = combinedHandleRights("read", "write")
	handleRightsByName["channel_default"] = combinedHandleRights("transfer", "wait", "inspect", "io", "signal", "signal_peer")
	handleRightsByName["event_default"] = combinedHandleRights("basic", "signal")
}

func HandleRightsByName(rightsName string) (fidlgen.HandleRights, bool) {
	rights, ok := handleRightsByName[rightsName]
	return rights, ok
}

type HandleDisposition struct {
	Handle Handle
	Type   fidlgen.ObjectType
	Rights fidlgen.HandleRights
}

type Encoding struct {
	WireFormat WireFormat
	Bytes      []byte
	Handles    []Handle
}

type HandleDispositionEncoding struct {
	WireFormat         WireFormat
	Bytes              []byte
	HandleDispositions []HandleDisposition
}

type WireFormat string

const (
	V2WireFormat WireFormat = "v2"
)

func AllWireFormats() []WireFormat {
	return []WireFormat{V2WireFormat}
}
