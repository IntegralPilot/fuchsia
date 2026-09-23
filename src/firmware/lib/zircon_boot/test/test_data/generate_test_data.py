#!/usr/bin/env fuchsia-vendored-python

# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""The code is to provide a reference of how test_images.h is generated"""

import hashlib
import os
import pathlib
import struct
import subprocess
import tempfile
from typing import List

MY_DIR = os.path.dirname(__file__)
FUCHSIA_DIR = os.environ.get("FUCHSIA_DIR")
AVB_REPO = os.path.join(
    f"{FUCHSIA_DIR}", "third_party", "android", "platform", "external", "avb"
)
AVB_TOOL = os.path.join(f"{AVB_REPO}", "avbtool.py")
ZBI_TOOL = os.path.join(f"{FUCHSIA_DIR}", "out", "default", "host_x64", "zbi")
PSK = os.path.join(f"{AVB_REPO}", "test", "data", "testkey_atx_psk.pem")
PUK = os.path.join(f"{AVB_REPO}", "test", "data", "testkey_atx_puk.pem")
UNLOCK_CHALLENGE_RANDOM_DATA = os.path.join(
    f"{AVB_REPO}", "test", "data", "atx_unlock_challenge.bin"
)
UNLOCK_CREDENTIAL = os.path.join(
    f"{AVB_REPO}", "test", "data", "atx_unlock_credential.bin"
)
PRODUCT_ID = os.path.join(f"{AVB_REPO}", "test", "data", "atx_product_id.bin")
ATX_METADATA = os.path.join(f"{AVB_REPO}", "test", "data", "atx_metadata.bin")
ATX_PERMANENT_ATTRIBUTES = os.path.join(
    f"{AVB_REPO}", "test", "data", "atx_permanent_attributes.bin"
)

KERNEL_IMG_SIZE = 1024


def GetCArrayDeclaration(data: bytes, var_name: str) -> str:
    """Generates a C array declaration for a byte array"""
    decl = f"const uint8_t {var_name}[] =" + "{ "
    decl += " ".join([f"0x{b:x}," for b in data])
    decl += "};"
    return decl


def GenerateTestImageCArrayDeclaration(
    ab_suffix: str, rollback_index: int = 0, rollback_index_location: int = 0
) -> List[str]:
    """Generates kernel/vbmeta image for a slot and their C array declarations"""
    decls = []
    with tempfile.TemporaryDirectory() as temp_dir:
        temp_dir = pathlib.Path(temp_dir)
        kernel = temp_dir / f"test_zircon_{ab_suffix}.bin"
        kernel_zbi = temp_dir / f"test_zircon_{ab_suffix}.zbi"

        # Generate random kernel image
        kernel.write_bytes(bytes(os.urandom(KERNEL_IMG_SIZE)))
        # Put image in a zbi container.
        subprocess.run(
            [
                ZBI_TOOL,
                "--output",
                kernel_zbi,
                "--type=KERNEL_ARM64",
                kernel,
            ]
        )

        # Set reserved scratch memory size to 0. Otherwise it's a random (huge) number
        kernel_zbi_bytes = bytearray(kernel_zbi.read_bytes())
        # See `ZbiKernelImage` defined in <phys/zbi.h>.
        # 2*zbi_header_t + uint64_t = 2 * 32 + 8
        kernel_zbi_bytes[72:80] = b"\x00" * 8
        kernel_zbi.write_bytes(kernel_zbi_bytes)

        # Generate C array declaration
        with open(kernel_zbi, "rb") as zbi:
            data = zbi.read()
            decls.append(
                GetCArrayDeclaration(
                    data, f"kTestZircon{ab_suffix.capitalize()}Image"
                )
            )
        # Generate vbmeta descriptor.
        vbmeta_desc = f"{temp_dir}/test_vbmeta_{ab_suffix}.desc"
        subprocess.run(
            [
                AVB_TOOL,
                "add_hash_footer",
                "--image",
                kernel_zbi,
                "--partition_name",
                "zircon",
                "--do_not_append_vbmeta_image",
                "--output_vbmeta_image",
                vbmeta_desc,
                "--partition_size",
                "209715200",
            ]
        )
        # Generate two cmdline ZBI items to add as property descriptors to vbmeta
        # image for test.
        vbmeta_prop_zbi_1 = f"{temp_dir}/test_vbmeta_prop_{ab_suffix}_1.img"
        subprocess.run(
            [
                ZBI_TOOL,
                "--output",
                vbmeta_prop_zbi_1,
                "--type=CMDLINE",
                f"--entry=vb_arg_1=foo_{ab_suffix}",
            ]
        )
        vbmeta_prop_zbi_2 = f"{temp_dir}/test_vbmeta_prop_{ab_suffix}_2.img"
        subprocess.run(
            [
                ZBI_TOOL,
                "--output",
                vbmeta_prop_zbi_2,
                "--type=CMDLINE",
                f"--entry=vb_arg_2=bar_{ab_suffix}",
            ]
        )
        # Generate vbmeta image
        vbmeta_img = f"{temp_dir}/test_vbmeta_{ab_suffix}.img"
        subprocess.run(
            [
                AVB_TOOL,
                "make_vbmeta_image",
                "--output",
                vbmeta_img,
                "--key",
                PSK,
                "--algorithm",
                "SHA512_RSA4096",
                "--public_key_metadata",
                ATX_METADATA,
                "--include_descriptors_from_image",
                vbmeta_desc,
                "--prop_from_file",
                f"zbi_vbmeta_arg_1:{vbmeta_prop_zbi_1}",
                "--prop_from_file",
                f"zbi_vbmeta_arg_2:{vbmeta_prop_zbi_2}",
                # The following should not be recognized as vbmeta zbi item, as
                # the value is not a zbi item.
                "--prop",
                f"zbi_vbmeta_arg_3:abc",
                "--rollback_index",
                f"{rollback_index}",
                "--rollback_index_location",
                f"{rollback_index_location}",
            ]
        )
        # Generate C array declaration
        with open(vbmeta_img, "rb") as vbmeta:
            data = vbmeta.read()
            decls.append(
                GetCArrayDeclaration(
                    data, f"kTestVbmeta{ab_suffix.capitalize()}Image"
                )
            )
    return decls


