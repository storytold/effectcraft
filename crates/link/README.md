# effectcraft-link

The EffectCraft side of Craft Link, the hand-off between FilmCraft and EffectCraft. FilmCraft's
"Edit in EffectCraft" nests the selected clips, writes an OpenTimelineIO file and asks EffectCraft
to open it. This crate imports that file as a comp, keeps the comp bound to the FilmCraft clip,
and hands the result back. The wire protocol is `craft-ipc`; its trust rules are in that crate.
Native only: `cargo xtask wasm` proves that no web crate depends on this one.

| Module | What it holds |
|---|---|
| `receive` | `open_link` answers `link.open`: import the payload, open the comp, bind it, stage the sidecar. A failed sidecar write restores the project, history and editor state from before the call. |
| `attach` | `attach_link` answers `link.attach` for a link that is bound already or whose sidecar is on disk. |
| `back` | `back_to_filmcraft` answers `link.back`: render the result, send `link.focus`, then `app.activate` FilmCraft. |
| `result` | `render_result` encodes the bound comp to the binding's `result_path` as ProRes 4444. Each encode writes its own `.part` file. An older epoch never replaces a newer result (`superseded`). |
| `sidecar` | Keeps `<link_id>.ecproj` current, so `link.attach` reopens the latest edits. |
| `frames`, `track`, `live` | The live link: serve frames of the bound comp while both apps run, count epochs, and render a missing or stale result before the window closes. |
| `activation` | The egui focus backend and the incoming `app.activate` handler. |
| `peer` | The client to the FilmCraft that started the hand-off. |

## Limits

- `link.renderResult` encodes on the UI thread, because the session is not shared with a worker.
  The caller waits for the file. The live path (`live::spawn_render`) encodes in the background.
- Linux cannot raise a window from another process. `app.activate` answers `false` there until
  winit applies activation tokens (rust-windowing/winit#3633). macOS answers `false` as well.
- Closing a link removes the binding. The sidecar and the result movie stay on disk.
