# ADR 0003: Hardware video in a helper process

- Status: proposed (an RFC; a maintainer asked for one on the ArtCraft Discord, 2026-11-13)
- Date: 2026-11-13
- Issues: [#805], [#498], [#804], [#3426], [#3624], [#2240]
- Earlier attempts: [#819] (closed), [#1840] (open)
- Builds on: [ADR 0001](0001-external-effect-helpers.md) (external effect helpers)

## In short

- Exports are slow because all video decoding and encoding runs on the CPU. The GPU's built-in
  video engine is never used.
- Using it means calling the graphics driver. A driver bug can crash the app and lose unsaved
  work, so EffectCraft has kept driver code out.
- Proposal: run the driver code in a **separate helper program**, the way ADR 0001 already runs
  native plug-ins. If the driver crashes, only the helper stops; EffectCraft keeps running and
  falls back to its own encoder.
- It is opt-in, and nothing changes for anyone who doesn't turn it on.

## The problem

Every frame of an export is decoded, composited and encoded. Only compositing can use the GPU.
Decoding and encoding always run on the CPU, with EffectCraft's own codecs, so on long exports the
CPU is maxed out while the GPU waits. Users with strong GPUs report this ([#3426], [#3624]).

Every modern GPU has a video engine built for exactly this job: VCN on AMD, NVENC / NVDEC on
NVIDIA, Quick Sync on Intel, and the media engine on Apple silicon.

How much it could help:

| Measurement | Result |
| --- | --- |
| Our CPU cost of a 1080p30 H.264 export (12-thread laptop, `main`, 2026-11-13) | ≈ 425 ms per frame: ≈ 165 ms encoding, ≈ 130 ms decoding the source |
| Hardware decoding ([#819]) | 5–31× less CPU per frame, identical pictures |
| 4K HEVC export, software encoder on a Mac ([#1840]) | 0.23 frames per second |
| Same export, Mac hardware encoder ([#1840]) | 15.3 frames per second |
| Same export, NVIDIA RTX 3090 / AMD RX 5500 ([#1840]) | 11.6 / 5.0 frames per second |

## Why it isn't done yet

FilmCraft already uses the video engine. Its [ADR 0001](https://github.com/storytold/filmcraft/blob/main/docs/adr/0001-platform-ffi.md)
allows `unsafe` code in one crate, `filmcraft-platform`, which supports hardware decoding on every
desktop platform and hardware encoding on Mac and NVIDIA. AMD encoding on Linux is still in review
([storytold/filmcraft#1612]).

[#819] tried to bring that crate into EffectCraft. It was declined to keep driver code out of the
app, for the same reason as [#498]: a crash inside a driver can't be caught, and the user loses
their work. [#1840] takes the same approach and is waiting on a rebase.

EffectCraft already faced this problem with native plug-ins, and solved it in ADR 0001 by running
them in a separate process. This ADR proposes the same for hardware video.

## The proposal

- **D1 No driver code inside EffectCraft.** A helper program, `effectcraft-helper-video`, talks
  to the driver. EffectCraft starts it and sends it work. EffectCraft itself keeps
  `unsafe_code = "forbid"`. The helper is built from FilmCraft's existing code and released
  separately from the app.
- **D2 Reuse the plug-in helper system.** The helper is found, started, timed out, restarted and
  blocklisted exactly like effect helpers (ADR 0001). Only new message types for video are added.
- **D3 Plug in at two points.** Exports get a helper-backed encoder next to the built-in ones.
  Footage decoding gets a helper-backed decoder next to FilmCraft's.
- **D4 Opt-in.** Each output module gets a **Hardware Encoding: Off / Auto** setting, Off at
  first. Auto uses the helper only when it is installed and supports the export settings;
  otherwise the built-in encoder runs and the render log says why. Default exports stay exactly as
  they are today.
- **D5 Fail gracefully.** No helper problem can stop the app or harm the project:
  - before using the hardware, ask the driver what it supports, and skip it if unsupported;
  - if the helper crashes, hangs or misbehaves, it is stopped. Decoding falls back to the
    built-in decoder for that frame. An export restarts with the built-in encoder, and the log
    says so;
  - after 3 crashes the helper is turned off until the user retries in Settings;
  - hardware-decoded pictures must match the built-in decoder's, or that file isn't decoded in
    hardware.
- **D6 Encoding first.** Exports spend the most CPU on encoding, and it is the simplest to
  connect: frames go one way and there is no seeking.

## Plan

| Phase | What | Notes |
| --- | --- | --- |
| 1 | H.264 / HEVC hardware encoding | Mac and NVIDIA code exists in FilmCraft; AMD on Linux is in [storytold/filmcraft#1612] |
| 2 | Hardware decoding, for export and preview | Code and measurements exist in [#819] |
| 3 | AMD encoding on Windows | Not in FilmCraft yet (Media Foundation or AMF) |

## Costs and trade-offs

- **Nothing changes without the helper.** The app, the web build and default exports stay the
  same.
- **A copy per frame.** Each frame is passed to the helper and back: about 3 MB at 1080p, 12 MB at
  4K. Phase 1 measures this; it should be small next to the 165 ms of encoding it saves.
- **Output varies by machine.** Hardware encoders produce different files on different GPUs and
  drivers, and often lower quality at the same file size. The render log records which encoder,
  GPU and driver were used.
- **More to release and test.** The helper needs its own builds and signing on each platform. CI
  can test the helper system with a stand-in helper, but not real GPUs, so a manual test list
  (NVIDIA, AMD and Intel on Windows and Linux, plus Apple silicon) goes in `docs/testing.md`.

## Other options considered

| Option | Why not (for now) |
| --- | --- |
| `filmcraft-platform` inside the app ([#819], [#1840]) | Simplest and fastest, but declined in [#819]. If maintainers prefer it after all, [#1840]'s settings and CLI work can be reused |
| Our own GPU encoder in shaders | No driver code, but most encoding steps can't run in parallel, so it's months of work for a small gain |
| ffmpeg as a separate program | AGENTS.md allows ffmpeg only for tests, never shipped |
| Doing nothing | Exports stay CPU-bound and strong GPUs stay idle |

Separately, and without any driver code: a Render Queue job uses either the GPU or the CPU
compositor, never both, so one of them idles. Using both at once is a separate improvement that
can go ahead on its own.

## Technical notes

- **Messages (sketch).** `hello` lists what the machine supports. Encoding:
  `encode.open {codec, width, height, rate, bitrate, colour}` → `encode.frame` → packets →
  `encode.flush` → `encode.close`. Decoding: `decode.open {codec, sample entry}` →
  `decode.sample` → picture. Pictures and packets travel through ADR 0001's frame exchange file,
  never through the pipe. The full protocol would go in `docs/external-helpers.md`.
- **Encoder seam.** `crates/export/src/encode.rs`, where `movie` picks the encoder. H.264 frames
  already arrive there as 4:2:0 planes, so the helper gets exactly what the built-in encoder gets.
- **Decoder seam.** `MediaPool` in `crates/media/src/pool.rs`, where it opens FilmCraft's decoder
  today.
- **Limits.** The crash limit and timeouts are the effect-helper ones (`effectcraft-helper-host`).
  Export settings the hardware can't handle (for example HDR, odd sizes or non-square pixels) are
  declined up front, as FilmCraft does.

## Questions for the maintainers

1. Is a separate helper process the right approach, or would you rather have it inside the app?
2. Can the installer include the helper, or should it be a separate optional download?
3. Where should the helper's code live (FilmCraft, craft-libs or its own repository), and who
   builds and signs it?
4. If the helper fails during an export, should the export restart with the built-in encoder
   (D5), or stop with an error, as FilmCraft does?
5. Encoding first (D6), or decoding first, since [#819]'s code already exists?
6. Hardware Encoding Off by default, or Auto once the manual tests pass?
7. How should this work together with [#1840] and its author?

[#498]: https://github.com/storytold/effectcraft/issues/498
[#804]: https://github.com/storytold/effectcraft/issues/804
[#805]: https://github.com/storytold/effectcraft/issues/805
[#819]: https://github.com/storytold/effectcraft/pull/819
[#1840]: https://github.com/storytold/effectcraft/pull/1840
[#2240]: https://github.com/storytold/effectcraft/issues/2240
[#3426]: https://github.com/storytold/effectcraft/issues/3426
[#3624]: https://github.com/storytold/effectcraft/issues/3624
[storytold/filmcraft#1612]: https://github.com/storytold/filmcraft/pull/1612
