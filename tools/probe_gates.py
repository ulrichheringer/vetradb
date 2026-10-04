#!/usr/bin/env python3
"""Demonstrate behavioral/format/link gate failures in a disposable local Git branch."""
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(command, directory, should_fail=False):
    result = subprocess.run(command, cwd=directory, text=True, capture_output=True)
    if (result.returncode != 0) != should_fail:
        raise RuntimeError(f'unexpected exit {result.returncode}: {command}\n{result.stdout}\n{result.stderr}')
    return result


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='vetra-gates-') as temp:
        directory = Path(temp)
        for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'README.md', 'CONTRIBUTING.md', 'GOVERNANCE.md', 'SECURITY.md']:
            shutil.copy2(ROOT / name, directory / name)
        for name in ['crates', 'docs', 'tools']:
            shutil.copytree(ROOT / name, directory / name, ignore=shutil.ignore_patterns('__pycache__'))
        run(['git', 'init', '--quiet', '--initial-branch=verification-probe'], directory)
        source = directory / 'crates/types/src/lib.rs'
        original = source.read_text()
        source.write_text(original + '\n#[cfg(test)]\nmod injected_defect {\n    #[test]\n    fn zero_identity_must_be_accepted() {\n        assert!(super::TransactionId::new(0).is_some());\n    }\n}\n')
        failure = run(['cargo', 'test', '-p', 'vetra-types', '--locked'], directory, should_fail=True)
        if 'injected_defect::zero_identity_must_be_accepted' not in failure.stdout:
            raise RuntimeError('test gate failed for the wrong reason')
        source.write_text(original + '\npub fn unformatted(  )->u64{1}\n')
        run(['cargo', 'fmt', '--all', '--', '--check'], directory, should_fail=True)
        source.write_text(original)
        readme = directory / 'README.md'
        readme.write_text(readme.read_text() + '\n[Injected missing document](docs/does-not-exist.md)\n')
        failure = run([sys.executable, 'tools/check_docs.py'], directory, should_fail=True)
        if 'does-not-exist.md' not in failure.stderr:
            raise RuntimeError('link gate failed for the wrong reason')
        print('Disposable branch: behavioral defect, formatting defect and broken link were all rejected.')
