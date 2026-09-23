#!/usr/bin/env fuchsia-vendored-python
import os
import subprocess

def get_fuchsia_root():
    try:
        res = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
        return res.stdout.strip()
    except Exception:
        return "/usr/local/google/home/dustingreen/fuchsia2"

FUCHSIA_ROOT = get_fuchsia_root()
CHROMIUM_MEDIA = os.path.join(FUCHSIA_ROOT, "src/media/third_party/chromium_media")
CHROMIUM_SRC = os.path.join(FUCHSIA_ROOT, "local_src/chromium/src")

COMPONENTS = {
    "Component 1: Bitstream Parsers": [
        "media/parsers/bit_reader_macros.h",
        "media/parsers/h264_bit_reader.cc",
        "media/parsers/h264_bit_reader.h",
        "media/parsers/h264_level_limits.cc",
        "media/parsers/h264_level_limits.h",
        "media/parsers/h264_parser.cc",
        "media/parsers/h264_parser.h",
        "media/parsers/h264_poc.cc",
        "media/parsers/h264_poc.h",
        "media/parsers/h26x_parser.h",
        "media/parsers/jpeg_parser.cc",
        "media/parsers/jpeg_parser.h",
        "media/parsers/vp9_parser.cc",
        "media/parsers/vp9_parser.h",
        "media/parsers/vp9_raw_bits_reader.cc",
        "media/parsers/vp9_raw_bits_reader.h",
        "media/parsers/vp9_uncompressed_header_parser.cc",
        "media/parsers/vp9_uncompressed_header_parser.h",
    ],
    "Component 2: Hardware Decoders & DPB Management": [
        "media/gpu/accelerated_video_decoder.h",
        "media/gpu/codec_picture.h",
        "media/gpu/gpu_video_encode_accelerator_helpers.cc",
        "media/gpu/gpu_video_encode_accelerator_helpers.h",
        "media/gpu/h264_decoder.cc",
        "media/gpu/h264_decoder.h",
        "media/gpu/h264_dpb.cc",
        "media/gpu/h264_dpb.h",
        "media/gpu/vp9_decoder.cc",
        "media/gpu/vp9_decoder.h",
        "media/gpu/vp9_picture.cc",
        "media/gpu/vp9_picture.h",
        "media/gpu/vp9_reference_frame_vector.cc",
        "media/gpu/vp9_reference_frame_vector.h",
        "media/filters/h264_bitstream_buffer.cc",
        "media/filters/h264_bitstream_buffer.h",
    ],
    "Component 3: Base Utilities & Core Data Structures": [
        "media/base/bit_reader.cc",
        "media/base/bit_reader.h",
        "media/base/bitrate.cc",
        "media/base/bitrate.h",
        "media/base/decoder_buffer.h",
        "media/base/decrypt_config.h",
        "media/base/ranges.cc",
        "media/base/ranges.h",
        "media/base/subsample_entry.cc",
        "media/base/subsample_entry.h",
        "media/base/video_bitrate_allocation.cc",
        "media/base/video_bitrate_allocation.h",
        "media/base/video_codecs.cc",
        "media/base/video_codecs.h",
        "media/base/video_color_space.cc",
        "media/base/video_color_space.h",
        "chromium_utils.h",
        "geometry.h",
        "time_delta.h",
    ],
    "Component 4: Video Encoding": [
        "media/video/video_encode_accelerator.cc",
        "media/video/video_encode_accelerator.h",
    ]
}

def resolve_upstream_path(rel_path):
    u_path = os.path.join(CHROMIUM_SRC, rel_path)
    if os.path.exists(u_path):
        return rel_path
    filename = os.path.basename(rel_path)
    res = subprocess.run(
        ["git", "-C", CHROMIUM_SRC, "ls-files", f"*{filename}"],
        capture_output=True, text=True
    )
    matches = [l.strip() for l in res.stdout.split() if "media/" in l or "ui/gfx/" in l or "base/" in l]
    return matches[0] if matches else rel_path

if __name__ == "__main__":
    summary = []
    for comp_name, files in COMPONENTS.items():
        summary.append(f"=== {comp_name} ===")
        for rel_path in files:
            fuchsia_file = os.path.join(CHROMIUM_MEDIA, rel_path)
            if not os.path.exists(fuchsia_file):
                summary.append(f"  [MISSING IN FUCHSIA] {rel_path}")
                continue

            upstream_rel = resolve_upstream_path(rel_path)
            upstream_file = os.path.join(CHROMIUM_SRC, upstream_rel)
            if not os.path.exists(upstream_file):
                summary.append(f"  [MISSING UPSTREAM] {rel_path} -> {upstream_rel}")
                continue

            res_w = subprocess.run(
                ["diff", "-u", "-w", fuchsia_file, upstream_file],
                capture_output=True, text=True
            )
            res_full = subprocess.run(
                ["diff", "-u", fuchsia_file, upstream_file],
                capture_output=True, text=True
            )

            lines_w = res_w.stdout.count('\n')
            lines_full = res_full.stdout.count('\n')

            if lines_full == 0:
                status = "PERFECT PARITY (100% identical)"
            elif lines_w == 0:
                status = f"WHITESPACE ONLY ({lines_full} diff lines)"
            else:
                status = f"LOGICAL DIFF ({lines_w} non-ws diff lines, {lines_full} total diff lines)"

            summary.append(f"  {rel_path} -> {upstream_rel}: {status}")

    print("\n".join(summary))
