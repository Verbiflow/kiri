import argparse
import codecs
import fcntl
import html
import json
import os
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import time
import unicodedata
from pathlib import Path


class Screen:
    def __init__(self, width, height):
        self.width, self.height = width, height
        self.cells = [[" "] * width for _ in range(height)]
        self.fg, self.bg, self.bold = "#dae0e7", "#0f1318", False
        self.styles = [[(self.fg, self.bg, self.bold)] * width for _ in range(height)]
        self.x = self.y = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def style(self, values):
        index = 0
        while index < len(values):
            value = values[index]
            if value == 0:
                self.fg, self.bg, self.bold = "#dae0e7", "#0f1318", False
            elif value == 1:
                self.bold = True
            elif value == 22:
                self.bold = False
            elif value == 39:
                self.fg = "#dae0e7"
            elif value == 49:
                self.bg = "#0f1318"
            elif value in (38, 48) and index + 4 < len(values) and values[index + 1] == 2:
                color = "#{:02x}{:02x}{:02x}".format(*values[index + 2:index + 5])
                if value == 38:
                    self.fg = color
                else:
                    self.bg = color
                index += 4
            index += 1

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith("\x1b["):
                match = re.match(r"\x1b\[([0-9;?=>]*)([ -/]*)([@-~])", self.pending)
                if not match:
                    return
                params, _, command = match.groups()
                self.pending = self.pending[match.end():]
                if params.startswith(("?", ">", "=")):
                    continue
                values = [int(v or 0) for v in params.split(";")]
                n = values[0] or 1
                if command in "Hf":
                    self.y = max(0, min(self.height - 1, n - 1))
                    self.x = max(0, min(self.width - 1, (values[1] if len(values) > 1 else 1) - 1))
                elif command == "G":
                    self.x = min(self.width - 1, n - 1)
                elif command == "A":
                    self.y = max(0, self.y - n)
                elif command == "B":
                    self.y = min(self.height - 1, self.y + n)
                elif command == "C":
                    self.x = min(self.width - 1, self.x + n)
                elif command == "D":
                    self.x = max(0, self.x - n)
                elif command == "m":
                    self.style(values)
                elif command == "J" and values[0] in (2, 3):
                    self.cells = [[" "] * self.width for _ in range(self.height)]
                    self.styles = [[(self.fg, self.bg, self.bold)] * self.width for _ in range(self.height)]
                elif command == "K":
                    start = 0 if values[0] in (1, 2) else self.x
                    end = self.x + 1 if values[0] == 1 else self.width
                    self.cells[self.y][start:end] = [" "] * (end - start)
                    self.styles[self.y][start:end] = [(self.fg, self.bg, self.bold)] * (end - start)
                continue
            if self.pending.startswith("\x1b]"):
                end = re.search("\x07|\x1b\\\\", self.pending)
                if not end:
                    return
                self.pending = self.pending[end.end():]
                continue
            if self.pending == "\x1b":
                return
            char, self.pending = self.pending[0], self.pending[1:]
            if char == "\r":
                self.x = 0
            elif char == "\n":
                self.y = min(self.height - 1, self.y + 1)
            elif ord(char) >= 32 and char != "\x7f" and self.x < self.width:
                if unicodedata.combining(char) and self.x > 0:
                    self.cells[self.y][self.x - 1] += char
                    continue
                size = 2 if unicodedata.east_asian_width(char) in "WF" else 1
                self.cells[self.y][self.x] = char
                self.styles[self.y][self.x] = (self.fg, self.bg, self.bold)
                if size == 2 and self.x + 1 < self.width:
                    self.cells[self.y][self.x + 1] = ""
                    self.styles[self.y][self.x + 1] = (self.fg, self.bg, self.bold)
                self.x = min(self.width, self.x + size)

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.cells)

    def svg(self):
        cw, ch = 9, 19
        canvas = max(self.width * cw, self.height * ch)
        parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{canvas}" height="{canvas}" viewBox="0 0 {canvas} {canvas}">', '<rect width="100%" height="100%" fill="#0f1318"/>']
        for y, row in enumerate(self.cells):
            start = 0
            while start < self.width:
                style = self.styles[y][start]
                end = start + 1
                while end < self.width and self.styles[y][end] == style:
                    end += 1
                fg, bg, bold = style
                text = "".join(row[start:end])
                parts.append(f'<rect x="{start * cw}" y="{y * ch}" width="{(end - start) * cw}" height="{ch}" fill="{bg}"/>')
                if text.strip():
                    parts.append(f'<text x="{start * cw}" y="{y * ch + 14}" font-family="Menlo,Consolas,monospace" font-size="14" font-weight="{700 if bold else 400}" fill="{fg}" xml:space="preserve" textLength="{(end - start) * cw}" lengthAdjust="spacingAndGlyphs">{html.escape(text)}</text>')
                start = end
        return "\n".join(parts + ["</svg>"])


