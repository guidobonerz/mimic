#!/usr/bin/env python3
"""Enforce the workspace dependency direction (CLAUDE.md golden rules 1 and 10).

Reads every member Cargo.toml directly (no cargo needed) and fails if a crate
depends on an internal crate its layer may not see, or if an emulation crate
pulls in a host crate (windowing, gamepads, audio, async runtime).

Usage: python tools/check-deps.py [repo-root]
"""
import fnmatch
import pathlib
import sys
import tomllib

# Crate-name pattern -> internal crates it may depend on (patterns allowed).
# First matching rule wins; a workspace crate with no rule is an error.
FRONTEND_DEPS = ["emu-core", "emu-machines", "emu-debug", "emu-preview", "video-crt",
                 "emu-input", "media-backends"]
RULES = [
    ("emu-core", []),
    ("cpu-*", ["emu-core"]),
    ("chips-*", ["emu-core"]),
    ("media", ["emu-core"]),
    ("devices-input", ["emu-core"]),
    ("video-crt", ["emu-core"]),
    ("emu-input", ["emu-core"]),
    ("media-backends", ["emu-core"]),
    ("drives-*", ["emu-core", "cpu-*", "chips-*", "media"]),
    ("machine-*", ["emu-core", "cpu-*", "chips-*", "drives-*", "devices-input", "media"]),
    # Also the catalogue of pluggable port devices for the frontends.
    ("emu-machines", ["emu-core", "machine-*", "devices-input"]),
    ("emu-debug", ["emu-core"]),                   # golden rule 1
    ("emu-preview", ["emu-core", "video-crt"]),    # golden rule 1
    ("emu-desktop", FRONTEND_DEPS),
    ("emu-web", FRONTEND_DEPS),
    ("emu-headless", FRONTEND_DEPS),
    ("emu-tests", ["*"]),
]

# Golden rule 10: emulation crates never see host devices.
HOST_CRATES = {"gilrs", "winit", "wgpu", "egui", "cpal", "tokio", "tokio-tungstenite"}
HOST_FREE = ["emu-core", "cpu-*", "chips-*", "drives-*", "devices-*", "media", "machine-*"]

DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def matches(name, patterns):
    return any(fnmatch.fnmatchcase(name, p) for p in patterns)


def dep_names(manifest):
    """Yield the package names of all dependencies, including target-specific ones."""
    tables = [manifest]
    tables += manifest.get("target", {}).values()
    for table in tables:
        for kind in DEP_TABLES:
            for key, spec in table.get(kind, {}).items():
                yield spec.get("package", key) if isinstance(spec, dict) else key


def members(root):
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    for pattern in workspace["members"]:
        for path in sorted(root.glob(pattern)):
            if (path / "Cargo.toml").is_file():
                yield tomllib.loads((path / "Cargo.toml").read_text(encoding="utf-8"))


def main():
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    manifests = list(members(root))
    internal = {m["package"]["name"] for m in manifests}
    errors = []

    for manifest in manifests:
        name = manifest["package"]["name"]
        allowed = next((deps for pattern, deps in RULES if fnmatch.fnmatchcase(name, pattern)), None)
        if allowed is None:
            errors.append(f"{name}: no layer rule in tools/check-deps.py")
            continue
        for dep in sorted(set(dep_names(manifest))):
            if dep in internal and not matches(dep, allowed):
                errors.append(f"{name} -> {dep}: not allowed by dependency direction")
            if dep in HOST_CRATES and matches(name, HOST_FREE):
                errors.append(f"{name} -> {dep}: host crate in emulation layer (golden rule 10)")

    for e in errors:
        print(f"error: {e}")
    print(f"checked {len(manifests)} crates, {len(errors)} violation(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
