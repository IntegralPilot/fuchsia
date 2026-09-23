---
name: chromium-media-merge-crosscheck
description: Audit, cross-check, and reconcile Fuchsia's chromium_media codebase against upstream Chromium (local_src/chromium/src) to ensure logical, security, and structural parity.
---

# Chromium Media Merge Cross-Check Skill

## Overview

Use this skill when performing a systematic audit, cross-check, or reconciliation of Fuchsia's maintained `chromium_media` codebase (`src/media/third_party/chromium_media/`) against upstream Chromium (`local_src/chromium/src/`).

This process ensures that:
1. Critical security bounds checks, integer overflow protections (`base::CheckedNumeric`), and parsing fixes present in upstream Chromium are incorporated into Fuchsia.
2. Intentional Fuchsia platform and hardware decoder extensions (e.g. pure C++ JPEG parser, NALU injection, secure surface allocation) are strictly preserved.
3. Fuchsia-style header include guards (e.g. `SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_...` or full repository path guards) are strictly preserved.
4. Whitespace and formatting differences on identical code tokens are aligned with upstream Chromium to minimize diff noise without using broad auto-formatters (`fx format-code`).

---

## Component Inventory

Maintained files in `src/media/third_party/chromium_media/` are grouped into four logical components:

### Component 1: Bitstream Parsers (`media/parsers/`)
- `media/parsers/bit_reader_macros.h`
- `media/parsers/h264_bit_reader.cc`, `media/parsers/h264_bit_reader.h`
- `media/parsers/h264_level_limits.cc`, `media/parsers/h264_level_limits.h`
- `media/parsers/h264_parser.cc`, `media/parsers/h264_parser.h`
- `media/parsers/h264_poc.cc`, `media/parsers/h264_poc.h`
- `media/parsers/h26x_parser.h`
- `media/parsers/jpeg_parser.cc`, `media/parsers/jpeg_parser.h` *(Note: Fuchsia intentionally retains C++ parser vs Chromium Rust FFI)*
- `media/parsers/vp9_parser.cc`, `media/parsers/vp9_parser.h`
- `media/parsers/vp9_raw_bits_reader.cc`, `media/parsers/vp9_raw_bits_reader.h`
- `media/parsers/vp9_uncompressed_header_parser.cc`, `media/parsers/vp9_uncompressed_header_parser.h`

### Component 2: Hardware Decoders & DPB Management (`media/gpu/` & `media/filters/`)
- `media/gpu/accelerated_video_decoder.h`
- `media/gpu/codec_picture.h`
- `media/gpu/gpu_video_encode_accelerator_helpers.cc`, `media/gpu/gpu_video_encode_accelerator_helpers.h`
- `media/gpu/h264_decoder.cc`, `media/gpu/h264_decoder.h`
- `media/gpu/h264_dpb.cc`, `media/gpu/h264_dpb.h`
- `media/gpu/vp9_decoder.cc`, `media/gpu/vp9_decoder.h`
- `media/gpu/vp9_picture.cc`, `media/gpu/vp9_picture.h`
- `media/gpu/vp9_reference_frame_vector.cc`, `media/gpu/vp9_reference_frame_vector.h`
- `media/filters/h264_bitstream_buffer.cc`, `media/filters/h264_bitstream_buffer.h`

### Component 3: Base Utilities & Core Data Structures (`media/base/` & root)
- `media/base/bit_reader.cc`, `media/base/bit_reader.h`
- `media/base/bitrate.cc`, `media/base/bitrate.h`
- `media/base/decoder_buffer.h`
- `media/base/decrypt_config.h`
- `media/base/ranges.cc`, `media/base/ranges.h`
- `media/base/subsample_entry.cc`, `media/base/subsample_entry.h`
- `media/base/video_bitrate_allocation.cc`, `media/base/video_bitrate_allocation.h`
- `media/base/video_codecs.cc`, `media/base/video_codecs.h`
- `media/base/video_color_space.cc`, `media/base/video_color_space.h`
- `chromium_utils.h`, `geometry.h`, `time_delta.h`

### Component 4: Video Encoding (`media/video/`)
- `media/video/video_encode_accelerator.cc`, `media/video/video_encode_accelerator.h`

---

## Step-by-Step Workflow

### Step 1: Resolve File Paths Across Repositories
Because Chromium upstream renames or moves files over time, resolve file paths dynamically using git lookup:

```python
import os, subprocess

def resolve_upstream_path(rel_path, chromium_src_dir):
    u_path = os.path.join(chromium_src_dir, rel_path)
    if os.path.exists(u_path):
        return rel_path
    filename = os.path.basename(rel_path)
    res = subprocess.run(
        ["git", "-C", chromium_src_dir, "ls-files", f"*{filename}"],
        capture_output=True, text=True
    )
    matches = [l.strip() for l in res.stdout.split() if "media/" in l]
    return matches[0] if matches else rel_path
```

### Step 2: Component-by-Component Diff Audit
Inspect unified diffs between Fuchsia's maintained files and upstream ToT using the helper script in `scripts/inspect_file_diff.py`:
```bash
fuchsia-vendored-python src/media/third_party/chromium_media/.agents/skills/chromium-media-merge-crosscheck/scripts/inspect_file_diff.py <rel_path> [--ignore-ws]
```

#### Audit Categories & Integration Guidelines
When reviewing diffs between Fuchsia and upstream Chromium across maintained files, evaluate changes for inclusion according to the following categories:

1. **Security & Memory Hardening Fixes**:
   - Out-of-bounds checks, input range validations, integer overflow protections (e.g. `base::CheckedNumeric`), pointer safety, and uninitialized variable fixes.
2. **Parser Spec Compliance & Bitstream Correctness**:
   - Bitstream spec compliance updates, accurate bit-size calculations (e.g. accounting for emulation prevention bytes), error recovery improvements, and invalid stream handling.
3. **Non-Damaging Logic Updates & Bug Fixes**:
   - Functional bug fixes, edge-case handle updates, parameter boundary corrections, and state machine stabilization that preserve hardware decoding capability.
4. **Code Simplifications & Idiomatic Modernizations**:
   - C++ standard library adoption (e.g. `std::move`, `std::ranges`, `std::span`), cleanups of redundant code, and removal of deprecated abstractions, provided they do not break Fuchsia's build system or standard libraries.
5. **Safe Refactorings**:
   - Internal helper refactoring and structural cleanups that improve maintainability without disrupting Fuchsia-specific API surfaces or hardware decoder integrations (such as NALU injection or secure surface allocation).

### Step 3: Targeted Whitespace & Formatting Alignment
To prevent diff pollution while avoiding broad formatters (do **NOT** run `fx format-code` during cross-checks), use targeted chunk-level alignment:
- Compare Fuchsia blocks against upstream Chromium blocks.
- If removing all whitespace makes two blocks 100% identical, adopt Chromium's exact line wrapping and indentation in Fuchsia.

### Step 4: Build Verification
Always verify changes compile cleanly using Fuchsia's hardware decoder targets:
```bash
fx build //src/media/drivers/amlogic_decoder:amlogic_decoder //src/media/codec/codecs/vaapi/test:vaapi_tests_package
```
