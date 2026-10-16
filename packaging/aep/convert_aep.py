"""Convert an AEP to an editable EffectCraft project through the shared importer.

Run with the py-aep Python environment. Set EFFECTCRAFT_CLI to a built CLI path.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def convert(source, output, cli, allow_losses=False, font_search_root=None):
    source = Path(source).resolve()
    output = Path(output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    exporter = Path(__file__).with_name('aep_ir_export.py')
    with tempfile.TemporaryDirectory(prefix='effectcraft-aep-', dir=output.parent) as folder:
        ir = Path(folder) / 'project.json'
        partial = Path(folder) / 'project.ecproj'
        export_args = [sys.executable, str(exporter), str(source), '-o', str(ir)]
        if font_search_root is not None:
            export_args.extend(['--font-search-root', str(Path(font_search_root).resolve())])
        exported = subprocess.run(export_args,
                                  capture_output=True, text=True)
        if exported.returncode:
            raise RuntimeError(exported.stderr.strip() or exported.stdout.strip())
        data = json.loads(ir.read_text())
        if data.get('losses') and not allow_losses:
            raise RuntimeError(f"AEP extraction has {len(data['losses'])} fidelity gaps; no project was written")
        params = json.dumps({'path': str(ir), 'sourceDir': str(source.parent), 'allowLosses': allow_losses})
        imported = subprocess.run([str(cli), 'exec', 'file.importAeIr', '--params', params,
                                   '--empty', '--save-as', str(partial), '--json'],
                                  capture_output=True, text=True)
        if imported.returncode:
            raise RuntimeError(imported.stderr.strip() or imported.stdout.strip())
        reply = json.loads(imported.stdout)
        result = reply.get('result', {})
        losses = data.get('losses', []) + result.get('losses', [])
        if losses and not allow_losses:
            raise RuntimeError(f'EffectCraft import has {len(losses)} fidelity gaps; no project was written')
        if not partial.is_file():
            raise RuntimeError('EffectCraft did not write a project')
        partial.replace(output)
    return {'project': str(output), 'counts': data['counts'],
            'losses': losses, 'missing_media': result.get('missingMedia', [])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    sibling_cli = Path(__file__).with_name('effectcraft-cli')
    parser.add_argument('--cli', default=os.environ.get('EFFECTCRAFT_CLI',
        str(sibling_cli) if sibling_cli.is_file() else 'effectcraft-cli'))
    parser.add_argument('--allow-losses', action='store_true', help='write an inspection project even with reported fidelity gaps')
    parser.add_argument('--font-search-root', type=Path, help='also search this bounded directory for exact matching project fonts')
    args = parser.parse_args()
    print(json.dumps(convert(args.source, args.output, args.cli, args.allow_losses, args.font_search_root), indent=2))


if __name__ == '__main__':
    main()
