# Linked compositions: proposed first version

This prototype treats a composition in another saved EffectCraft project as one footage-like
item. It is intended for testing the link, save, refresh and render workflow locally. It does
not yet provide the full linked-composition feature.

The destination owns each layer instance: its timing, placement, masks, effects and opacity.
The source project owns the composition's layers and animation. The source timeline cannot be
opened through the destination's linked item, and the source's dependency items are not added
to the destination's Project panel. Rendering loads the source in its own project context.

Several compositions from the same source project can be linked as separate items. Refreshing
that source updates its linked items together from the same saved source snapshot. Each linked
item may be placed in the destination timeline more than once.

New linked items use the Brown label by default. Labels remain user-editable; existing items
keep their saved labels. Linked items and their timeline layers show a composition icon with
a chain-link badge, independently of their label color.

## Trying the desktop workflow

Build the sandbox binaries in a disposable checkout, then generate the two-project demo:

```powershell
cargo build -p effectcraft -p effectcraft-cli
cargo test -p effectcraft-engine --lib linked_compositions_two_saved_sources_refresh_without_importing_their_layers
```

The test replaces its generated `Titles.ecproj`, `Main_Show.ecproj`, `before.png`, and `after.png`
in `target/test-out/linked-compositions-demo`. Keep artist-edited demo files in another folder.
To try the desktop workflow, open the projects in
two sandbox windows, edit and save the source, then select a linked item in the destination's
Project panel and choose **File ▸ Reload Footage** (Ctrl+Alt+L on Windows, Cmd+Option+L on macOS).
The linked item's context menu has the same Reload Footage action. It refreshes every linked
composition from that source project together. A destination timeline layer selected instead
of a Project item can also identify the linked source.

```powershell
.\target\debug\effectcraft.exe --control 9881 target/test-out/linked-compositions-demo/Main_Show.ecproj
.\target\debug\effectcraft.exe --control 9882 target/test-out/linked-compositions-demo/Titles.ecproj
.\target\debug\effectcraft-cli.exe exec file.reloadFootage --bridge 9881 --params '{}'
```

Launch each window separately (or use a separate terminal) so both remain open. Use the sandbox
build to edit the source as well, so its durable identities are preserved.

To create a link, choose **File ▸ Import ▸ Linked EffectCraft Composition…**, pick a saved source
project, then select its compositions in the picker. Compositions already linked anywhere in
the destination project show **Already linked** and cannot be selected again. This status uses
the source project and composition identities, so renames do not hide existing links. Linking
several selected compositions creates their Project items in one undo step; it does not place
layers in the active timeline. The separate **EffectCraft Project…** import still makes copies.

## Command workflow

Save a source project with two compositions, such as `Opening Title` and `End Credits`. Then
open or create a destination project and run these engine commands through the CLI or a
headless automation session. Replace the paths with absolute paths in your sandbox.

```json
{"command":"file.linkComposition","params":{"path":"/sandbox/Titles.ecproj","comp":"Opening Title"}}
{"command":"file.linkComposition","params":{"path":"/sandbox/Titles.ecproj","comp":"End Credits"}}
{"command":"project.linkedCompositions","params":{}}
```

Each link command returns its destination item ID. Place that item with the existing
`layer.addItem` command, using the destination composition ID:

```json
{"command":"layer.addItem","params":{"comp":1,"item":2,"time":0}}
```

For a picker or batch import, `file.linkableCompositions {path}` lists the saved source's stable
composition IDs, metadata and `alreadyLinked` status. Pass selected IDs as
`file.linkComposition {path, comps: [stableId, ...], sourceProjectId}`. The batch validates the
whole selection before making one edit, and reuses existing links. The optional source project
ID guards against the file being replaced while a picker is open.

The IDs above are examples; use the IDs returned by your own commands. Save the destination,
change and successfully save the source, then reload the linked item:

```json
{"command":"file.reloadFootage","params":{"item":2}}
```

Refresh reads saved contents only. Unsaved edits in another editor are not shared. The linked
item keeps its identity in the destination, so existing destination layers keep their instance
settings. `project.linkedCompositions` reports the source identity and any refresh error. The
separate `project.refreshLinkedCompositions {}` command remains available to refresh all links
without selecting an item. A mixed selection of linked compositions and ordinary footage
refreshes links and reloads ordinary media through their respective loaders.

The JSON objects above can be saved as a JSON Lines command file and run from this checkout with
`cargo run -p effectcraft-cli -- run --project /sandbox/Main_Show.ecproj --commands steps.jsonl --save`.
Keep each sequence that edits the same destination in one invocation, or save it before the
next invocation.

## Color interpretation

A linked item is automatically tagged with the source project's working color space and
whether its working pixels are linear light. The source composition is rendered with its
own color settings, then converted into the destination's working space before destination
layer effects and compositing. The source's display/output transform and HDR tone mapping
are not baked into the link; the destination applies its own display or export treatment.

Built-in managed projects and OCIO projects using the existing built-in ACES configuration
can therefore link across different working spaces. For example, a linear ACEScg source can
be used in a managed Rec. 709 destination. This does not add arbitrary external OCIO
configuration support.

An unmanaged source has no authoritative working-space tag. Its existing renderer convention
is shown explicitly as an **assumption**, rather than as a detected profile. An offline source
shows its last known interpretation. Interpret Footage selections containing both linked and
ordinary items expose their shared color controls; **Keep individual settings** leaves each
item's current profile, encoding and Preserve RGB setting intact.

Select the linked item and choose **Interpret Footage ▸ Main…** to inspect its automatic tag
or assign a different profile and encoding. **Automatic source color space** follows future
source saves and Reload Footage. An assigned profile is a destination-side override: it
persists when the source refreshes and applies to every timeline instance of that linked
item. Choosing Automatic again removes the override. ACES working spaces are inherently
linear, so their encoding checkbox remains checked.

