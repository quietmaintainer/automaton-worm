"""Verify the extracted arena against the frozen source compiled-table replays."""

import hashlib
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CASE_BYTES = 100 * 100 * 100 * 9 * 4


def main():
    fixture = ROOT / "tests" / "fixtures" / "compiled-table-sha256.txt"
    command = ["cargo", "run", "--release", "--locked", "--example", "replay"]
    with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE) as replay:
        cases = 0
        for line in fixture.read_text(encoding="ascii").splitlines():
            seed, mode, expected = line.split()
            remaining = CASE_BYTES
            digest = hashlib.sha256()
            while remaining:
                block = replay.stdout.read(min(remaining, 1 << 20))
                if not block:
                    raise SystemExit(f"Incomplete replay for {seed} {mode}")
                digest.update(block)
                remaining -= len(block)
            if digest.hexdigest() != expected:
                replay.kill()
                raise SystemExit(f"Compiled-table baseline differs for {seed} {mode}")
            cases += 1
        if replay.stdout.read(1):
            replay.kill()
            raise SystemExit("Replay produced data beyond the frozen cases")
        if replay.wait() or cases != 192:
            raise SystemExit("Replay verification failed")
    print(f"{cases} seed/mode replays match through 100 ticks, including both engine switches.")


if __name__ == "__main__":
    main()
