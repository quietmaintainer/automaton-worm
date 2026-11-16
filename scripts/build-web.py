"""Build the static site with Cargo and the matching wasm-bindgen CLI."""

import json
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CLI_VERSION = "0.2.130"
ASSETS = (
    "index.html",
    "style.css",
    "main.js",
    "shared.js",
    "settings.js",
    "view.js",
    "worker.js",
    "autonomous-worker.js",
    "favicon.svg",
)


def main():
    cli = shutil.which("wasm-bindgen")
    if not cli or subprocess.check_output([cli, "--version"], text=True).strip() != f"wasm-bindgen {CLI_VERSION}":
        raise SystemExit(
            f"Install wasm-bindgen {CLI_VERSION} with "
            f"cargo install --locked wasm-bindgen-cli --version {CLI_VERSION}"
        )

    # Read the artifact path from Cargo so CARGO_TARGET_DIR and config overrides
    # still work on all platforms.
    command = [
        "cargo", "build", "--locked", "--release", "--lib", "--target",
        "wasm32-unknown-unknown", "--message-format=json-render-diagnostics",
    ]
    artifact = None
    with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, text=True, encoding="utf-8") as cargo:
        for line in cargo.stdout:
            message = json.loads(line)
            if message["reason"] == "compiler-artifact" and message["target"]["name"] == "automaton_worm":
                artifact = next((path for path in message["filenames"] if path.endswith(".wasm")), artifact)
        if cargo.wait():
            raise SystemExit(cargo.returncode)
    if not artifact:
        raise SystemExit("Cargo did not produce the WebAssembly library")

    dist = ROOT / "dist"
    # Complete the build before replacing an existing preview.
    with tempfile.TemporaryDirectory(prefix=".site-", dir=ROOT) as scratch:
        site = Path(scratch) / "dist"
        site.mkdir()
        subprocess.run([cli, "--target", "web", "--out-dir", site / "pkg", "--out-name", "worm",
                        "--no-typescript", artifact], check=True)
        for name in ASSETS:
            shutil.copyfile(ROOT / "web" / name, site / name)
        for name in ("LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"):
            shutil.copyfile(ROOT / name, site / name)
        (site / ".nojekyll").touch()
        if dist.is_symlink() or dist.resolve() != dist:
            raise SystemExit("dist must be a local directory")
        if dist.exists():
            shutil.rmtree(dist)
        site.rename(dist)
    print(f"Static site ready in {dist}")


if __name__ == "__main__":
    main()