Assigning a profile changes what the incoming numbers mean; it does not change the source
project. **Preserve RGB** bypasses the boundary conversion for data-like values. Source
bit-depth quantization remains part of the source render, and an integer-depth destination
can still clip or quantize values that a float destination retains.

## Identity and compatibility proposal

This is the proposed contract for maintainer review, not an agreed file-format migration:

- A project and each local item receive independent opaque 128-bit IDs. Numeric item IDs and
  display names remain local addresses and presentation; neither is the external identity.
- Native builds use operating-system entropy. Browser builds use Web Crypto through
  `getrandom`'s `wasm_js` backend. If entropy is unavailable, authoring/save/link commands
  report an error instead of persisting an empty or predictable identity.
- Opening an older project assigns missing IDs in memory before its first undo snapshot.
  A successful save makes them durable. An older source must therefore be opened and saved
  by this build before it is linkable; a destination never modifies a source to migrate it.
- Rename, Save As and increment-save preserve identities. They represent continued versions
  of the same project. The stored source path still chooses the file to read; refresh never
  searches the filesystem by identity or silently follows another copy.
- New Project (including a template) creates new project/item identities. Import EffectCraft
  Project creates new local item identities. There is no separate Fork Project command yet.
- Divergent Save As copies can share identities. Worker deduplication therefore compares
  actual snapshot content, not only IDs. The batch picker treats matching project/comp IDs
  as already linked; intentionally linking divergent copies is outside that picker workflow.
- Duplicate item IDs are repaired when an editable project is opened/saved. A read-only
  linked source with ambiguous composition IDs is refused until saved by a compatible build.
- The fields are additive to schema 2. An older writer may discard them, which breaks identity
  continuity. Until maintainers settle the migration/versioning policy, edit linked sources
  with this build and keep a backup. No forward-compatibility guarantee is claimed.

Before merge, maintainers should confirm Save As continuity versus an explicit fork command,
the old-writer policy/schema version, and whether source identity belongs in the shared project
format crate. Coordinate that decision with the adjacent
[Craft Link proposal](https://github.com/storytold/effectcraft/pull/3138), especially project
identity, command IDs and render-worker snapshots. This feature does not implement Craft Link's
cross-application workflow.

## Prototype boundaries

- Refresh is manual; automatic desktop watching is not implemented.
- Sources containing another project link are refused, including cyclic project links.
- Source projects are limited to 16 MiB each, 4,096 items and 8,192 layers. A refresh handles
  at most eight source projects and 64 linked compositions; nested local comps are limited
  to 12 levels and a linked composition is limited to 16 megapixels.
- Worker messages contain one snapshot per distinct saved source, with a 64 MiB total message
  limit and the same per-source/count limits. Invalid references and nested transport envelopes
  are refused before decoding source models.
- Linked items are flattened frame sources. They do not expose source layers or support
  Collapse Transformations across the project boundary.
- Relinking and Make Local are follow-up work.
- Saving uses a relative source path when the source is inside the destination project's
  folder; other source paths remain absolute. Moving a source independently can make the
  link offline. Richer relinking and recovery controls are follow-up work.
- Linked items expose color interpretation through Interpret Footage. Timing, alpha,
  Remember/Apply Interpretation and proxy operations remain source-controlled or unsupported.
  Change source contents in the source project and use Reload Footage. Projects containing
  links currently refuse Replace Footage, Relink, Collect Files, template export and
  interchange export, which do not yet preserve the complete external dependency graph.
- Saving an EffectCraft project keeps only its links. Render-worker transport carries the
  resolved source snapshots separately, so a running job uses the source generation captured
  when it started.
- Save, Save As and increment-save refuse to overwrite a linked source project or its known
  media/proxies/sequences, including existing filesystem aliases and missing source paths.
  Ordinary saves to the destination's own project file remain supported. As with the existing
  output guard, an external filesystem change between checking and writing is not atomic.
  Known dependencies remain protected after a failed refresh in that session. If a destination
  is reopened while its source cannot be read, only the source project path is known and
  protected; the reference-only file does not inventory unavailable source media.
- Compositions that can read a linked item conservatively bypass frame/layer caching. Unused
  linked assets do not disable caching for unrelated compositions. Fine-grained linked-frame
  keys and persistent caching remain performance work.
- The native movie decoder scheduling fix prevents the reproduced rapid-seek/parallel-export
  deadlock. Its validation includes a 512-jump linked preview test and ten default-parallel
  video/audio exports; see the [preview follow-up](linked-preview-stall.md). Unrelated full-CI
  failures and the identity migration review remain recorded in the
  [validation notes](linked-compositions-validation.md).
- Source identity is checked independently from the composition's displayed name. Sources
  need to be saved by this build before they can be linked.
- Source-project reads use the session's file services and input limits. Automation read roots
  still apply to source projects and any supported source media.
- Confined automation supports this prototype in headless mode. In confined bridge mode,
  source inspection, linking and linked-source reloads or refreshes are refused; `file.open` disables linked-source
  resolution in the connected desktop session and refuses linked documents. Open Recent and Revert are refused
  there too; use an explicit `file.open` path for ordinary projects.

The engine rejects unsupported source configurations explicitly. Check command errors and
`project.linkedCompositions` before using a prototype link for final output. This document and
the sandbox tests describe the supported subset; they do not establish production support for
every project, media dependency, worker renderer or browser configuration.

## Running the sandbox regression tests

```sh
cargo test -p effectcraft-engine --lib linked_compositions
cargo test -p effectcraft-render --lib linked_composition
cargo test -p effectcraft-automation --lib confined_
```

The tests construct their source projects and media locally. They do not require third-party
project files or changes to the installed application.
