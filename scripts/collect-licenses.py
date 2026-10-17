#!/usr/bin/env python3
"""Collect the notices for all locked crates in this target's dependency graph."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

output = Path(sys.argv[1])
output.mkdir(parents=True, exist_ok=True)
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--locked', '--format-version=1',
    '--filter-platform=x86_64-unknown-linux-gnu'
]))
resolved = {node['id'] for node in metadata['resolve']['nodes']}
summary = ['# Rust dependency licenses', '', '| Crate | License | Source |', '| --- | --- | --- |']
for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
    if package['id'] not in resolved or package['source'] is None:
        continue
    name = package['name'] + '-' + package['version']
    summary.append(f"| {name} | {package.get('license') or 'See notices'} | {package.get('repository') or 'https://crates.io/crates/' + package['name']} |")
    root = Path(package['manifest_path']).parent
    for source in root.rglob('*'):
        if not source.is_file() or source.is_symlink():
            continue
        filename = source.name.upper()
        if not filename.startswith(('LICENSE', 'COPYING', 'COPYRIGHT', 'NOTICE')):
            continue
        destination = output / name / source.relative_to(root)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
(output / 'DEPENDENCIES.md').write_text('\n'.join(summary) + '\n')
print(f'Collected dependency notices in {output}')
