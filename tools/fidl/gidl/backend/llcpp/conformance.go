// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package llcpp

import (
	"bytes"
	_ "embed"
	"fmt"
	"slices"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/config"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/hlcpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/llcpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

var (
	//go:embed conformance.tmpl
	conformanceTmplText string

	conformanceTmpl = template.Must(template.New("conformanceTmpl").Parse(conformanceTmplText))
)

type conformanceTmplInput struct {
	EncodeSuccessCases []encodeSuccessCase
	DecodeSuccessCases []decodeSuccessCase
	EncodeFailureCases []encodeFailureCase
	DecodeFailureCases []decodeFailureCase
}

type encodeSuccessCase struct {
	Name, WireFormatVersion, HandleDefs, ValueBuild, ValueVar, Bytes, Handles string
	FuchsiaOnly, CheckHandleRights                                            bool
}

type decodeSuccessCase struct {
	Name, HandleDefs, HandleKoidVectorName string
	WireFormatVersion                      string
	ValueBuild, ValueVar, ValueType        string
	Equality                               llcpp.EqualityCheck
	Bytes, Handles                         string
	FuchsiaOnly                            bool
}

type encodeFailureCase struct {
	Name, WireFormatVersion, HandleDefs, ValueBuild, ValueVar, ErrorCode string
	FuchsiaOnly                                                          bool
}

type decodeFailureCase struct {
	Name, WireFormatVersion, HandleDefs, ValueType, Bytes, Handles, ErrorCode string
	FuchsiaOnly                                                               bool
}

// Generate generates Low-Level C++ tests.
func GenerateConformanceTests(gidl ir.All, fidl fidlgen.Root, config config.GeneratorConfig) ([]byte, error) {
	schema := mixer.BuildSchema(fidl)
	encodeSuccessCases, err := encodeSuccessCases(gidl.EncodeSuccess, schema)
	if err != nil {
		return nil, err
	}
	decodeSuccessCases, err := decodeSuccessCases(gidl.DecodeSuccess, schema)
	if err != nil {
		return nil, err
	}
	encodeFailureCases, err := encodeFailureCases(gidl.EncodeFailure, schema)
	if err != nil {
		return nil, err
	}
	decodeFailureCases, err := decodeFailureCases(gidl.DecodeFailure, schema)
	if err != nil {
		return nil, err
	}
	var buf bytes.Buffer
	err = conformanceTmpl.Execute(&buf, conformanceTmplInput{
		EncodeSuccessCases: encodeSuccessCases,
		DecodeSuccessCases: decodeSuccessCases,
		EncodeFailureCases: encodeFailureCases,
		DecodeFailureCases: decodeFailureCases,
	})
	return buf.Bytes(), err
}

func encodeSuccessCases(gidlEncodeSuccesses []ir.EncodeSuccess, schema mixer.Schema) ([]encodeSuccessCase, error) {
	var encodeSuccessCases []encodeSuccessCase
	for _, encodeSuccess := range gidlEncodeSuccesses {
		decl, err := schema.ExtractDeclarationEncodeSuccess(encodeSuccess.Value, encodeSuccess.HandleDefs)
		if err != nil {
			return nil, fmt.Errorf("encode success %s: %s", encodeSuccess.Name, err)
		}
		handleDefs := hlcpp.BuildHandleDefs(encodeSuccess.HandleDefs)
		valueBuild, valueVar := llcpp.BuildValueAllocator("allocator", encodeSuccess.Value, decl, llcpp.HandleReprRaw)
		fuchsiaOnly := decl.IsResourceType() || len(encodeSuccess.HandleDefs) > 0
		for _, encoding := range encodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			encodeSuccessCases = append(encodeSuccessCases, encodeSuccessCase{
				Name:              testCaseName(encodeSuccess.Name, encoding.WireFormat),
				WireFormatVersion: wireFormatVersionName(encoding.WireFormat),
				HandleDefs:        handleDefs,
				ValueBuild:        valueBuild,
				ValueVar:          valueVar,
				Bytes:             hlcpp.BuildBytes(encoding.Bytes),
				Handles:           hlcpp.BuildRawHandleDispositions(encoding.HandleDispositions),
				FuchsiaOnly:       fuchsiaOnly,
				CheckHandleRights: encodeSuccess.CheckHandleRights,
			})
		}
	}
	return encodeSuccessCases, nil
}

