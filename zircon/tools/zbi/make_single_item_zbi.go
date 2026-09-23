// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"path"
	"unsafe"

	"zbi"
)

var flags = struct {
	ItemPath    string
	OutputPath  string
	DepfilePath string
}{}

func init() {
	flag.StringVar(&flags.ItemPath, "item-path", "",
		"Path to file containing a JSON representation of the item, including: \n"+
			" * `type`(string ZBI_TYPE...)\n"+
			" * `extra`(optional string ZBI_KERNEL_DRIVER....)\n"+
			" * `contents`(JSON representation of the payload)")
	flag.StringVar(&flags.OutputPath, "output", "",
		"Path where the item, header and payload will be written to. Item is prefixed with "+
			"container header so it may be combined with other ZBI entries.")
	flag.StringVar(&flags.DepfilePath, "depfile", "", "Depfile for generated output file.")
}

func usage() {
	msg := `Usage:  ` + path.Base(os.Args[0]) + ` [flags]

ZBI item generator, used for build time generation of custom items being injected into the ZBI.
JSON representation of objects are used to generate the payload.

Flags:
`
	fmt.Fprint(flag.CommandLine.Output(), msg)
	flag.PrintDefaults()
}

func ItemHeader(zbiType zbi.Type, zbiExtra uint32, payloadLength uint32) zbi.Header {
	return zbi.Header{
		Type:   zbiType,
		Length: payloadLength,
		Extra:  zbiExtra,
		Flags:  zbi.FlagsVersion,
		Magic:  zbi.ItemMagic,
		Crc32:  zbi.ItemNoCrc32,
	}
}

func ContainerHeader(payloadLength uint32) zbi.Header {
	return zbi.Header{
		Type:   zbi.TypeContainer,
		Length: payloadLength,
		Extra:  zbi.ContainerMagic,
		Flags:  zbi.FlagsVersion,
		Magic:  zbi.ItemMagic,
		Crc32:  zbi.ItemNoCrc32,
	}
}

// Any ZBI item generator must fulfill this contract.
type ZbiItemGenerator = func(item Item) (Type zbi.Type, Extra uint32, Payload []byte, Error error)

// JSON content struct.
type Item = struct {
	Type     string          `json:"type"`
	Extra    string          `json:"extra"`
	Contents json.RawMessage `json:"contents"`
}

func GenerateItem() error {
	itemFile, err := os.ReadFile(flags.ItemPath)
	if err != nil {
		return err
	}

	var item Item
	if err = json.Unmarshal(itemFile, &item); err != nil {
		return err
	}

	var zbiType zbi.Type
	var zbiExtra uint32
	var payload []byte

	// New ZBI items should be added here.
	switch item.Type {
	case "ZBI_TYPE_KERNEL_DRIVER":
		zbiType = zbi.TypeKernelDriver
		switch item.Extra {
		case "ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND":
			var suspendStates []zbi.DcfgArmPsciCpuSuspendState
			zbiExtra = uint32(zbi.KernelDriverArmPsciCpuSuspend)
			payload, err = GenerateArrayPayload(suspendStates, []byte(item.Contents))
			break
		default:
			return fmt.Errorf("Unsupported Kernel Driver: %s", item.Extra)
		}
		break
	default:
		return fmt.Errorf("Unsupported item type: %s", item.Type)
	}

	if err != nil {
		return err
	}

	payloadLength := uint32(0)
	if payload != nil {
		payloadLength = uint32(len(payload))
	}

	itemHeader := ItemHeader(zbiType, zbiExtra, payloadLength)
	containerHeader := ContainerHeader(payloadLength + uint32(unsafe.Sizeof(itemHeader)))

	// Write file contents.
	zbiFile, err := os.Create(flags.OutputPath)

	if err != nil {
		return err
	}

	defer zbiFile.Close()
	if err := binary.Write(zbiFile, binary.LittleEndian, &containerHeader); err != nil {
		return fmt.Errorf("Failed to write container header: %v", err)
	}

	if err := binary.Write(zbiFile, binary.LittleEndian, &itemHeader); err != nil {
		return fmt.Errorf("Failed to write item header: %v", err)
	}

	if payloadLength > 0 {
		if err := binary.Write(zbiFile, binary.LittleEndian, payload); err != nil {
			return fmt.Errorf("Failed to write item header: %v", err)
		}
	}

	// Now depfile
	depFile, err := os.Create(flags.DepfilePath)
	if err != nil {
		return fmt.Errorf("Failed to create depfile (%s): %v", flags.DepfilePath, err)
	}

	defer depFile.Close()
	_, err = fmt.Fprintf(depFile, "%s: %s\n", flags.OutputPath, flags.ItemPath)
	if err != nil {
		return fmt.Errorf("Failed to write depfile (%s) contents: %v", flags.DepfilePath, err)
	}

	return nil
}

func GenerateArrayPayload[T any](payload []T, jsonContents []byte) ([]byte, error) {
	if err := json.Unmarshal(jsonContents, &payload); err != nil {
		return nil, err
	}

	// So we can determine the size of each element.
	var element T
	elementSize := int(unsafe.Sizeof(element))

	payloadSize := elementSize * len(payload)
	buffer := bytes.NewBuffer(make([]byte, 0, payloadSize))

	for _, payloadItem := range payload {
		if err := binary.Write(buffer, binary.LittleEndian, &payloadItem); err != nil {
			return nil, fmt.Errorf("Failed to generate binary payload: %v", err)
		}
	}

	return buffer.Bytes(), nil
}

func main() {
	flag.Parse()

	if err := GenerateItem(); err != nil {
		log.Fatalf("Failed to generate item: %v. Flags: %+v", err, flags)
	}
}
