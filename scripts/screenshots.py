"""Render README screenshots from a real PTY session against the demo repository.

    python3 scripts/screenshots.py [--binary target/release/kiri] [--out assets/screenshots]

Frames are captured with the terminal emulator from verify_tui.py, written as SVG,
and rasterized at 2x with headless Chrome when it is installed.

AI screens are captured against a scripted model: a local HTTP server that speaks
the OpenAI Responses protocol and answers every request with a fixed draft or plan
for the demo repository. Kiri runs its real capture, cost estimate, review, and
guard code; only the model text is canned. No request leaves the machine.
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
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_tui import Screen  # noqa: E402

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
BG = "#0f1318"
CELL_W, CELL_H, FONT = 8.43, 19, 14  # Menlo advance at 14px
PAD, RADIUS = 18, 12
CTRL_B, CTRL_S = b"\x02", b"\x13"

DRAFT = (
    "search: rank matches and ignore blank queries\n\n"
    "Lower-case both sides before matching, score prefix matches above\n"
    "substring matches, and cap results at 50. A blank query now returns\n"
    "nothing instead of every path."
)
PLAN = [
    ("search: rank matches and ignore blank queries",
     "src/search.rs is a self-contained change to matching and ordering.",
     ["src/search.rs"]),
    ("workspace: open a workspace from its root path",
     "Workspace::open and the test that covers it ship together.",
     ["src/workspace.rs", "tests/workspace.rs"]),
    ("chore: ignore the target directory",
     "Unrelated to the code changes; keep it out of both feature commits.",
     [".gitignore"]),
]

SHOTS = [
    # name, terminal size, settings.json for the scripted model (None = AI not connected), [(keys, text to wait for), ...]
    ("review", (150, 38), {}, []),
    ("plan", (150, 38), {}, [(CTRL_B, "Review commit plan")]),
    ("draft", (120, 34), {}, [(b"a", "Review commit")]),
    ("cost", (120, 34), {"ui": {"auto_approve_calls": 0}}, [(CTRL_B, "Review AI analysis")]),
    ("providers", (120, 34), None, [(b"P", "AI providers")]),
    ("themes", (120, 34), {}, [(b"T", "Themes")]),
]


def strings(value):
    """Every string inside a JSON value, in document order."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


class ScriptedModel(BaseHTTPRequestHandler):
    """OpenAI Responses endpoint that returns the canned draft or plan above."""

    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        result = request["text"]["format"]["schema"]["properties"]["result"]["properties"]
        prompt = "\n".join(strings(request["input"]))
        if "commits" in result:
            catalog = json.loads(re.search(r"COMMIT UNITS\n(\[.*\])", prompt).group(1))
            assignments = {}
            for unit in catalog:
                index = next((i for i, (_, _, paths) in enumerate(PLAN) if any(unit["path"].endswith(p) for p in paths)), len(PLAN) - 1)
                assignments[unit["id"]] = index
            output = {"commits": [{"message": m, "reason": r} for m, r, _ in PLAN], "assignments": assignments}
        else:
            output = {"message": DRAFT}
        text = json.dumps({"action": "finish", "result": output, "requests": [], "notes": ""})
        body = json.dumps({
            "id": "resp_demo", "object": "response", "created_at": 0, "status": "completed", "model": request.get("model", "demo"),
            "output": [{"type": "message", "id": "msg_demo", "role": "assistant", "status": "completed",
                        "content": [{"type": "output_text", "text": text, "annotations": []}]}],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def scripted_model():
    server = ThreadingHTTPServer(("127.0.0.1", 0), ScriptedModel)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}"


def connect(config, endpoint, overrides):
    """Write settings and a placeholder key so the TUI treats the scripted model as a connected provider."""
    config = Path(config)
    config.mkdir(parents=True, exist_ok=True)
    settings = {"active": "openai", "providers": {"openai": {"model": "gpt-5-mini", "endpoint": endpoint}}}
    for key, value in overrides.items():
        settings.setdefault(key, {}).update(value)
    (config / "settings.json").write_text(json.dumps(settings))
    (config / "credentials.json").write_text(json.dumps({"openai": {"type": "api_key", "key": "scripted-model"}}))
    os.chmod(config / "credentials.json", 0o600)


def demo_repository():
    script = Path(__file__).resolve().parent / "demo.py"
    return json.loads(subprocess.run([sys.executable, str(script)], capture_output=True, text=True, check=True).stdout)


def capture(binary, width, height, settings, steps):
    info = demo_repository()
    server = None
    if settings is not None:
        server, endpoint = scripted_model()
        connect(info["config"], endpoint, settings)
    screen = Screen(width, height)
    transcript = bytearray()
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
    env = dict(os.environ, KIRI_CONFIG_DIR=info["config"], TERM="xterm-256color", COLORTERM="truecolor")
    env.pop("NO_COLOR", None)
    env.pop("OPENAI_API_KEY", None)
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
        if server is not None:
            server.shutdown()


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
    for name, (width, height), settings, steps in SHOTS:
        if args.only and name not in args.only:
            continue
        screen = capture(binary, width, height, settings, steps)
        content, total_w, total_h = svg(screen)
        svg_path = args.out / f"{name}.svg"
        svg_path.write_text(content)
        png_path = args.out / f"{name}.png"
        if rasterize(svg_path, png_path, total_w, total_h):
            svg_path.unlink()
        print(f"{name}: {width}x{height} -> {png_path if png_path.exists() else svg_path}")


if __name__ == "__main__":
    main()
