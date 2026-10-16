# After Effects project import

`file.importAep` runs the bundled `aep_ir_export.py` through Python, then imports its
versioned JSON into native EffectCraft project objects. The resulting `.ecproj`
does not require After Effects or Python to open or edit. The importer requires
Python 3.11 or newer with [`py-aep`](https://github.com/forticheprod/py-aep) installed while
converting; the SceneYard acceptance corpus uses py-aep commit
`e12a451c35bacd3f34a080090265f9370e66162b` (MIT).

Set `EFFECTCRAFT_AEP_PYTHON` to the Python interpreter that has py-aep installed.
Set `EFFECTCRAFT_AEP_FONT_ROOT` when fonts shared by multiple projects live
outside an AEP's own folder; the exporter searches that directory for exact
PostScript-name and version matches. The standalone converter accepts the same
directory through `--font-search-root`.
The pinned dependency is in `requirements.txt` for a separate import-time venv.
Automatic discovery is limited to `Contents/Resources/aep_ir_export.py` inside
a macOS `.app` bundle. Development checkouts and other platforms must set
`EFFECTCRAFT_AEP_EXPORTER` explicitly. Parent folders are never searched.
Conversion JSON is read through a bounded pipe (512 MiB maximum), without a
shared temporary file. The exporter is also usable directly:

```sh
EFFECTCRAFT_CLI=/path/to/effectcraft-cli python3 packaging/aep/convert_aep.py source.aep source.ecproj

# Lower-level data inspection:
python3 packaging/aep/aep_ir_export.py source.aep -o source.aep.json
effectcraft-cli exec file.importAeIr --params '{"path":"source.aep.json"}' --empty --save-as source.ecproj
```

Imports reject any reported fidelity loss by default. Use `allowLosses: true`
only to inspect an incomplete import while implementing a missing native feature.
Missing footage remains an editable offline item with its original path so it
can be relinked later.

Layered Illustrator/PDF footage retains its saved source layer and fractional
artwork bounds (`crop_bounds`); rendering the cropped artwork is not part of this
importer yet.

Opening and editing every corpus project is a structural check, not visual
acceptance. Unsupported Puppet state, model lighting/cameras, effects and fonts
still require separate validation against reference frames.
