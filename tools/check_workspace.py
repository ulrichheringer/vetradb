#!/usr/bin/env python3
"""Enforce declared crate graph, runtime-free embedded closure and reviewed locked dependencies."""
import json
import tomllib
from pathlib import Path
from verify_foundation import graph_check

ROOT = Path(__file__).resolve().parents[1]


def check():
    graph = json.loads((ROOT / 'docs/specs/modules.json').read_text())
    graph_check(graph)
    actual = {path.parent.name for path in (ROOT / 'crates').glob('*/Cargo.toml')}
    if actual != set(graph):
        raise ValueError('workspace/module inventory mismatch')
    inventory = json.loads((ROOT / "docs/specs/dependencies.json").read_text())
    for name, dependencies in graph.items():
        manifest = tomllib.loads((ROOT / f'crates/{name}/Cargo.toml').read_text())
        declared = manifest.get('dependencies', {})
        if set(declared) != {f'vetra-{dep}' for dep in dependencies} | set(inventory['direct'].get(name, {})):
            raise ValueError(f'{name}: dependency policy mismatch')
        for dep in dependencies:
            if declared[f'vetra-{dep}'] != {'path': f'../{dep}'}:
                raise ValueError(f'{name}: external/features dependency needs reviewed inventory')
        for dep, options in inventory['direct'].get(name, {}).items():
            if declared.get(dep) != options:
                raise ValueError(f'{name}: unreviewed external options')
        if manifest.get('dev-dependencies') or manifest.get('build-dependencies'):
            raise ValueError(f'{name}: undeclared test/build dependency')
        if manifest.get('lints') != {'workspace': True}:
            raise ValueError(f'{name}: unsafe/lint policy override')
        package = manifest['package']
        for key in ('version', 'edition', 'rust-version', 'license', 'repository', 'publish'):
            if package.get(key) != {'workspace': True}:
                raise ValueError(f'{name}: {key} must inherit workspace policy')
        if package.get('name') != f'vetra-{name}':
            raise ValueError('crate name does not match graph')
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    expected = {(p['name'], p['version'], p['source'], p['checksum']) for p in inventory['packages']}
    actual_external = {(p['name'], p['version'], p['source'], p['checksum']) for p in lock['package'] if p.get('source')}
    if expected != actual_external:
        raise ValueError('external lockfile drift from reviewed inventory')
    return len(actual)


if __name__ == '__main__':
    print(f'Workspace policy: {check()} crates; reviewed locked dependency inventory.')
