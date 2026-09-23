# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import fcntl
import os
import re
import select
import shutil
import subprocess
import sys
import tempfile
import time

from PIL import Image, ImageDraw, ImageFont


class Terminal:
    def __init__(
        self,
        width=800,
        height=450,
        font_path="/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        font_size=14,
    ):
        self.width = width
        self.height = height
        self.font = ImageFont.truetype(font_path, font_size)
        bold_font_path = font_path.replace(
            "DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf"
        )
        self.font_bold = ImageFont.truetype(bold_font_path, font_size)
        self.lines = [""]
        self.line_height = font_size + 6
        self.padding = 15
        self.max_lines = max(1, (height - 2 * self.padding) // self.line_height)
        self.bg_color = (30, 30, 30)  # #1e1e1e
        self.fg_color = (212, 212, 212)  # #d4d4d4
        self.colors = {
            "g": (78, 201, 176),  # Green/Teal
            "b": (86, 156, 214),  # Blue
            "y": (206, 145, 120),  # Yellow/Orange
            "m": (197, 134, 192),  # Magenta
            "c": (156, 220, 254),  # Cyan
            "r": (244, 71, 71),  # Red
        }
        self.prompt = "\x1b[32m❯\x1b[0m "
        self.in_escape = False
        self.escape_buf = ""

    def add_line(self, line):
        self.lines.append(line)
        if len(self.lines) > self.max_lines:
            self.lines.pop(0)

    def write(self, text):
        for char in text:
            if self.in_escape:
                self.escape_buf += char
                if char == "m":
                    self.in_escape = False
                    if self.lines:
                        self.lines[-1] += self.escape_buf
                    self.escape_buf = ""
                elif len(self.escape_buf) > 20:  # Safety fallback
                    self.in_escape = False
                    if self.lines:
                        self.lines[-1] += self.escape_buf
                    self.escape_buf = ""
            elif char == "\x1b":
                self.in_escape = True
                self.escape_buf = "\x1b"
            elif char == "\n":
                self.add_line("")
            elif char == "\r":
                if self.lines:
                    self.lines[-1] = ""
            else:
                if not self.lines:
                    self.lines.append("")
                self.lines[-1] += char

    def strip_tags(self, line):
        return re.sub(r"\x1b\[[0-9;]*m", "", line)

    def draw(self, show_cursor=True):
        img = Image.new("RGB", (self.width, self.height), color=self.bg_color)
        draw = ImageDraw.Draw(img)

        for i, line in enumerate(self.lines):
            y = self.padding + i * self.line_height
            self.draw_line(draw, line, self.padding, y)

        # Draw cursor at the end of the last line
        if show_cursor and self.lines:
            last_line = self.lines[-1]
            raw_text = self.strip_tags(last_line)
            x = self.padding + draw.textlength(raw_text, font=self.font)
            y = self.padding + (len(self.lines) - 1) * self.line_height
            draw.rectangle(
                [x + 2, y + 4, x + 10, y + self.line_height - 2],
                fill=self.fg_color,
            )

        return img

    def draw_line(self, draw, line, x, y):
        parts = re.split(r"(\x1b\[[0-9;]*m)", line)
        current_color = self.fg_color
        current_font = self.font
        current_x = x
        for part in parts:
            if not part:
                continue
            if part.startswith("\x1b["):
                code = part[2:-1]
                if code == "0":
                    current_color = self.fg_color
                    current_font = self.font
                elif code == "1":
                    current_font = self.font_bold
                elif code in ("32", "92"):
                    current_color = self.colors.get("g", self.fg_color)
                elif code in ("33", "93"):
                    current_color = self.colors.get("y", self.fg_color)
                elif code in ("34", "94"):
                    current_color = self.colors.get("b", self.fg_color)
                elif code in ("35", "95"):
                    current_color = self.colors.get("m", self.fg_color)
                elif code in ("36", "96"):
                    current_color = self.colors.get("c", self.fg_color)
                elif code in ("31", "91"):
                    current_color = self.colors.get("r", self.fg_color)
            else:
                draw.text(
                    (current_x, y), part, font=current_font, fill=current_color
                )
                current_x += draw.textlength(part, font=current_font)


class MultiplexedTerminal:
    def __init__(
        self,
        width=850,
        height=450,
        font_path="/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        font_size=12,
    ):
        self.width = width
        self.height = height
        self.font_path = font_path
        self.font_size = font_size
        self.split = False
        self.active_pane = 1

        self.bg_color = (30, 30, 30)
        self.border_inactive_color = (60, 60, 60)
        self.border_active_color = (0, 122, 204)

        self.term_full = Terminal(
            width=width, height=height, font_path=font_path, font_size=font_size
        )

        self.border_width = 2
        self.pane_height = height // 2 - 3
        self.inner_width = width - 2 * self.border_width
        self.inner_height = self.pane_height - 2 * self.border_width

        self.term_left = Terminal(
            width=self.inner_width,
            height=self.inner_height,
            font_path=font_path,
            font_size=font_size,
        )
        self.term_right = Terminal(
            width=self.inner_width,
            height=self.inner_height,
            font_path=font_path,
            font_size=font_size,
        )

    def perform_split(self):
        self.split = True
        self.term_left.lines = list(self.term_full.lines)
        if len(self.term_left.lines) > self.term_left.max_lines:
            self.term_left.lines = self.term_left.lines[
                -self.term_left.max_lines :
            ]
        self.term_right.lines = [""]

    def perform_merge(self):
        self.split = False
        self.term_full.lines = list(self.term_left.lines)
        if len(self.term_full.lines) > self.term_full.max_lines:
            self.term_full.lines = self.term_full.lines[
                -self.term_full.max_lines :
            ]

    def draw(self, show_cursor=True):
        if not self.split:
            return self.term_full.draw(show_cursor=show_cursor)

        img = Image.new("RGB", (self.width, self.height), color=self.bg_color)
        draw = ImageDraw.Draw(img)

        img_left = self.term_left.draw(
            show_cursor=(show_cursor and self.active_pane == 1)
        )
        img.paste(img_left, (self.border_width, self.border_width))

        img_right = self.term_right.draw(
            show_cursor=(show_cursor and self.active_pane == 2)
        )
        img.paste(
            img_right,
            (self.border_width, self.height // 2 + 1 + self.border_width),
        )

        top_border_color = (
            self.border_active_color
            if self.active_pane == 1
            else self.border_inactive_color
        )
        draw.rectangle(
            [0, 0, self.width - 1, self.pane_height],
            outline=top_border_color,
            width=self.border_width,
        )

        bottom_border_color = (
            self.border_active_color
            if self.active_pane == 2
            else self.border_inactive_color
        )
        draw.rectangle(
            [0, self.height // 2 + 1, self.width - 1, self.height - 1],
            outline=bottom_border_color,
            width=self.border_width,
        )

        return img


def setup_mock_workspace():
    test_dir = tempfile.mkdtemp(prefix="fx-worktree-demo-")
    fuchsia_dir = os.path.join(test_dir, "fuchsia")
    os.makedirs(fuchsia_dir)

    subprocess.run(
        ["git", "init", "-q", "--initial-branch=main"],
        cwd=fuchsia_dir,
        check=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Test"], cwd=fuchsia_dir, check=True
    )
    subprocess.run(
        ["git", "config", "user.email", "test@test.com"],
        cwd=fuchsia_dir,
        check=True,
    )

    with open(os.path.join(fuchsia_dir, "dummy"), "w") as f:
        f.write("hello")
    subprocess.run(["git", "add", "dummy"], cwd=fuchsia_dir, check=True)
    subprocess.run(
        ["git", "commit", "-m", "initial commit", "-q"],
        cwd=fuchsia_dir,
        check=True,
    )
    subprocess.run(
        ["git", "tag", "JIRI_HEAD", "HEAD"], cwd=fuchsia_dir, check=True
    )

    jiri_root = os.path.join(fuchsia_dir, ".jiri_root")
    os.makedirs(jiri_root)
    os.makedirs(os.path.join(jiri_root, "worktrees"))
    with open(os.path.join(jiri_root, "worktrees_registry"), "w") as f:
        pass

    mock_jiri_dir = os.path.join(jiri_root, "bin")
    os.makedirs(mock_jiri_dir)
    mock_jiri_path = os.path.join(mock_jiri_dir, "jiri")

    jiri_script = """#!/bin/bash
base_dir="$(cd "$(dirname "$0")/../.." && pwd)"
if [ "$1" = "worktree" ]; then
    if [ "$2" = "add" ]; then
        target_path="$3"
        git -C "$base_dir" worktree add -q -f --detach "$target_path" main
        echo "$(realpath "$target_path")" >> "$base_dir/.jiri_root/worktrees_registry"
        exit 0
    elif [ "$2" = "remove" ]; then
        target_path="$3"
        if [ "$target_path" = "-f" ] || [ "$target_path" = "-force" ]; then
            target_path="$4"
        fi
        resolved_path="$(realpath "$target_path")"
        if [ -f "$base_dir/.jiri_root/worktrees_registry" ]; then
            grep -v "^$resolved_path$" "$base_dir/.jiri_root/worktrees_registry" > "$base_dir/.jiri_root/worktrees_registry.tmp" || true
            mv "$base_dir/.jiri_root/worktrees_registry.tmp" "$base_dir/.jiri_root/worktrees_registry"
        fi
        git -C "$base_dir" worktree remove -f "$target_path" 2>/dev/null || rm -rf "$target_path"
        exit 0
    elif [ "$2" = "clean" ]; then
        exit 0
    fi
fi
exit 0
"""
    with open(mock_jiri_path, "w") as f:
        f.write(jiri_script)
    os.chmod(mock_jiri_path, 0o755)

    os.makedirs(os.path.join(fuchsia_dir, "scripts"))
    mock_fx_path = os.path.join(fuchsia_dir, "scripts", "fx")
    fx_script = """#!/bin/bash
if [ "$1" = "set" ]; then
    cfg="$2"
    product="${cfg%.*}"
    board="${cfg#*.}"
    if [ "$product" = "$board" ]; then
        board="x64"
    fi
    outdir="out/${cfg}-balanced"
    mkdir -p "$outdir"
    echo "build_info_product = \\"$product\\"" > "$outdir/args.gn"
    echo "build_info_board = \\"$board\\"" >> "$outdir/args.gn"
    echo "$outdir" > ".fx-build-dir"
    echo "✔ Configure outdir for $cfg"
    exit 0
fi

if [ "$1" = "use" ]; then
    cfg="$2"
    echo "$cfg" > ".fx-build-dir"
    echo "Using build directory $cfg"
    exit 0
fi

if [ "$1" = "build" ]; then
    build_dir=$(cat .fx-build-dir 2>/dev/null || echo "out/default")
    cfg=$(basename "$build_dir" | sed 's/-balanced//')
    echo "Building $cfg..."
    for i in {1..10}; do
        printf "[%d/100] CXX obj/src/file_%d.o\\r" $((i*10)) $i
        sleep 0.05
    done
    echo -e "\\nDone"
    exit 0
fi
exit 0
"""
    with open(mock_fx_path, "w") as f:
        f.write(fx_script)
    os.chmod(mock_fx_path, 0o755)

    subprocess.run(["git", "add", "scripts/fx"], cwd=fuchsia_dir, check=True)
    subprocess.run(
        ["git", "commit", "-m", "add mock fx", "-q"],
        cwd=fuchsia_dir,
        check=True,
    )

    return test_dir, fuchsia_dir


def make_replacer(fuchsia_dir):
    def replace(text):
        if not text:
            return text
        return text.replace(fuchsia_dir, "/home/user/fuchsia")

    return replace


def filter_list_output(
    text,
    allowed_names=["wt-alpha", "wt-beta", "agent-refactor", "agent-bugfix"],
):
    def strip_ansi(s):
        return re.sub(r"\x1b\[[0-9;]*m", "", s)

    lines = text.splitlines()
    entries = []
    current_entry = None

    for line in lines:
        clean_line = strip_ansi(line).strip()
        if not clean_line:
            continue
        if clean_line.startswith("No worktrees found."):
            entries.append((None, [line]))
            continue

        if not clean_line.startswith(" ") and not any(
            clean_line.startswith(m) for m in ["└", "├", "│"]
        ):
            parts = clean_line.split()
            if parts:
                wt_name = parts[0]
                current_entry = (wt_name, [line])
                entries.append(current_entry)
        else:
            if current_entry:
                current_entry[1].append(line)

    allowed_entries = [wt_lines for name, wt_lines in entries]

    if not allowed_entries:
        return "No worktrees found.\n"

    result = ""
    for i, wt_lines in enumerate(allowed_entries):
        result += "\n".join(wt_lines) + "\n"
        if i < len(allowed_entries) - 1:
            result += "\n"
    return result


def compress_frames(frames, max_static_duration_ms=1000):
    if not frames:
        return []

    compressed = []
    prev_img, prev_duration = frames[0]
    merged_any = False

    def images_equal(img1, img2):
        if img1 == img2:
            return True
        return img1.tobytes() == img2.tobytes()

    for img, duration in frames[1:]:
        if images_equal(img, prev_img):
            prev_duration += duration
            merged_any = True
        else:
            if merged_any:
                prev_duration = min(prev_duration, max_static_duration_ms)
            compressed.append((prev_img, prev_duration))
            prev_img = img
            prev_duration = duration
            merged_any = False

    if merged_any:
        prev_duration = min(prev_duration, max_static_duration_ms)
    compressed.append((prev_img, prev_duration))

    return compressed


def make_non_blocking(fd):
    fl = fcntl.fcntl(fd, fcntl.F_GETFL)
    fcntl.fcntl(fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)


def make_blocking(fd):
    fl = fcntl.fcntl(fd, fcntl.F_GETFL)
    fcntl.fcntl(fd, fcntl.F_SETFL, fl & ~os.O_NONBLOCK)


def type_command(multiplexed_term, pane_id, cmd, frames):
    term = (
        multiplexed_term.term_full
        if not multiplexed_term.split
        else (
            multiplexed_term.term_left
            if pane_id == 1
            else multiplexed_term.term_right
        )
    )
    multiplexed_term.active_pane = pane_id

    current_prompt = getattr(term, "prompt", "\x1b[32m❯\x1b[0m ")
    term.write(current_prompt)
    frames.append((multiplexed_term.draw(), 300))

    for i in range(len(cmd)):
        term.write(cmd[i])
        frames.append((multiplexed_term.draw(), 60))

    term.write("\n")
    frames.append((multiplexed_term.draw(), 200))


def run_cmd_anim(
    multiplexed_term,
    pane_id,
    cmd,
    cwd,
    env,
    frames,
    replacer=None,
    filter_fn=None,
    fps=10,
):
    term = (
        multiplexed_term.term_full
        if not multiplexed_term.split
        else (
            multiplexed_term.term_left
            if pane_id == 1
            else multiplexed_term.term_right
        )
    )
    multiplexed_term.active_pane = pane_id

    proc = subprocess.Popen(
        cmd,
        shell=True,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    make_non_blocking(proc.stdout.fileno())

    last_frame_time = time.time()
    frame_interval = 1.0 / fps
    buffer = ""

    while proc.poll() is None:
        r, _, _ = select.select([proc.stdout], [], [], 0.05)
        if proc.stdout in r:
            try:
                data = proc.stdout.read(4096)
                if data:
                    if filter_fn:
                        buffer += data
                    else:
                        if replacer:
                            data = replacer(data)
                        term.write(data)
            except BlockingIOError:
                pass

        if not filter_fn:
            current_time = time.time()
            if current_time - last_frame_time >= frame_interval:
                frames.append(
                    (multiplexed_term.draw(), int(frame_interval * 1000))
                )
                last_frame_time = current_time

    make_blocking(proc.stdout.fileno())
    try:
        data = proc.stdout.read()
        if data:
            if filter_fn:
                buffer += data
            else:
                if replacer:
                    data = replacer(data)
                term.write(data)
    except Exception as e:
        print(f"Error reading remaining in run_cmd_anim: {e}")

    if filter_fn:
        filtered_data = filter_fn(buffer)
        if replacer:
            filtered_data = replacer(filtered_data)
        term.write(filtered_data)

    frames.append((multiplexed_term.draw(), 500))


def run_parallel_cmds_anim(
    multiplexed_term,
    cmd1,
    cwd1,
    env1,
    cmd2,
    cwd2,
    env2,
    frames,
    replacer=None,
    fps=5,
):
    proc1 = subprocess.Popen(
        cmd1,
        shell=True,
        cwd=cwd1,
        env=env1,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    proc2 = subprocess.Popen(
        cmd2,
        shell=True,
        cwd=cwd2,
        env=env2,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    make_non_blocking(proc1.stdout.fileno())
    make_non_blocking(proc2.stdout.fileno())

    last_frame_time = time.time()
    frame_interval = 1.0 / fps

    while proc1.poll() is None or proc2.poll() is None:
        readable = []
        if proc1.poll() is None:
            readable.append(proc1.stdout)
        if proc2.poll() is None:
            readable.append(proc2.stdout)

        if readable:
            r, _, _ = select.select(readable, [], [], 0.05)
            for pipe in r:
                if pipe == proc1.stdout:
                    try:
                        data = proc1.stdout.read(4096)
                        if data:
                            if replacer:
                                data = replacer(data)
                            multiplexed_term.term_left.write(data)
                    except BlockingIOError:
                        pass
                elif pipe == proc2.stdout:
                    try:
                        data = proc2.stdout.read(4096)
                        if data:
                            if replacer:
                                data = replacer(data)
                            multiplexed_term.term_right.write(data)
                    except BlockingIOError:
                        pass

        current_time = time.time()
        if current_time - last_frame_time >= frame_interval:
            frames.append((multiplexed_term.draw(), int(frame_interval * 1000)))
            last_frame_time = current_time

    for proc, term in (
        (proc1, multiplexed_term.term_left),
        (proc2, multiplexed_term.term_right),
    ):
        make_blocking(proc.stdout.fileno())
        try:
            data = proc.stdout.read()
            if data:
                if replacer:
                    data = replacer(data)
                term.write(data)
        except Exception as e:
            print(f"Error reading remaining in run_parallel_cmds_anim: {e}")

    frames.append((multiplexed_term.draw(), 1000))


def generate_frames(main_py, fuchsia_dir):
    term = MultiplexedTerminal()
    frames = []

    env = os.environ.copy()
    env["FUCHSIA_DIR"] = fuchsia_dir
    env["FORCE_COLOR"] = "1"

    replacer = make_replacer(fuchsia_dir)

    cwd_left = fuchsia_dir
    cwd_right = fuchsia_dir

    # Helper to run python3 main.py <subcommand>
    cmd_prefix = f"python3 {main_py}"

    # === Phase 0: Silent Setup (Not in frames) ===
    # Pre-provision the pool so we have checkouts ready with build configs
    subprocess.run(
        f"python3 {main_py} pool add --set fuchsia_internal.arm64 --set fuchsia_internal.x64",
        shell=True,
        cwd=fuchsia_dir,
        env=env,
        check=True,
    )
    subprocess.run(
        f"python3 {main_py} pool add --set fuchsia_internal.arm64 --set fuchsia_internal.x64",
        shell=True,
        cwd=fuchsia_dir,
        env=env,
        check=True,
    )

    # === Phase 1: Setup (Single Pane) ===
    type_command(term, 1, "fx worktree list", frames)
    run_cmd_anim(
        term,
        1,
        f"{cmd_prefix} list",
        fuchsia_dir,
        env,
        frames,
        replacer=replacer,
        filter_fn=filter_list_output,
    )

    type_command(term, 1, "fx worktree add agent-refactor", frames)
    run_cmd_anim(
        term,
        1,
        f"{cmd_prefix} add agent-refactor",
        fuchsia_dir,
        env,
        frames,
        replacer=replacer,
    )

    # === Phase 2: Split & Parallel (Split Pane) ===
    term.perform_split()
    frames.append((term.draw(), 500))

    type_command(term, 2, "fx worktree add agent-bugfix", frames)
    run_cmd_anim(
        term,
        2,
        f"{cmd_prefix} add agent-bugfix",
        fuchsia_dir,
        env,
        frames,
        replacer=replacer,
    )

    type_command(term, 2, "cd $(fx worktree locate agent-bugfix)", frames)
    term.term_right.write("\n")
    term.term_right.prompt = "[agent-bugfix] ❯ "
    cwd_right = os.path.join(
        fuchsia_dir, ".jiri_root", "worktrees", "agent-bugfix"
    )
    frames.append((term.draw(), 500))

    type_command(term, 2, "fx use out/fuchsia_internal.x64-balanced", frames)
    run_cmd_anim(
        term,
        2,
        "scripts/fx use out/fuchsia_internal.x64-balanced",
        cwd_right,
        env,
        frames,
        replacer=replacer,
    )

    type_command(term, 1, "cd $(fx worktree locate agent-refactor)", frames)
    term.term_left.write("\n")
    term.term_left.prompt = "[agent-refactor] ❯ "
    cwd_left = os.path.join(
        fuchsia_dir, ".jiri_root", "worktrees", "agent-refactor"
    )
    frames.append((term.draw(), 500))

    type_command(term, 1, "fx use out/fuchsia_internal.arm64-balanced", frames)
    run_cmd_anim(
        term,
        1,
        "scripts/fx use out/fuchsia_internal.arm64-balanced",
        cwd_left,
        env,
        frames,
        replacer=replacer,
    )

    type_command(term, 1, "fx build build/info:version", frames)
    type_command(term, 2, "fx build build/info:version", frames)

    build_target = "build/info:version"
    run_parallel_cmds_anim(
        term,
        f"scripts/fx build {build_target}",
        cwd_left,
        env,
        f"scripts/fx build {build_target}",
        cwd_right,
        env,
        frames,
        replacer=replacer,
        fps=10,
    )

    # === Phase 3: Cleanup (Split Pane) ===
    type_command(term, 1, "fx worktree remove agent-refactor", frames)
    run_cmd_anim(
        term,
        1,
        f"{cmd_prefix} remove agent-refactor",
        fuchsia_dir,
        env,
        frames,
        replacer=replacer,
    )
    term.term_left.prompt = "❯ "
    cwd_left = fuchsia_dir

    type_command(term, 2, "fx worktree remove agent-bugfix", frames)
    run_cmd_anim(
        term,
        2,
        f"{cmd_prefix} remove agent-bugfix",
        fuchsia_dir,
        env,
        frames,
        replacer=replacer,
    )
    term.term_right.prompt = "❯ "
    cwd_right = fuchsia_dir

    # === Phase 4: Teardown (Single Pane) ===
    term.perform_merge()
    frames.append((term.draw(), 500))

    type_command(term, 1, "fx worktree list", frames)
    run_cmd_anim(
        term,
        1,
        f"{cmd_prefix} list",
        fuchsia_dir,
        env,
        frames,
        replacer=replacer,
        filter_fn=filter_list_output,
    )

    for _ in range(2):
        term.term_full.write("\n")
        frames.append((term.draw(show_cursor=True), 250))
        term.term_full.lines[-1] = ""
        frames.append((term.draw(show_cursor=False), 250))

    return frames


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 generate_demo.py <output_path>")
        sys.exit(1)
    output_path = sys.argv[1]

    print("Using MOCK workspace...")
    test_dir, fuchsia_dir = setup_mock_workspace()

    # Find real tools/devshell/worktree/main.py
    repo_root = os.path.abspath(
        os.path.join(os.path.dirname(__file__), "..", "..", "..", "..")
    )
    main_py = os.path.join(
        repo_root, "tools", "devshell", "worktree", "main.py"
    )
    if not os.path.exists(main_py):
        print(f"Error: Cannot find main.py at {main_py}")
        sys.exit(1)

    try:
        os.makedirs(
            os.path.dirname(os.path.abspath(output_path)), exist_ok=True
        )

        print("Generating frames...")
        frames_data = generate_frames(main_py, fuchsia_dir)

        print("Compressing static frames...")
        frames_data = compress_frames(frames_data)

        images = [f[0] for f in frames_data]
        durations = [f[1] for f in frames_data]

        print(f"Saving GIF to {output_path}...")
        images[0].save(
            output_path,
            save_all=True,
            append_images=images[1:],
            duration=durations,
            loop=0,
        )
        print("Done!")
    finally:
        if test_dir:
            shutil.rmtree(test_dir)
