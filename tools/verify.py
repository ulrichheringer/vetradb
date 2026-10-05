#!/usr/bin/env python3
"""Single contributor/CI gate. Uses the repository-pinned Rust toolchain unless RUSTUP_TOOLCHAIN overrides it."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
commands = [
    [sys.executable, 'tools/check_docs.py'],
    [sys.executable, 'tools/check_contracts.py'],
    [sys.executable, 'tools/check_workspace.py'],
    [sys.executable, 'tools/verify_foundation.py'],
    ['cargo', 'fmt', '--all', '--', '--check'],
    ['cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings'],
    ['cargo', 'test', '--workspace', '--all-targets', '--locked'],
    ['cargo', 'test', '--workspace', '--doc', '--locked'],
    ['cargo', 'build', '-p', 'vetra-embedded', '--no-default-features', '--locked'],
    ['cargo', 'run', '--locked', '-p', 'vetra-test-support', '--bin', 'fuzz-foundation', '--', '42', '1000'],
    ['cargo', 'run', '--locked', '-p', 'vetra-storage', '--bin', 'fuzz-storage', '--', '42', '200'],
]
if __name__ == '__main__':
    for command in commands:
        print('> ' + ' '.join(command), flush=True)
        subprocess.run(command, cwd=ROOT, check=True)
    print('All local foundation and storage gates passed.')
