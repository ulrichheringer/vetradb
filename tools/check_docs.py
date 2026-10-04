#!/usr/bin/env python3
"""Check repository-local Markdown links; network URLs are not fetched."""
import re
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]


def check(root=ROOT):
    errors = []
    files = [root / 'README.md', root / 'CONTRIBUTING.md', root / 'GOVERNANCE.md', root / 'SECURITY.md']
    files += sorted((root / 'docs').rglob('*.md'))
    for path in files:
        for target in re.findall(r'\]\(([^)]+)\)', path.read_text()):
            if re.match(r'^[a-zA-Z][\w+.-]*:', target) or target.startswith('#'):
                continue
            target = unquote(target.split('#')[0].strip('<>'))
            if target and not (path.parent / target).exists():
                errors.append(f'{path.relative_to(root)}: missing {target}')
    if errors:
        raise ValueError('\n'.join(errors))
    return len(files)


if __name__ == '__main__':
    print(f'Local links checked in {check()} Markdown documents.')
