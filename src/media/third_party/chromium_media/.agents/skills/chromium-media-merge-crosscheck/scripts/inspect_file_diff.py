#!/usr/bin/env fuchsia-vendored-python
import sys, os, subprocess

def get_fuchsia_root():
    try:
        res = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
        return res.stdout.strip()
    except Exception:
        return "/usr/local/google/home/dustingreen/fuchsia2"

FUCHSIA_ROOT = get_fuchsia_root()
CHROMIUM_MEDIA = os.path.join(FUCHSIA_ROOT, "src/media/third_party/chromium_media")
CHROMIUM_SRC = os.path.join(FUCHSIA_ROOT, "local_src/chromium/src")

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
    if len(sys.argv) < 2:
        print("Usage: inspect_file_diff.py <rel_path> [--ignore-ws]")
        sys.exit(1)

    rel_path = sys.argv[1]
    fuchsia_file = os.path.join(CHROMIUM_MEDIA, rel_path)
    upstream_rel = resolve_upstream_path(rel_path)
    upstream_file = os.path.join(CHROMIUM_SRC, upstream_rel)

    print(f"=== FUCHSIA:  {fuchsia_file}")
    print(f"=== UPSTREAM: {upstream_file}")

    cmd = ["diff", "-u", fuchsia_file, upstream_file]
    if "--ignore-ws" in sys.argv or "-w" in sys.argv:
        cmd.insert(2, "-w")

    res = subprocess.run(cmd, capture_output=True, text=True)
    print(res.stdout)