func decodeSuccessCases(gidlDecodeSuccesses []ir.DecodeSuccess, schema mixer.Schema) ([]decodeSuccessCase, error) {
	var decodeSuccessCases []decodeSuccessCase
	for _, decodeSuccess := range gidlDecodeSuccesses {
		decl, err := schema.ExtractDeclaration(decodeSuccess.Value, decodeSuccess.HandleDefs)
		if err != nil {
			return nil, fmt.Errorf("decode success %s: %s", decodeSuccess.Name, err)
		}
		handleDefs := hlcpp.BuildHandleInfoDefs(decodeSuccess.HandleDefs)
		equalityInputVar := "actual"
		handleKoidVectorName := "handle_koids"
		equality := llcpp.BuildEqualityCheck(equalityInputVar, decodeSuccess.Value, decl, handleKoidVectorName)
		fuchsiaOnly := decl.IsResourceType() || len(decodeSuccess.HandleDefs) > 0
		for _, encoding := range decodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			decodeSuccessCases = append(decodeSuccessCases, decodeSuccessCase{
				Name:                 testCaseName(decodeSuccess.Name, encoding.WireFormat),
				WireFormatVersion:    wireFormatVersionName(encoding.WireFormat),
				HandleDefs:           handleDefs,
				ValueType:            llcpp.ConformanceType(ir.TypeFromValue(decodeSuccess.Value)),
				Equality:             equality,
				Bytes:                hlcpp.BuildBytes(encoding.Bytes),
				Handles:              hlcpp.BuildRawHandleInfos(encoding.Handles),
				FuchsiaOnly:          fuchsiaOnly,
				HandleKoidVectorName: handleKoidVectorName,
			})
		}
	}
	return decodeSuccessCases, nil
}

func encodeFailureCases(gidlEncodeFailurees []ir.EncodeFailure, schema mixer.Schema) ([]encodeFailureCase, error) {
	var encodeFailureCases []encodeFailureCase
	for _, encodeFailure := range gidlEncodeFailurees {
		decl, err := schema.ExtractDeclarationUnsafe(encodeFailure.Value)
		if err != nil {
			return nil, fmt.Errorf("encode failure %s: %s", encodeFailure.Name, err)
		}
		handleDefs := hlcpp.BuildHandleDefs(encodeFailure.HandleDefs)
		valueBuild, valueVar := llcpp.BuildValueAllocator("allocator", encodeFailure.Value, decl, llcpp.HandleReprRaw)
		errorCode := llcpp.LlcppErrorCode(encodeFailure.Err)
		fuchsiaOnly := decl.IsResourceType() || len(encodeFailure.HandleDefs) > 0
		for _, wireFormat := range supportedEncodeFailureFormats {
			encodeFailureCases = append(encodeFailureCases, encodeFailureCase{
				Name:              testCaseName(encodeFailure.Name, wireFormat),
				WireFormatVersion: wireFormatVersionName(wireFormat),
				HandleDefs:        handleDefs,
				ValueBuild:        valueBuild,
				ValueVar:          valueVar,
				ErrorCode:         errorCode,
				FuchsiaOnly:       fuchsiaOnly,
			})
		}
	}
	return encodeFailureCases, nil
}

func decodeFailureCases(gidlDecodeFailurees []ir.DecodeFailure, schema mixer.Schema) ([]decodeFailureCase, error) {
	var decodeFailureCases []decodeFailureCase
	for _, decodeFailure := range gidlDecodeFailurees {
		decl, err := schema.ExtractDeclarationByName(decodeFailure.Type)
		if err != nil {
			return nil, fmt.Errorf("decode failure %s: %s", decodeFailure.Name, err)
		}
		handleDefs := hlcpp.BuildHandleInfoDefs(decodeFailure.HandleDefs)
		valueType := llcpp.ConformanceType(decodeFailure.Type)
		errorCode := llcpp.LlcppErrorCode(decodeFailure.Err)
		fuchsiaOnly := decl.IsResourceType() || len(decodeFailure.HandleDefs) > 0
		for _, encoding := range decodeFailure.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			decodeFailureCases = append(decodeFailureCases, decodeFailureCase{
				Name:              testCaseName(decodeFailure.Name, encoding.WireFormat),
				WireFormatVersion: wireFormatVersionName(encoding.WireFormat),
				HandleDefs:        handleDefs,
				ValueType:         valueType,
				Bytes:             hlcpp.BuildBytes(encoding.Bytes),
				Handles:           hlcpp.BuildRawHandleInfos(encoding.Handles),
				ErrorCode:         errorCode,
				FuchsiaOnly:       fuchsiaOnly,
			})
		}
	}
	return decodeFailureCases, nil
}

var supportedWireFormats = []ir.WireFormat{
	ir.V2WireFormat,
}
var supportedEncodeFailureFormats = []ir.WireFormat{
	ir.V2WireFormat,
}

func wireFormatSupported(wireFormat ir.WireFormat) bool {
	return slices.Contains(supportedWireFormats, wireFormat)
}

func wireFormatVersionName(wireFormat ir.WireFormat) string {
	return fmt.Sprintf("::fidl::internal::WireFormatVersion::k%s", fidlgen.ToUpperCamelCase(string(wireFormat)))
}

func testCaseName(baseName string, wireFormat ir.WireFormat) string {
	return fmt.Sprintf("%s_%s", baseName, fidlgen.ToUpperCamelCase(string(wireFormat)))
}
