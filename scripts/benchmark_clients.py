import argparse
import fcntl
import hashlib
import json
import os
import platform
import pty
import re
import select
import statistics
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path
from verify_tui import Screen


def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, env=environment(), stderr=subprocess.DEVNULL)


def environment():
    return dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_OPTIONAL_LOCKS="0", GIT_AUTHOR_NAME="Benchmark", GIT_AUTHOR_EMAIL="bench@example.invalid", GIT_COMMITTER_NAME="Benchmark", GIT_COMMITTER_EMAIL="bench@example.invalid", TERM="xterm-256color", COLORTERM="truecolor")


def fixture(parent, count):
    repo = parent / f"files-{count}"
    repo.mkdir()
    (repo / "bulk").mkdir()
    git(repo, "init", "-q")
    (repo / "000_main.rs").write_text("fn prior_value() -> u32 { 1 }\n")
    for index in range(count - 1):
        (repo / "bulk" / f"file-{index:06}.rs").write_text(f"fn file_{index}() -> u32 {{ 1 }}\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "Fixture baseline")
    (repo / "000_main.rs").write_text("fn observed_after_731() -> u32 { 2 }\n")
    for index in range(count - 1):
        (repo / "bulk" / f"file-{index:06}.rs").write_text(f"fn file_{index}() -> u32 {{ 2 }}\n")
    return repo


NAVIGATION = {"kiri": {"prepare": [b"k", b"\r"], "next": b"j"}, "gitui": {"prepare": [], "next": b"\x1b[B"}}
NAVIGATION_PATTERN = re.compile(r"file_(\d+)\(\) -> u32 \{ 2 \}")


def pump(master, screen, ready, timeout):
    started = time.perf_counter()
    while time.perf_counter() - started < timeout:
        if select.select([master], [], [], 0.005)[0]:
            try:
                data = os.read(master, 65536)
            except OSError:
                return None
            if not data:
                return None
            for query, answer in [(b"\x1b]11;?", b"\x1b]11;rgb:1010/1010/1010\x07"), (b"\x1b[6n", b"\x1b[1;1R"), (b"\x1b[c", b"\x1b[?1;2c"), (b"\x1b[?u", b"\x1b[?0u")]:
                if query in data:
                    os.write(master, answer)
            screen.feed(data)
            if ready(screen.text()):
                return (time.perf_counter() - started) * 1000
    return None


def navigation(master, screen, name, presses, cadence):
    keys = NAVIGATION.get(name)
    if not keys:
        return []
    for key in keys["prepare"]:
        os.write(master, key)
        pump(master, screen, lambda text: False, 0.15)
    seen = {int(match) for match in NAVIGATION_PATTERN.findall(screen.text())}
    latencies = []
    for _ in range(presses):
        pump(master, screen, lambda text: False, cadence)
        before = set(seen)
        os.write(master, keys["next"])
        latency = pump(master, screen, lambda text: bool({int(m) for m in NAVIGATION_PATTERN.findall(text)} - before), 2.0)
        seen |= {int(match) for match in NAVIGATION_PATTERN.findall(screen.text())}
        if latency is not None:
            latencies.append(latency)
    return latencies


def measure(name, binary, repo, output, number, presses, cadence, idle):
    config = output / f"{name}-config"
    config.mkdir(exist_ok=True)
    trace = output / f"{name}-{number}-git.jsonl"
    env = dict(environment(), XDG_CONFIG_HOME=str(config), XDG_CACHE_HOME=str(config), KIRI_CONFIG_DIR=str(config), GIT_TRACE2_EVENT=str(trace))
    env.pop("NO_COLOR", None)
    commands = {"kiri": [binary, "-C", str(repo)], "lumen": [binary, "diff", "--theme", "dracula"], "gitui": [binary]}
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 160, 0, 0))
    screen = Screen(160, 42)
    started = time.perf_counter()
    process = subprocess.Popen(commands[name], cwd=repo, env=env, stdin=slave, stdout=slave, stderr=slave, close_fds=True)
    os.close(slave)
    inventory = patch = None
    transcript = bytearray()
    try:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.01)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                at = (time.perf_counter() - started) * 1000
                transcript.extend(data)
                for query, answer in [(b"\x1b]11;?", b"\x1b]11;rgb:1010/1010/1010\x07"), (b"\x1b[6n", b"\x1b[1;1R"), (b"\x1b[c", b"\x1b[?1;2c"), (b"\x1b[?u", b"\x1b[?0u")]:
                    if query in data:
                        os.write(master, answer)
                screen.feed(data)
                text = screen.text()
                if inventory is None and "000_main.rs" in text:
                    inventory = at
                if "observed_after_731" in text:
                    patch = at
                    break
            if process.poll() is not None:
                break
        rss = None
        if process.poll() is None:
            measured = subprocess.run(["ps", "-p", str(process.pid), "-o", "rss="], capture_output=True, text=True)
            if measured.returncode == 0 and measured.stdout.strip().isdigit():
                rss = int(measured.stdout.strip())
        (output / f"{name}-{number}.txt").write_text(screen.text())
        (output / f"{name}-{number}.ansi").write_bytes(transcript)
        nav = navigation(master, screen, name, presses, cadence) if patch is not None and process.poll() is None else []
        startup_processes = git_processes(trace)
        if idle and process.poll() is None:
            pump(master, screen, lambda text: False, idle)
        idle_processes = git_processes(trace) - startup_processes
        os.write(master, b"q")
        end = time.monotonic() + 2
        while process.poll() is None and time.monotonic() < end:
            if select.select([master], [], [], 0.02)[0]:
                try:
                    os.read(master, 65536)
                except OSError:
                    break
        return {"inventory_ms": inventory, "visible_patch_ms": patch, "timed_out": patch is None, "parent_rss_at_ready_kib": rss,
                "navigation_ms": nav, "navigation_median_ms": statistics.median(nav) if nav else None,
                "startup_git_processes": startup_processes, "idle_git_processes": idle_processes, "idle_seconds": idle, "read_only_keys": "q"}
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        os.close(master)


