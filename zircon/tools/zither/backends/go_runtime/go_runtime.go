// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package go_runtime

import (
	"embed"
	"fmt"
	"path/filepath"
	"sort"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither/backends/golang"
)

//go:embed templates/*
var templates embed.FS

type Generator struct {
	// Separate generators to handle the formatting differences.
	goGen  fidlgen.Generator
	asmGen fidlgen.Generator
}

// Go runtime sources, given by source-relative path. Each file has a
// corresponding template named "Generate-${file basename}".
var generatedSources = []string{
	// Syscall bindings are defined in the 'runtime' package instead of the
	// 'syscall/zx' package, since the former currently also needs to make
	// syscalls and the latter is one of its dependents.
	filepath.Join("src", "runtime", "vdso_keys_fuchsia.go"),
	filepath.Join("src", "runtime", "vdsocalls_fuchsia_amd64.s"),
	filepath.Join("src", "runtime", "vdsocalls_fuchsia_arm64.s"),

	// Defines the public `Sys_foo_bar(...)`s, defined as jumps to the runtime
	// package's `vdsoCall_zx_foo_bar` analogues.
	filepath.Join("src", "syscall", "zx", "syscalls_fuchsia.go"),
	filepath.Join("src", "syscall", "zx", "syscalls_fuchsia_amd64.s"),
	filepath.Join("src", "syscall", "zx", "syscalls_fuchsia_arm64.s"),
}

func NewGenerator(formatter fidlgen.Formatter) *Generator {
	funcMap := template.FuncMap{
		"ARM64AsmBinding":          armAsmBinding,
		"FFIParameterType":         ffiParameterType,
		"FFIReturnType":            ffiReturnType,
		"Hash":                     elfGNUHash,
		"LowerCaseWithUnderscores": lowerCaseWithUnderscores,
		"LastParameterIndex":       func(syscall zither.Syscall) int { return len(syscall.Parameters) - 1 },
		"ParameterType":            parameterType,
		"ReturnType":               returnType,
		"X86AsmBinding":            x86AsmBinding,
	}
	return &Generator{
		goGen:  *fidlgen.NewGenerator("GoRuntimeGoTemplates", templates, formatter, funcMap),
		asmGen: *fidlgen.NewGenerator("GoRuntimeAsmTemplates", templates, fidlgen.NewFormatter(""), funcMap),
	}
}

func (gen Generator) DeclOrder() zither.DeclOrder { return zither.SourceDeclOrder }

func (gen Generator) DeclCallback(zither.Decl) {}

func (gen *Generator) Generate(summary zither.LibrarySummary, outputDir string) ([]string, error) {
	var syscalls []zither.Syscall
	for _, summary := range summary.Files {
		for _, decl := range summary.Decls {
			if !decl.IsSyscallFamily() {
				continue
			}
			for _, syscall := range decl.AsSyscallFamily().Syscalls {
				syscalls = append(syscalls, syscall)
			}
		}
	}
	sort.Slice(syscalls, func(i, j int) bool {
		return strings.Compare(syscalls[i].Name, syscalls[j].Name) < 0
	})

	var outputs []string
	for _, file := range generatedSources {
		output := filepath.Join(outputDir, file)
		templateName := "Generate-" + filepath.Base(file)
		var err error
		if filepath.Ext(file) == ".go" {
			err = gen.goGen.GenerateFile(output, templateName, syscalls)
		} else {
			err = gen.asmGen.GenerateFile(output, templateName, syscalls)
		}
		if err != nil {
			return nil, err
		}
		outputs = append(outputs, output)
	}
	return outputs, nil
}

func ffiType(desc zither.TypeDescriptor) string {
	switch desc.Kind {
	case zither.TypeKindBool, zither.TypeKindInteger, zither.TypeKindSize:
		return golang.PrimitiveTypeName(fidlgen.PrimitiveSubtype(desc.Type))
	case zither.TypeKindEnum:
		enum := desc.Decl.(*zither.Enum)
		return golang.PrimitiveTypeName(fidlgen.PrimitiveSubtype(enum.Subtype))
	case zither.TypeKindBits:
		bits := desc.Decl.(*zither.Bits)
		return golang.PrimitiveTypeName(fidlgen.PrimitiveSubtype(bits.Subtype))
		// Structs are passed as pointers.
	case zither.TypeKindStruct, zither.TypeKindArray,
		zither.TypeKindPointer, zither.TypeKindVoidPointer:
		return "unsafe.Pointer"
	case zither.TypeKindHandle:
		return "uint32"
	case zither.TypeKindAlias:
		// TODO(https://fxbug.dev/42061412): These types are currently misdefined as
		// `uint64` aliases (instead of `uintptr` ones).
		if desc.Type == "zx/Paddr" || desc.Type == "zx/Vaddr" {
			return "uintptr"
		}
		alias := desc.Decl.(*zither.Alias)
		return ffiType(alias.Value)
	default:
		panic(fmt.Sprintf("unknown kind %q: %#v", desc.Kind, desc))
	}
}