def save(path, content):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as file:
        os.fchmod(file.fileno(), 0o600)
        file.write(content)


def git(root, *args):
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_AUTHOR_NAME="Kiri Test", GIT_AUTHOR_EMAIL="test@example.invalid", GIT_COMMITTER_NAME="Kiri Test", GIT_COMMITTER_EMAIL="test@example.invalid")
    return subprocess.run(["git", "-C", str(root), *args], env=env, check=True, capture_output=True).stdout


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/kiri")
    parser.add_argument("--repo", type=Path)
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--svg", type=Path)
    parser.add_argument("--color", choices=["default", "auto", "always", "never"], default="default")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    with tempfile.TemporaryDirectory(prefix="kiri-tui-check-") as tmp:
        temp = Path(tmp)
        repo = args.repo
        if repo is None:
            repo = temp / "project"
            repo.mkdir()
            git(repo, "init", "--initial-branch=main", "--quiet")
            sources = {"alpha.rs": "fn main() {\n    let count = 1;\n}\n", "beta.rs": "fn second() {}\n", "folder/one.rs": "fn one() {}\n", "folder/nested/two.rs": "fn two() {}\n", "folder-other/unrelated.rs": "fn unrelated() {}\n"}
            for name, content in sources.items():
                (repo / name).parent.mkdir(parents=True, exist_ok=True)
                (repo / name).write_text(content)
            git(repo, "add", ".")
            git(repo, "commit", "--quiet", "-m", "initial")
            (repo / ".git/info/exclude").write_text("folder/\n")
            (repo / "folder/generated.tmp").write_text("Ignored untracked content must not be staged\n")
            (repo / "alpha.rs").write_text("fn main() {\n    let count = 2;\n}\n")
            (repo / "beta.rs").write_text("fn second() { println!(\"ready\"); }\n")
            for name in ["folder/one.rs", "folder/nested/two.rs", "folder-other/unrelated.rs"]:
                (repo / name).write_text(sources[name] + "fn added() {}\n")
        screen = Screen(160, 42)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 160, 0, 0))
        env = dict(os.environ, KIRI_CONFIG_DIR=str(temp / "config"), TERM="xterm-256color", NO_COLOR="1")
        if args.repo is None:
            env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_AUTHOR_NAME="Kiri Test", GIT_AUTHOR_EMAIL="test@example.invalid", GIT_COMMITTER_NAME="Kiri Test", GIT_COMMITTER_EMAIL="test@example.invalid")
        started = time.perf_counter()
        command = [str(binary), "-C", str(repo)]
        if args.color != "default":
            command.extend(["--color", args.color])
        process = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True)
        os.close(slave)
        first_frame = None
        transcript = bytearray()

        def receive(seconds=0.05):
            nonlocal first_frame
            if select.select([master], [], [], seconds)[0]:
                data = os.read(master, 65536)
                transcript.extend(data)
                screen.feed(data)
                if first_frame is None and "kiri" in screen.text():
                    first_frame = (time.perf_counter() - started) * 1000

        def wait_for(text, timeout=12):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                receive()
                if text in screen.text():
                    return
                if process.poll() is not None:
                    raise AssertionError(f"TUI exited early: {screen.text()}")
            raise AssertionError(f"Did not see {text!r}:\n{screen.text()}")

        def press(keys, text):
            os.write(master, keys)
            wait_for(text)

        def close(title):
            os.write(master, b"\x1b")
            deadline = time.monotonic() + 5
            while title in screen.text() and time.monotonic() < deadline:
                receive()
            assert title not in screen.text(), f"Panel did not close: {title}"

        try:
            wait_for("hunk")
            first_patch = (time.perf_counter() - started) * 1000
            first_syntax = None
            if args.color in ("default", "always"):
                deadline = time.monotonic() + 12
                syntax_colors = [b"38;2;194;152;255", b"38;2;156;219;150", b"38;2;247;174;122", b"38;2;123;185;255", b"38;2;114;205;226"]
                while not any(color in transcript for color in syntax_colors) and time.monotonic() < deadline:
                    receive()
                assert any(color in transcript for color in syntax_colors), f"No syntax colors emitted:\n{screen.text()}"
                if args.repo is None:
                    assert b"38;2;194;152;255" in transcript, "Rust keywords were not colored"
                first_syntax = (time.perf_counter() - started) * 1000
            else:
                assert not re.search(rb"(?:38|48);(?:2|5);", transcript), "NO_COLOR/--color never was ignored"
            for _ in range(5):
                receive(0.03)
            if args.snapshot:
                save(args.snapshot, screen.text() + "\n")
            if args.svg:
                save(args.svg, screen.svg())
            press(b"?", "Keyboard shortcuts")
            close("Keyboard shortcuts")
            press(b"P", "AI providers")
            close("AI providers")
            press(b"\x1b[<0;20;3M\x1b[<0;20;3m", "No automatic merge")
            close("No automatic merge")
            press(b"B", "Local branches")
            close("Local branches")
            press(b"l", "Recent history")
            close("Recent history")
            if args.repo is None:
                press(b"j", "╭ beta.rs")
                press(b" ", "Index updated")
                press(b"c", "Review commit")
                os.write(master, b"fix: verify terminal commit editor")
                press(b"\x13", "Committed")
                assert git(repo, "show", "--format=", "--name-only", "HEAD").decode().strip() == "beta.rs"
                press(b"g", "2 working files in this folder")
                press(b" ", "Stage 2 files?")
                press(b"\r", "Index updated")
                expected = {"folder/one.rs", "folder/nested/two.rs"}
                assert set(git(repo, "diff", "--cached", "--name-only").decode().splitlines()) == expected
                press(b"s", "s Staged 2")
                press(b"g", "2 staged files in this folder")
                press(b" ", "Unstage 2 files?")
                press(b"\r", "Nothing staged")
                assert git(repo, "diff", "--cached", "--name-only") == b""
                assert "added" in (repo / "folder/one.rs").read_text()
                assert "added" in (repo / "folder-other/unrelated.rs").read_text()
                press(b"u", "╭ alpha.rs")
                press(b" ", "s Staged 1")
                press(b"g", "2 working files in this folder")
                press(b" ", "Stage 2 files?")
                press(b"\r", "s Staged 3")
                press(b"s", "2 staged files in this folder")
                press(b"c", "2 selected files only")
                os.write(master, b"feat: commit just the selected folder")
                press(b"\x13", "Committed")
                assert set(git(repo, "show", "--format=", "--name-only", "HEAD").decode().splitlines()) == expected
                assert git(repo, "diff", "--cached", "--name-only").decode().strip() == "alpha.rs"
                assert git(repo, "ls-files", "--", "folder/generated.tmp") == b""
                assert (repo / "folder/generated.tmp").exists()
            os.write(master, b"q")
            deadline = time.monotonic() + 5
            while process.poll() is None and time.monotonic() < deadline:
                try:
                    receive()
                except OSError:
                    break
            process.wait(timeout=1)
            assert process.returncode == 0
            checks = ["real PTY startup", "interactive patch", "actual ANSI color policy", "help", "provider picker", "mouse pull confirmation without execution", "branches", "history", "clean exit"]
            if args.repo is None:
                checks.extend(["file staging", "manual commit", "collapsed folder staging", "folder unstage", "staged selection follows the folder", "folder-scoped commit preserves other staged files", "sibling and working-file preservation", "tracked changes under ignored folders", "ignored untracked files stay untracked"])
            print(json.dumps({"first_frame_ms": round(first_frame, 2), "first_patch_ms": round(first_patch, 2), "first_syntax_ms": round(first_syntax, 2) if first_syntax else None, "checks": checks}, indent=2))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)


if __name__ == "__main__":
    main()
