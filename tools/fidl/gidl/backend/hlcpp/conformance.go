// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package hlcpp

import (
	"bytes"
	_ "embed"
	"fmt"
	"slices"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/config"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/hlcpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
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
	Name, HandleDefs, ValueType, ValueBuild, ValueVar, Bytes, Handles, WireFormat string
	FuchsiaOnly, CheckRights                                                      bool
}

type decodeSuccessCase struct {
	Name, HandleDefs, ValueType, ActualValueVar, EqualityCheck, Bytes, Handles, HandleKoidVectorName, WireFormat string
	FuchsiaOnly                                                                                                  bool
}

type encodeFailureCase struct {
	Name, HandleDefs, ValueType, ValueBuild, ValueVar, ErrorCode, WireFormat string
	FuchsiaOnly                                                              bool
}

type decodeFailureCase struct {
	Name, HandleDefs, ValueType, Bytes, Handles, ErrorCode, WireFormat string
	FuchsiaOnly                                                        bool
}

// Generate generates High-Level C++ tests.
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
	input := conformanceTmplInput{
		EncodeSuccessCases: encodeSuccessCases,
		DecodeSuccessCases: decodeSuccessCases,
		EncodeFailureCases: encodeFailureCases,
		DecodeFailureCases: decodeFailureCases,
	}
	var buf bytes.Buffer
	err = conformanceTmpl.Execute(&buf, input)
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
		valueBuilder := newCppValueBuilder()
		valueVar := valueBuilder.visit(encodeSuccess.Value, decl)
		valueBuild := valueBuilder.String()
		fuchsiaOnly := decl.IsResourceType() || len(encodeSuccess.HandleDefs) > 0
		for _, encoding := range encodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			encodeSuccessCases = append(encodeSuccessCases, encodeSuccessCase{
				Name:        testCaseName(encodeSuccess.Name, encoding.WireFormat),
				HandleDefs:  handleDefs,
				ValueBuild:  valueBuild,
				ValueVar:    valueVar,
				ValueType:   declName(decl),
				Bytes:       hlcpp.BuildBytes(encoding.Bytes),
				Handles:     hlcpp.BuildRawHandleDispositions(encoding.HandleDispositions),
				FuchsiaOnly: fuchsiaOnly,
				CheckRights: encodeSuccess.CheckHandleRights,
				WireFormat:  wireFormatEnum(encoding.WireFormat),
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
		actualValueVar := "value"
		handleKoidVectorName := "handle_koids"
		fuchsiaOnly := decl.IsResourceType() || len(decodeSuccess.HandleDefs) > 0
		equalityCheck := BuildEqualityCheck(actualValueVar, decodeSuccess.Value, decl, handleKoidVectorName)
		for _, encoding := range decodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			decodeSuccessCases = append(decodeSuccessCases, decodeSuccessCase{
				Name:                 testCaseName(decodeSuccess.Name, encoding.WireFormat),
				HandleDefs:           handleDefs,
				ActualValueVar:       actualValueVar,
				ValueType:            declName(decl),
				Bytes:                hlcpp.BuildBytes(encoding.Bytes),
				Handles:              hlcpp.BuildRawHandleInfos(encoding.Handles),
				FuchsiaOnly:          fuchsiaOnly,
				EqualityCheck:        equalityCheck,
				HandleKoidVectorName: handleKoidVectorName,
				WireFormat:           wireFormatEnum(encoding.WireFormat),
			})
		}
	}
	return decodeSuccessCases, nil
}

func encodeFailureCases(gidlEncodeFailures []ir.EncodeFailure, schema mixer.Schema) ([]encodeFailureCase, error) {
	var encodeFailureCases []encodeFailureCase
	for _, encodeFailure := range gidlEncodeFailures {
		decl, err := schema.ExtractDeclarationUnsafe(encodeFailure.Value)
		if err != nil {
			return nil, fmt.Errorf("encode failure %s: %s", encodeFailure.Name, err)
		}
		handleDefs := hlcpp.BuildHandleDefs(encodeFailure.HandleDefs)
		valueBuilder := newCppValueBuilder()
		valueVar := valueBuilder.visit(encodeFailure.Value, decl)
		valueBuild := valueBuilder.String()
		errorCode := cppErrorCode(encodeFailure.Err)
		fuchsiaOnly := decl.IsResourceType() || len(encodeFailure.HandleDefs) > 0
		for _, wireFormat := range supportedWireFormats {
			encodeFailureCases = append(encodeFailureCases, encodeFailureCase{
				Name:        testCaseName(encodeFailure.Name, wireFormat),
				HandleDefs:  handleDefs,
				ValueBuild:  valueBuild,
				ValueVar:    valueVar,
				ValueType:   declName(decl),
				ErrorCode:   errorCode,
				FuchsiaOnly: fuchsiaOnly,
				WireFormat:  wireFormatEnum(wireFormat),
			})
		}
	}
	return encodeFailureCases, nil
}

func decodeFailureCases(gidlDecodeFailures []ir.DecodeFailure, schema mixer.Schema) ([]decodeFailureCase, error) {
	var decodeFailureCases []decodeFailureCase
	for _, decodeFailure := range gidlDecodeFailures {
		decl, err := schema.ExtractDeclarationByName(decodeFailure.Type)
		if err != nil {
			return nil, fmt.Errorf("decode failure %s: %s", decodeFailure.Name, err)
		}
		handleDefs := hlcpp.BuildHandleInfoDefs(decodeFailure.HandleDefs)
		valueType := cppConformanceType(decodeFailure.Type)
		errorCode := cppErrorCode(decodeFailure.Err)
		fuchsiaOnly := decl.IsResourceType() || len(decodeFailure.HandleDefs) > 0
		for _, encoding := range decodeFailure.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			decodeFailureCases = append(decodeFailureCases, decodeFailureCase{
				Name:        testCaseName(decodeFailure.Name, encoding.WireFormat),
				HandleDefs:  handleDefs,
				ValueType:   valueType,
				Bytes:       hlcpp.BuildBytes(encoding.Bytes),
				Handles:     hlcpp.BuildRawHandleInfos(encoding.Handles),
				ErrorCode:   errorCode,
				FuchsiaOnly: fuchsiaOnly,
				WireFormat:  wireFormatEnum(encoding.WireFormat),
			})
		}
	}
	return decodeFailureCases, nil
}

func wireFormatEnum(wireFormat ir.WireFormat) string {
	return fmt.Sprintf("fidl::internal::WireFormatVersion::k%s", fidlgen.ToUpperCamelCase(string(wireFormat)))
}

var supportedWireFormats = []ir.WireFormat{
	ir.V2WireFormat,
}

func wireFormatSupported(wireFormat ir.WireFormat) bool {
	return slices.Contains(supportedWireFormats, wireFormat)
}

func testCaseName(baseName string, wireFormat ir.WireFormat) string {
	return fmt.Sprintf("%s_%s", baseName,
		fidlgen.ToUpperCamelCase(string(wireFormat)))
}

func cppErrorCode(code ir.ErrorCode) string {
	if code == ir.TooFewBytesInPrimaryObject {
		return "ZX_ERR_BUFFER_TOO_SMALL"
	}
	// TODO(https://fxbug.dev/42110793) Implement different codes for different FIDL error cases.
	return "ZX_ERR_INVALID_ARGS"
}

func cppConformanceType(gidlTypeString string) string {
	return "test::conformance::" + gidlTypeString
}