//
// Template functions.
//

func lowerCaseWithUnderscores(el zither.Element) string {
	// Account for reserved keywords with some cheesy renaming.
	name := zither.LowerCaseWithUnderscores(el)
	switch name {
	case "type":
		return "typ"
	case "func":
		return "funk"
	case "g":
		return "g_"
	default:
		return name
	}
}

func parameterType(param zither.SyscallParameter) string {
	if passedAsPointer(param) {
		elementType := param.Type
		elementType.Mutable = !param.IsStrictInput()
		return golang.DescribeType(zither.TypeDescriptor{
			Kind:        zither.TypeKindPointer,
			ElementType: &elementType,
		})
	}
	return golang.DescribeType(param.Type)
}

func passedAsPointer(param zither.SyscallParameter) bool {
	// Structs and non-inputs should be passed as pointers.
	kind := param.Type.Kind
	return !kind.IsPointerLike() && (!param.IsStrictInput() || kind == zither.TypeKindStruct)
}

func returnType(syscall zither.Syscall) string {
	if syscall.ReturnType == nil {
		return ""
	}
	return golang.DescribeType(*syscall.ReturnType)
}

func ffiParameterType(param zither.SyscallParameter) string {
	// Non-inputs are passed by pointers.
	if !param.IsStrictInput() {
		return "unsafe.Pointer"
	}
	return ffiType(param.Type)
}

func ffiReturnType(syscall zither.Syscall) string {
	if syscall.ReturnType == nil {
		return ""
	}
	return ffiType(*syscall.ReturnType)
}

// String hash for the DT_GNU_HASH format, which uses the djb2 hash algorithm.
// This gives one the parts of the ELF dynamic linking metadata needed to look
// up names in the vDSO.
func elfGNUHash(s string) string {
	h := uint32(5381)
	for _, c := range s {
		h += (h << 5) + uint32(c)
	}
	return fmt.Sprintf("%#x", int(h))
}

type arch int

const (
	archX86 arch = iota
	archARM64
)

func x86AsmBinding(syscall zither.Syscall) []string {
	return asmBinding(syscall, archX86)
}

func armAsmBinding(syscall zither.Syscall) []string {
	return asmBinding(syscall, archARM64)
}

