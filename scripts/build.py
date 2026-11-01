"""Build SaveStates' Windows x64 runnables into target/runnables.

Builds every runnable that exists in the workspace:
  savestates.exe        the app (crates/app, once it exists)
  savestates-spike.exe  the Raven spike (crates/ui-raven/examples/spike.rs)

Each build compiles a snapshot of the working tree (uncommitted changes included) in a
throwaway directory under target/, so the output never depends on stale local state.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import glob
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
TRIPLE = "x86_64-pc-windows-msvc"
SOURCE = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo", "crates", "vendor")
RAVEN_DIR = ROOT / "third_party/raven-bin"
RAVEN_VERSION = "v0.2.0"
VSWHERE = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"


@dataclass(frozen=True)
class Runnable:
    key: str
    package: str
    kind: str  # "bin" or "example"
    target: str
    output_name: str
    manifest: Path

    def available(self) -> bool:
        return self.manifest.is_file()


RUNNABLES = (
    Runnable("app", "savestates", "bin", "savestates", "savestates.exe", ROOT / "crates/app/Cargo.toml"),
    Runnable("spike", "savestates-ui-raven", "example", "spike", "savestates-spike.exe",
             ROOT / "crates/ui-raven/examples/spike.rs"),
)


def run(argv, *, label=None, **kwargs):
    print("BUILD", label or subprocess.list2cmdline(list(map(str, argv))), flush=True)
    return subprocess.run(list(map(str, argv)), check=True, **kwargs)


def capture(argv, **kwargs) -> str:
    return subprocess.check_output(list(map(str, argv)), text=True, **kwargs).strip()


def preflight():
    if os.name != "nt":
        raise RuntimeError("SaveStates builds for Windows only; run this on Windows with Rust and MSVC")
    if platform.machine().lower() not in ("amd64", "x86_64"):
        raise RuntimeError("This script supports x86_64 Windows only")
    for tool in ("cargo", "rustc", "rustup"):
        if not shutil.which(tool):
            raise RuntimeError(f"Missing {tool}; install Rust (rustup) and the MSVC build tools first")


def ensure_raven():
    """Fetch the pinned Raven release (SHA256-checked) when its library or notices are missing."""
    needed = (RAVEN_DIR / "lib/raven.lib", RAVEN_DIR / "LICENSE", RAVEN_DIR / "NOTICE")
    if all(path.is_file() for path in needed):
        return
    shell = shutil.which("pwsh") or shutil.which("powershell")
    if not shell:
        raise RuntimeError("Raven is missing and neither pwsh nor powershell was found to fetch it")
    run([shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ROOT / "scripts/fetch-raven.ps1"],
        label=f"fetch Raven {RAVEN_VERSION}")
    missing = [str(path) for path in needed if not path.is_file()]
    if missing:
        raise RuntimeError(f"Raven fetch did not produce: {', '.join(missing)}")


def snapshot(destination: Path):
    skip = shutil.ignore_patterns("target", "__pycache__", "*.pdb")
    for name in SOURCE:
        source = ROOT / name
        if source.is_dir():
            shutil.copytree(source, destination / name, ignore=skip)
        elif source.is_file():
            shutil.copy2(source, destination / name)


def build_environment() -> dict:
    env = os.environ.copy()
    # The snapshot's .cargo/config.toml points RAVEN_LIB_DIR at a path relative to the snapshot;
    # an explicit environment variable takes precedence over Cargo's [env] table.
    env["RAVEN_LIB_DIR"] = str(RAVEN_DIR / "lib")
    env["RAVEN_STATIC"] = "1"
    return env


def compile_runnable(runnable: Runnable, source: Path, target_dir: Path, options, env) -> Path:
    command = ["cargo", "build", "--locked", "--target", TRIPLE, "--target-dir", target_dir,
               "--jobs", options.jobs, "-p", runnable.package, f"--{runnable.kind}", runnable.target]
    if options.profile == "release":
        command.append("--release")
    if options.offline:
        command.append("--offline")
    run(command, cwd=source, env=env, label=f"{runnable.output_name} ({options.profile})")
    folder = Path(target_dir) / TRIPLE / options.profile
    if runnable.kind == "example":
        folder = folder / "examples"
    binary = folder / f"{runnable.target}.exe"
    if not binary.is_file():
        raise RuntimeError(f"Cargo finished but {binary} does not exist")
    return binary


def imported_vcruntimes(binary: Path) -> list[str]:
    """The VC++ runtime DLLs the executable imports (import names are ASCII in the PE file)."""
    data = binary.read_bytes().lower()
    return [name for name in ("vcruntime140.dll", "vcruntime140_1.dll") if name.encode() in data]


def find_vcruntime(name: str) -> Path | None:
    """Prefer the Visual Studio redistributable copy (licensed for app-local deployment)."""
    if VSWHERE.is_file():
        try:
            install = capture([VSWHERE, "-latest", "-products", "*", "-requires",
                               "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"])
        except (OSError, subprocess.CalledProcessError):
            install = ""
        if install:
            pattern = str(Path(install) / "VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT" / name)
            candidates = sorted(glob.glob(pattern))
            if candidates:
                return Path(candidates[-1])
    system_copy = Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32" / name
    return system_copy if system_copy.is_file() else None


def smoke(binary: Path, seconds: float):
    """Launch the GUI, require it to stay up for a few seconds, then close it."""
    process = subprocess.Popen([str(binary)], cwd=binary.parent,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    time.sleep(seconds)
    if process.poll() is not None:
        output = process.stdout.read() if process.stdout else ""
        raise RuntimeError(f"{binary.name} exited with code {process.returncode} during the smoke test:\n{output}")
    process.kill()
    process.wait()
    print(f"SMOKE {binary.name} stayed up for {seconds:g} s", flush=True)


def git_facts() -> dict:
    try:
        commit = capture(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT)
        dirty = bool(capture(["git", "status", "--porcelain"], cwd=ROOT))
    except (OSError, subprocess.CalledProcessError):
        return {"commit": None, "dirty": None}
    return {"commit": commit, "dirty": dirty}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--what", choices=("all", "app", "spike"), default="all",
                        help="Which runnable to build (default: every one that exists)")
    parser.add_argument("--profile", choices=("release", "debug"), default="release")
    parser.add_argument("--output", type=Path, default=ROOT / "target/runnables")
    parser.add_argument("--offline", action="store_true", help="Use cached Cargo dependencies only")
    parser.add_argument("--keep-build-cache", action="store_true",
                        help="Reuse target/build-cache/windows between runs (faster rebuilds)")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--smoke", action="store_true",
                        help="Launch each built exe for a few seconds to check it starts (opens windows)")
    parser.add_argument("--no-vcruntime", action="store_true",
                        help="Do not copy vcruntime140.dll next to the exe")
    options = parser.parse_args()
    if options.jobs < 1:
        parser.error("--jobs must be positive")

    preflight()
    selected = [r for r in RUNNABLES if options.what in ("all", r.key)]
    if options.what == "all":
        selected = [r for r in selected if r.available()]
    for runnable in selected:
        if not runnable.available():
            parser.error(f"'{runnable.key}' does not exist yet ({runnable.manifest.relative_to(ROOT)})")
    if not selected:
        parser.error("Nothing to build")
    ensure_raven()

    output = options.output.resolve() / "windows-x86_64"
    output.mkdir(parents=True, exist_ok=True)
    env = build_environment()
    rustc = capture(["rustc", "--version"], cwd=ROOT)
    scratch_parent = ROOT / "target"
    scratch_parent.mkdir(exist_ok=True)
    built = []
    with tempfile.TemporaryDirectory(prefix="runnable-build-", dir=scratch_parent) as temp:
        scratch = Path(temp).resolve()
        if not scratch.is_relative_to(scratch_parent.resolve()):
            raise RuntimeError("Temporary build directory escaped the workspace target directory")
        source = scratch / "source"
        source.mkdir()
        snapshot(source)
        target_dir = ROOT / "target/build-cache/windows" if options.keep_build_cache else scratch / "target"
        for runnable in selected:
            binary = compile_runnable(runnable, source, target_dir, options, env)
            destination = output / runnable.output_name
            shutil.copy2(binary, destination)
            built.append((runnable, destination))

    runtimes = sorted({name for _, exe in built for name in imported_vcruntimes(exe)})
    bundled = []
    if not options.no_vcruntime:
        for name in runtimes:
            found = find_vcruntime(name)
            if found is None:
                print(f"WARNING {name} not found; friends' PCs will need the VC++ 2015-2022 redistributable",
                      file=sys.stderr, flush=True)
                continue
            shutil.copy2(found, output / name)
            bundled.append(name)
    shutil.copy2(RAVEN_DIR / "LICENSE", output / "RAVEN-LICENSE.txt")
    shutil.copy2(RAVEN_DIR / "NOTICE", output / "RAVEN-NOTICE.txt")

    if options.smoke:
        for _, exe in built:
            smoke(exe, 4.0)

    facts = {
        "target": TRIPLE,
        "profile": options.profile,
        "rustc": rustc,
        "raven": RAVEN_VERSION,
        **git_facts(),
        "vcruntime_bundled": bundled,
        "runnables": {exe.name: {"package": r.package, r.kind: r.target,
                                 "sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
                                 "bytes": exe.stat().st_size}
                      for r, exe in built},
    }
    (output / "BUILD.json").write_text(json.dumps(facts, indent=2) + "\n", encoding="utf-8")
    (options.output.resolve() / "README.txt").write_text(
        "SaveStates x86_64 Windows runnables\n\n"
        "windows-x86_64/savestates.exe        the app (once it exists)\n"
        "windows-x86_64/savestates-spike.exe  Raven spike: drag to pan, Ctrl+wheel to zoom, FPS in the title\n\n"
        "Copy the whole windows-x86_64 folder to another PC; vcruntime140.dll travels with the exe.\n"
        "Needs a graphics driver with Vulkan support (any GPU from roughly 2016 onwards, up to date).\n"
        "Raven is Apache-2.0: see RAVEN-LICENSE.txt and RAVEN-NOTICE.txt.\n"
        "BUILD.json records the commit, toolchain and each exe's SHA-256.\n",
        encoding="utf-8")

    print("\nCreated runnables:", flush=True)
    for _, exe in built:
        print(exe, flush=True)
    if not options.keep_build_cache:
        print("Temporary source snapshot and build output removed.", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"Build failed: {error}", file=sys.stderr)
        raise SystemExit(1)