def GenerateBinaryFileDeclaration(file_name: str, var_name: str) -> str:
    with open(file_name, "rb") as bin_file:
        data = bin_file.read()
        return GetCArrayDeclaration(data, var_name)


def GenerateVariableDeclarationHeader(decls: List[str], out_file: str):
    """Writes a header file that contains all C array declarations"""
    with open(out_file, "w") as f:
        f.write(
            """// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

        """
        )
        # This will trigger 'fx format-code' to fix inclusion macro.
        f.write("#pragma once\n\n")
        for decl in decls:
            f.write(f"{decl}\n\n")
    subprocess.run(["fx", "format-code", f"--files={out_file}"])


def GenerateTestGptDeclaration() -> str:
    # Generate a realistic GPT disk for testing firmware sdk and reference code
    with tempfile.TemporaryDirectory() as temp_dir:
        filepath = pathlib.Path(os.path.join(temp_dir, "gpt.bin"))
        partitions = [
            # (name, size Kb)
            ("zircon_a", 2),
            ("zircon_b", 2),
            ("zircon_r", 2),
            ("vbmeta_a", 6),
            ("vbmeta_b", 6),
            ("vbmeta_r", 6),
            ("durable_boot", 1),
        ]
        total_size = (1 + 33 * 2) * 512  # MBR + (gpt header + entries) * 2
        args = ["sgdisk", filepath, "--clear", "--set-alignment", "512"]
        for i, (name, size_kb) in enumerate(partitions, start=1):
            args += [
                "--new",
                f"{i}:+0:+{size_kb}K",
                "--change-name",
                f"{i}:{name}",
            ]
            total_size += size_kb * 1024
        filepath.write_bytes(b"\x00" * total_size)
        ret = subprocess.run(args, check=True, text=True)
        return GetCArrayDeclaration(filepath.read_bytes(), "kTestGptDisk")


def GenerateTestUnlockChallenge() -> bytes:
    """Generate a test unlock challenge

    The test unlock challenge is generated using the test keys and
    pre-generated fixed random data.
    """
    # An unlock challenge is version + product id hash + 16 byte random data
    ret = struct.pack("<I", 1)
    with open(PRODUCT_ID, "rb") as product_id:
        ret += hashlib.sha256(product_id.read()).digest()

    with open(UNLOCK_CHALLENGE_RANDOM_DATA, "rb") as random_data:
        ret += random_data.read()

    return ret


if __name__ == "__main__":
    decls = []
    # slot images
    decls.extend(GenerateTestImageCArrayDeclaration("a", rollback_index=5))
    decls.extend(GenerateTestImageCArrayDeclaration("b", rollback_index=10))
    decls.extend(GenerateTestImageCArrayDeclaration("r"))
    decls.extend(
        GenerateTestImageCArrayDeclaration("slotless", rollback_index=2)
    )
    # permanent attributes
    decls.append(
        GenerateBinaryFileDeclaration(
            ATX_PERMANENT_ATTRIBUTES, "kPermanentAttributes"
        )
    )
    decls.append(
        GenerateBinaryFileDeclaration(
            UNLOCK_CHALLENGE_RANDOM_DATA, "kUnlockChallengeRandom"
        )
    )
    decls.append(
        GenerateBinaryFileDeclaration(UNLOCK_CREDENTIAL, "kUnlockCredential")
    )
    decls.append(
        GetCArrayDeclaration(
            GenerateTestUnlockChallenge(), "kTestUnlockChallenge"
        )
    )
    decls.append(GenerateTestGptDeclaration())
    GenerateVariableDeclarationHeader(
        decls, os.path.join(MY_DIR, "test_images.h")
    )
