"""Render README screenshots from a real PTY session against the demo repository.

    python3 scripts/screenshots.py [--binary target/release/kiri] [--out assets/screenshots]

Frames are captured with the terminal emulator from verify_tui.py, written as SVG,
and rasterized at 2x with headless Chrome when it is installed.
"""
import argparse
import copy
import fcntl
import html
import json
import math
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_tui import Screen  # noqa: E402

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
BG = "#0f1318"
CELL_W, CELL_H, FONT = 8.43, 19, 14  # Menlo advance at 14px
PAD, RADIUS = 18, 12

SHOTS = [
    # name, terminal size, [(keys, text to wait for), ...]
    ("review", (150, 38), []),
    ("folder", (120, 34), [(b"k", "working files in this folder")]),
    ("providers", (120, 34), [(b"P", "AI providers")]),
    ("commit", (120, 34), [(b"s", "Staged"), (b"c", "Review commit")]),
]


def demo_repository():
    script = Path(__file__).resolve().parent / "demo.py"
    return json.loads(subprocess.run([sys.executable, str(script)], capture_output=True, text=True, check=True).stdout)


def capture(binary, width, height, steps):
    info = demo_repository()
    screen = Screen(width, height)
    transcript = bytearray()
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
    env = dict(os.environ, KIRI_CONFIG_DIR=info["config"], TERM="xterm-256color", COLORTERM="truecolor")
    env.pop("NO_COLOR", None)
    process = subprocess.Popen([str(binary), "-C", info["repository"]], stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True)
    os.close(slave)

    def pump(seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([master], [], [], 0.05)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    return
                transcript.extend(data)
                screen.feed(data)

    def wait_for(text, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            pump(0.1)
            if text in screen.text():
                pump(0.5)
                return
            if process.poll() is not None:
                raise SystemExit(f"kiri exited early:\n{screen.text()}")
        raise SystemExit(f"timed out waiting for {text!r}:\n{screen.text()}")

    try:
        wait_for("hunk")
        for keys, text in steps:
            os.write(master, keys)
            wait_for(text)
        unhandled = set()
        for params in re.findall(rb"\x1b\[([0-9;]*)m", bytes(transcript)):
            values = [int(v or 0) for v in params.split(b";")]
            index = 0
            while index < len(values):
                value = values[index]
                if value in (38, 48) and index + 1 < len(values) and values[index + 1] == 2:
                    index += 5
                    continue
                if value not in (0, 1, 22, 39, 49, 59):
                    unhandled.add(value)
                index += 1
        if unhandled:
            print(f"warning: SGR codes the emulator ignores: {sorted(unhandled)}", file=sys.stderr)
        return copy.deepcopy(screen)  # shutdown keystrokes below would otherwise redraw over the frame
    finally:
        if process.poll() is None:
            try:
                os.write(master, b"\x1b\x1bq")
                pump(0.5)
            except OSError:
                pass
            if process.poll() is None:
                process.kill()
            process.wait()
        os.close(master)


BOX = {
    "─": "h", "│": "v", "╭": "se", "╮": "sw", "╰": "ne", "╯": "nw",
    "├": "nsE", "┤": "nsW", "┬": "ewS", "┴": "ewN", "┼": "nsew", "┌": "SE", "┐": "SW", "└": "NE", "┘": "NW",
}


def box_path(char, x, top):
    """Return an SVG path for a box-drawing character occupying the cell at (x, top)."""
    cx, cy, w, h = x + CELL_W / 2, top + CELL_H / 2, CELL_W, CELL_H
    r = min(w, h) / 2
    spec = BOX[char]
    if spec == "h":
        return f"M{x} {cy}H{x + w}"
    if spec == "v":
        return f"M{cx} {top}V{top + h}"
    if spec in ("se", "sw", "ne", "nw"):
        # Rounded corner: arc from one edge midpoint to the other through the cell centre.
        vertical = top + h if "s" in spec else top
        horizontal = x + w if "e" in spec else x
        sweep = {"se": 0, "sw": 1, "ne": 1, "nw": 0}[spec]
        return f"M{horizontal} {cy}A{r} {r} 0 0 {sweep} {cx} {vertical}"
    parts = []
    if "n" in spec.lower():
        parts.append(f"M{cx} {top}V{cy}")
    if "s" in spec.lower():
        parts.append(f"M{cx} {cy}V{top + h}")
    if "e" in spec.lower():
        parts.append(f"M{cx} {cy}H{x + w}")
    if "w" in spec.lower():
        parts.append(f"M{x} {cy}H{cx}")
    return "".join(parts)


def svg(screen):
    inner_w, inner_h = screen.width * CELL_W, screen.height * CELL_H
    total_w, total_h = inner_w + 2 * PAD, inner_h + 2 * PAD
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="{total_h}" viewBox="0 0 {total_w} {total_h}">',
        f'<rect width="{total_w}" height="{total_h}" rx="{RADIUS}" fill="{BG}"/>',
    ]
    for y, row in enumerate(screen.cells):
        start = 0
        while start < screen.width:
            style = screen.styles[y][start]
            end = start + 1
            while end < screen.width and screen.styles[y][end] == style:
                end += 1
            fg, bg, bold = style
            x, top = PAD + start * CELL_W, PAD + y * CELL_H
            if bg != BG:
                parts.append(f'<rect x="{x}" y="{top}" width="{(end - start) * CELL_W}" height="{CELL_H}" fill="{bg}"/>')
            cells = list(row[start:end])
            paths = [box_path(c, x + i * CELL_W, top) for i, c in enumerate(cells) if c in BOX]
            if paths:
                parts.append(f'<path d="{"".join(paths)}" stroke="{fg}" stroke-width="1.4" fill="none" stroke-linecap="butt"/>')
            text = "".join(" " if c in BOX else c for c in cells)
            if text.strip():
                weight = 700 if bold else 400
                parts.append(
                    f'<text x="{x}" y="{top + 14}" font-family="Menlo,SF Mono,Consolas,monospace" font-size="{FONT}" '
                    f'font-weight="{weight}" fill="{fg}" xml:space="preserve" textLength="{(end - start) * CELL_W}" '
                    f'lengthAdjust="spacingAndGlyphs">{html.escape(text)}</text>'
                )
            start = end
    parts.append("</svg>")
    return "\n".join(parts), total_w, total_h


def rasterize(svg_path, png_path, width, height):
    if not Path(CHROME).exists():
        print(f"skipping PNG for {png_path.name}: Chrome not found", file=sys.stderr)
        return False
    profile = Path(tempfile.mkdtemp(prefix="kiri-shot-chrome-"))
    png_path = Path(png_path).resolve()
    png_path.unlink(missing_ok=True)
    # Chrome writes the screenshot promptly but does not always exit afterwards, so poll and stop it.
    process = subprocess.Popen(
        [CHROME, "--headless", "--disable-gpu", "--hide-scrollbars", "--no-first-run", "--disable-extensions",
         f"--user-data-dir={profile}", "--default-background-color=00000000", "--force-device-scale-factor=2",
         f"--window-size={math.ceil(width)},{math.ceil(height)}", f"--screenshot={png_path}", svg_path.resolve().as_uri()],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if png_path.exists() and png_path.stat().st_size > 0:
                time.sleep(0.5)
                return True
            if process.poll() is not None:
                break
            time.sleep(0.2)
        raise SystemExit(f"Chrome did not write {png_path}")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        shutil.rmtree(profile, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/kiri")
    parser.add_argument("--out", default="assets/screenshots", type=Path)
    parser.add_argument("--only", nargs="*")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    for name, (width, height), steps in SHOTS:
        if args.only and name not in args.only:
            continue
        screen = capture(binary, width, height, steps)
        content, total_w, total_h = svg(screen)
        svg_path = args.out / f"{name}.svg"
        svg_path.write_text(content)
        png_path = args.out / f"{name}.png"
        if rasterize(svg_path, png_path, total_w, total_h):
            svg_path.unlink()
        print(f"{name}: {width}x{height} -> {png_path if png_path.exists() else svg_path}")


if __name__ == "__main__":
    main()