def git_processes(trace):
    try:
        return sum(1 for line in trace.read_text().splitlines() if line and json.loads(line).get("event") == "start")
    except (OSError, ValueError):
        return 0


def main():
    parser = argparse.ArgumentParser(description="Compare terminal Git clients on identical disposable fixtures: launch to visible patch, keypress to next patch, and Git processes spawned while idle.")
    parser.add_argument("--kiri", required=True)
    parser.add_argument("--lumen")
    parser.add_argument("--gitui")
    parser.add_argument("--files", type=int, nargs="+", default=[100, 5000])
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--presses", type=int, default=12, help="Keypresses measured for navigation latency")
    parser.add_argument("--cadence", type=float, default=0.1, help="Seconds between navigation keypresses; 0 measures back-to-back presses")
    parser.add_argument("--idle", type=float, default=6.0, help="Seconds to stay idle after navigation while counting Git processes")
    args = parser.parse_args()
    output = Path(tempfile.mkdtemp(prefix="kiri-client-benchmark-"))
    binaries = {name: str(Path(value).resolve()) for name in ("kiri", "lumen", "gitui") if (value := getattr(args, name))}
    report = {"platform": platform.platform(), "terminal": {"columns": 160, "rows": 42}, "cache_policy": "No OS cache flush. First process run recorded separately; later runs rotate client order.", "binaries": binaries,
              "methodology": f"Warm medians exclude the first run. Navigation presses the client's move-down key every {args.cadence:g} s and times the next patch. Idle Git processes are counted from Git Trace2 after navigation settles.", "navigation_cadence_s": args.cadence, "scenarios": []}
    report["sha256"] = {}
    for name, binary in binaries.items():
        with open(binary, "rb") as executable:
            report["sha256"][name] = hashlib.file_digest(executable, "sha256").hexdigest()
    for count in args.files:
        repo = fixture(output, count)
        head = git(repo, "rev-parse", "HEAD")
        scenario = {"files": count, "runs": {name: [] for name in binaries}}
        for number in range(args.runs):
            names = list(binaries)
            names = names[number % len(names):] + names[:number % len(names)]
            for name in names:
                record = measure(name, binaries[name], repo, output, f"{count}-{number}", args.presses, args.cadence, args.idle)
                scenario["runs"][name].append(record)
                assert git(repo, "rev-parse", "HEAD") == head
                assert git(repo, "diff", "--cached", "--name-only") == b""
                print(json.dumps({"files": count, "client": name, "run": number, **record}), flush=True)
        scenario["warm_medians_ms"] = {name: {metric: statistics.median(values) if (values := [run[metric] for run in runs[1:] if run[metric] is not None]) else None for metric in ("inventory_ms", "visible_patch_ms", "navigation_median_ms")} for name, runs in scenario["runs"].items()}
        scenario["idle_git_processes"] = {name: [run["idle_git_processes"] for run in runs] for name, runs in scenario["runs"].items()}
        print(json.dumps({"files": count, "warm_medians_ms": scenario["warm_medians_ms"], "idle_git_processes": scenario["idle_git_processes"]}), flush=True)
        report["scenarios"].append(scenario)
        (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps({"report": str(output / "report.json")}, indent=2))


if __name__ == "__main__":
    main()