// See https://cs.opensource.google/go/go/+/master:src/cmd/compile/abi-internal.md
// for particularities below.
func asmBinding(syscall zither.Syscall, arch arch) []string {
	totalParamSize := 0
	for _, param := range syscall.Parameters {
		size := sizeOnStack(param)
		if size == 1 && arch == archARM64 {
			size = 8
		}
		// Pad until the parameter's natural alignment.
		totalParamSize = align(totalParamSize, size) + size
	}
	if totalParamSize%8 == 4 {
		totalParamSize += 4
	}

	retSize := 0
	if syscall.ReturnType != nil {
		retSize = syscall.ReturnType.Size
	}

	var (
		regs                              [8]string
		callIns, retReg, suffix4, suffix8 string
		frameSize                         int = 0
	)
	switch arch {
	case archX86:
		regs = [8]string{"DI", "SI", "DX", "CX", "R8", "R9", "R12", "R13"}
		callIns = "CALL"
		retReg = "AX"
		suffix4 = "L"
		suffix8 = "Q"
		frameSize = 8
		if len(syscall.Parameters) == 7 {
			frameSize += 16 + 8
		} else if len(syscall.Parameters) == 8 {
			frameSize += 16 + 2*8
		}
	case archARM64:
		regs = [8]string{"R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7"}
		callIns = "BL"
		retReg = "R0"
		suffix4 = "W"
		suffix8 = "D"
	}

	var output []string
	appendIns := func(ins ...string) {
		output = append(output, ins...)
	}

	syscallName := lowerCaseWithUnderscores(syscall)
	appendIns(
		fmt.Sprintf("TEXT runtime·vdsoCall_zx_%s(SB),NOSPLIT,$%d-%d", syscallName, frameSize, totalParamSize+retSize),
		"GO_ARGS",
		"NO_LOCAL_POINTERS",
	)

	// Set vdso{PC,SP} so that pprof tracebacks work for VDSO calls.
	switch arch {
	case archX86:
		appendIns(
			"get_tls(CX)",
			"MOVQ g(CX), AX",
			"MOVQ g_m(AX), R14",
			"PUSHQ R14",
			"LEAQ ret+0(FP), DX",
			"MOVQ -8(DX), CX",
			"MOVQ CX, m_vdsoPC(R14)",
			"MOVQ DX, m_vdsoSP(R14)",
		)
	case archARM64:
		appendIns(
			"MOVD g_m(g), R21",
			"MOVD LR, m_vdsoPC(R21)",

			// If pprof sees the SP value, it will assume the PC value is
			// written and valid. This may not be valid due to store/store
			// reordering on ARM64. This store barrier exists to ensure that
			// any observer of m->vdsoSP is also guaranteed to see m->vdsoPC.
			"DMB $0xe",
			"MOVD $ret-8(FP), R20 // caller's SP",
			"MOVD R20, m_vdsoSP(R21)",
		)
	}

	// runtime·entersyscall ensures that other goroutines and the garbage
	// collector are not blocked. However, the system will hang in the case of
	// `zx_futex_wait()` and `zx_nanosleep()`.
	blockingNonHanging := syscall.Blocking && syscallName != "futex_wait" && syscallName != "nanosleep"
	if blockingNonHanging {
		appendIns("CALL runtime·entersyscall(SB)")
	}

	offset := 0
	for i, param := range syscall.Parameters {
		suffix := suffix8
		size := sizeOnStack(param)
		if size == 4 {
			suffix = suffix4
		} else if size == 1 && arch == archARM64 {
			size = 8 // As above.
		}
		offset = align(offset, size)
		name := lowerCaseWithUnderscores(param)
		appendIns(fmt.Sprintf("MOV%s %s+%d(FP), %s", suffix, name, offset, regs[i]))
		offset += size
	}

	switch arch {
	case archX86:
		if len(syscall.Parameters) >= 7 {
			appendIns(
				"MOVQ SP, BP   // BP is preserved across vsdo call by the x86-64 ABI",
				"ANDQ $~15, SP // stack alignment for x86-64 ABI",
			)
			if len(syscall.Parameters) == 8 {
				appendIns("PUSHQ R13")
			}
			appendIns("PUSHQ R12")
		}
		appendIns(
			fmt.Sprintf("MOVQ vdso_zx_%s(SB), AX", syscallName),
			"CALL AX",
		)
		if len(syscall.Parameters) >= 7 {
			appendIns("POPQ R12")
			if len(syscall.Parameters) == 8 {
				appendIns("POPQ R13")
			}
			appendIns("MOVQ BP, SP")
		}
	case archARM64:
		appendIns(fmt.Sprintf("BL vdso_zx_%s(SB)", syscallName))
	}

	if retSize > 0 {
		suffix := suffix8
		if retSize == 4 {
			suffix = suffix4
		}
		appendIns(fmt.Sprintf("MOV%s %s, ret+%d(FP)", suffix, retReg, totalParamSize))
	}

	if blockingNonHanging {
		appendIns(fmt.Sprintf("%s runtime·exitsyscall(SB)", callIns))
	}

	// Clear vdsoSP. sigprof only checks vdsoSP for generating tracebacks, so
	// we can leave vdsoPC alone.
	switch arch {
	case archX86:
		appendIns(
			"POPQ R14",
			"MOVQ $0, m_vdsoSP(R14)",
		)
	case archARM64:
		appendIns(
			"MOVD g_m(g), R21",
			"MOVD $0, m_vdsoSP(R21)",
		)
	}

	appendIns("RET")
	return output
}

func align(a, b int) int {
	// The numbers are small enough that overflow is not a remote possibility.
	return ((a + b - 1) / b) * b
}

func sizeOnStack(param zither.SyscallParameter) int {
	if passedAsPointer(param) {
		return 8
	}
	return param.Type.Size
}
