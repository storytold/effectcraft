# Video helper protocol (proposed)

**Status: proposed.** This page specifies how EffectCraft talks to a hardware video helper, as
decided in [RFC-0031][rfc-0031] (hardware video in a helper process). The host side is in
`crates/helper-host` (`video`), tested against a stand-in helper; there is no real helper yet and
export does not use it. Phase 1 covers **encoding**; decoding is reserved for phase 2
([decoding](#decoding-phase-2)). Where this page is more specific than the RFC or differs from it,
[Differences from RFC-0031](#differences-from-rfc-0031) lists it.

A **video helper** is an [external helper](external-helpers.md) that offers hardware video
encoding instead of effects. It reuses that page's process model: the helper is a child process,
requests and replies are JSON lines, pixels and packets go through the frame exchange file, a
helper that crashes, hangs or breaks the protocol is stopped, and one that does so 3 times is
turned off. This page covers only what is different for video.

## Starting and finding it

- **The user decides.** Starting a video helper needs the same consent as an effect helper (see
  [Using helpers](external-helpers.md#using-helpers)). Opening a project whose output module says
  Hardware Encoding: Auto does not start it. Confined agents and scripts without write and network
  permission cannot start it either.
- **Only the app's own install folder.** The program is `effectcraft-helper-video`, found in
  `helpers/` next to the EffectCraft executable (`Contents/Helpers` in a macOS bundle). It is not
  looked for in the settings folder, in `EFFECTCRAFT_HELPER_PATH`, in `PATH` or in the current
  folder. Effect-helper scans skip it.
- **Checked before any work.** EffectCraft reads the `hello` reply (below) first and sends nothing
  else until the versions fit.

## `hello`

EffectCraft adds `"video": 1` to its `hello`: the highest video protocol version it speaks. The
helper answers with the usual fields plus a `video` object:

```json
{"op":"hello","version":1,"name":"Hardware video","helper_version":"0.1.0","loads":[],
 "video":{"version":1,"app_min":"0.25.0","devices":[
   {"id":"nvenc:0","name":"NVIDIA GeForce RTX 3090","api":"nvenc","driver":"NVIDIA 570.86",
    "encode":[{"codec":"h264","profiles":["high"],"bit_depths":[8],"min_size":[64,64],
               "max_size":[4096,4096],"rate_control":["bitrate"]}],
    "decode":[]}]}}
```

- `video.version` is the video protocol version the helper will use. If EffectCraft does not speak
  it, or is older than `app_min`, it does not use the helper, and the message names the installed
  helper version and the version to install.
- `devices` lists every GPU the helper can use, each with a stable `id`, the `api` it reaches it
  through (`vaapi`, `nvenc`, `videotoolbox`, or later `mediafoundation`), the `driver`, and what
  it can `encode` and `decode`. Two GPUs that both offer H.264 are both listed; EffectCraft picks
  one and its render log says which. The rule for choosing is still open in RFC-0031.
- The helper answers `describe` with no effects.
- **Start-up order.** The helper starts its hardware APIs (CUDA, VA-API and others) on one thread,
  in a fixed order, before it answers `hello`. Starting NVENC and VA-API on two threads at once
  can deadlock with NVIDIA's VA-API driver.

## Pictures

Video does not use the effect protocol's float RGBA image refs (16 bytes a pixel, about 133 MB at
4K). A picture is **8-bit 4:2:0, three planes**, Y then U then V. The luma plane is `width ×
height`; each chroma plane is `width/2 × height/2`. Width and height are even and within the
device's `min_size` and `max_size`; other sizes are declined before the encoder opens. Every
`encode.frame` describes the three planes itself (see [Encoding](#encoding)). EffectCraft version 1
always sends them tightly packed (`stride` = plane width), Y then U then V, one after the other in
the slot's `in` region; a later version can add padding without changing the message. Colour is
given once per job, in `encode.open`, as [H.273](https://www.itu.int/rec/T-REC-H.273) code points.
Only limited range is used at first.

## Encoding

```
encode.open → encode.opened → (encode.frame → encode.packets)… → encode.flush → encode.flushed
              → encode.close
```

| Request | Reply | |
|---|---|---|
| `{"op":"encode.open","job":1,"device":"nvenc:0","codec":"h264","width":1920,"height":1080,"rate":[30,1],"bitrate_kbps":15000,"profile":"high","keyint":60,"colour":{"primaries":1,"transfer":1,"matrix":1,"range":"limited"},"slots":[{"in":{"offset":0,"len":3110400},"out":{"offset":3110400,"len":1048576}},…]}` | `{"op":"encode.opened","job":1,"codec_config":"<base64 avcC>","delay":2}` | `job` is chosen by EffectCraft and is never reused. `slots` is the memory the helper may use, decided by EffectCraft. `codec_config` is the `avcC` (H.264) or `hvcC` (HEVC) record for the MP4 sample entry. `delay` is the number of frames the encoder holds back (B-frames). |
| `{"op":"encode.frame","job":1,"frame":0,"slot":0,"planes":[{"offset":0,"stride":1920,"size":[1920,1080],"len":2073600},{"offset":2073600,"stride":960,"size":[960,540],"len":518400},{"offset":2592000,"stride":960,"size":[960,540],"len":518400}]}` | `{"op":"encode.packets","job":1,"frame":0,"packets":[{"offset":0,"len":41230,"pts":0,"dts":-2,"key":true}]}` | The picture is in `slots[0].in`. `planes` lists Y, U and V: `offset` is from the start of the file, `size` is in samples, `len` is `stride × height` bytes. EffectCraft sets them, and the helper reads only them. The reply means the slot may be reused and says where the packets are, inside `slots[0].out`. It may hold no packets (the encoder is still holding frames) or several. |
| `{"op":"encode.flush","job":1,"out":{"offset":…,"len":…}}` | `{"op":"encode.packets","job":1,"frame":null,"packets":[…]}` then `{"op":"encode.flushed","job":1}` | EffectCraft sends it only after every `encode.frame` has been answered, and after it sends only `cancel` or `encode.close`. The helper answers with exactly one `encode.packets` with `"frame":null` (its `packets` may be empty), then exactly one `encode.flushed`. The packets are the rest of the stream, inside `out`, and their offsets are counted from it. EffectCraft sizes `out` for the frames the encoder still holds: at least `(delay + 1)` times the length of a slot's `out` region. If the encoder's output still does not fit, the helper answers `error` with code `packet_too_large`. |
| `{"op":"encode.close","job":1}` | none | Free everything for the job. EffectCraft sends it after `encode.flushed` or `cancelled`, and frees or reuses the job's regions only then. If the helper was stopped first, there is no reply to wait for: the exchange file is dropped and its regions are never reused (see [stopping](#timeouts-and-errors)). |
| `{"op":"cancel","job":1}` | `{"op":"cancelled","job":1}` | Stop now. After it sends `cancel`, EffectCraft ignores every reply for the job except `cancelled`, so replies for frames still in flight are not protocol breaks. The helper stops using the job's regions before it replies `cancelled`, and `cancelled` ends every frame of the job that is still outstanding. |

- **Packets are MP4 samples.** Each is a sequence of length-prefixed NAL units, every length a
  4-byte big-endian number, which is what `codec_config` declares (`lengthSizeMinusOne` 3). A
  helper whose API produces Annex B converts it. EffectCraft writes the bytes into the MP4 as
  they are.
- **Packet offsets** are counted from the start of the region the reply uses (`slots[slot].out`,
  or the `out` region of `encode.flush`). In the example the packet starts at file offset
  3110400 + 0. EffectCraft adds the region's offset and checks that `offset + len` is within the
  region's length. A packet outside the region is a protocol break (see the table below), not a
  `packet_too_large` error.
- **Timestamps** are in units of 1 / `rate[0]` seconds, so frame `i` has `pts = i × rate[1]`.
  `dts` may be earlier than `pts`.
- **Who owns what.** EffectCraft decides every offset. The helper reads only the slot it is told
  to and writes only that slot's `out` region (or the `out` region of `encode.flush`). EffectCraft
  checks every offset and length it gets back with checked arithmetic against the regions it gave
  out.
- **Backpressure.** At most 8 frames, and at most 256 MiB, are queued plus in flight together. When
  the budget is used up EffectCraft waits for a reply before sending the next frame, so a slow GPU
  slows the export and does not use more memory.
- **Replies are in frame order,** `error` replies included.
- **Control lines** are at most 1 MiB. Pictures and packets never go through the pipe.

### A normal job

An encoder that holds back one frame (`delay` 1), two frames. `E` is EffectCraft, `H` the helper:

```
E  encode.open     job 1                     H  encode.opened       delay 1
E  encode.frame    job 1, frame 0, slot 0    H  encode.packets      frame 0, packets []
E  encode.frame    job 1, frame 1, slot 1    H  encode.packets      frame 1, 1 packet (pts 0)
E  encode.flush    job 1, out {…}            (sent once both frame replies are in)
                                             H  encode.packets      frame null, 1 packet (pts 1)
                                             H  encode.flushed
E  encode.close    job 1
```

Out of order: after frames 0 and 1 are sent, a reply for frame 1 before the reply for frame 0 is a
protocol break, and so is `encode.flushed` before the `encode.packets` with `"frame":null`.
Anything the helper sends for job 1 after `encode.flushed` arrives for an ended job (EffectCraft
sends `encode.close` as soon as it reads `encode.flushed`) and is dropped.

### Job and frame ids

`job` is a positive whole number that EffectCraft takes from a counter that only goes up, so an id
is never reused within one helper process. A helper that is restarted, or a second helper process,
is a separate connection and may see the same number again. `frame` counts from 0 inside a job and is
never reused in it. Every id EffectCraft has sent is **issued**. A job is **open** from
`encode.open` until EffectCraft sends `cancel` or `encode.close`; after that it is **ended**, and
it stays ended. An id EffectCraft has not sent is **never issued**.

### What EffectCraft does with each reply

A **protocol break** stops the helper and counts towards the 3 stops. A **job error** ends one job
and the helper carries on; it does not count.

| Reply | EffectCraft |
|---|---|
| `encode.opened` for an open job with an `encode.open` outstanding | Accepts it; the job can take frames. |
| `encode.packets` or `error` for an open job, for the oldest outstanding frame, with every packet inside its region | Accepts it. |
| After `encode.flush`: the first `encode.packets` with `"frame":null`, every packet inside the `out` region of that `encode.flush` | Accepts it. |
| After that batch: `encode.flushed` | Accepts it; the job is complete and EffectCraft sends `encode.close`. |
| `encode.packets` with `"frame":null` or `encode.flushed` when no `encode.flush` is outstanding; a second `encode.packets` with `"frame":null`; `encode.flushed` before the batch; `encode.opened` when no `encode.open` is outstanding | Protocol break. |
| No reply to `encode.open`, `encode.frame`, or `encode.flush` (the batch and `encode.flushed` both count) in time | Protocol break: EffectCraft stops the helper ([timeouts](#timeouts-and-errors)). |
| `encode.packets` or `error` for an open job with a `frame` that was never sent, already answered, or not the oldest outstanding; or `error` without `frame` when no `encode.open` or `encode.flush` is outstanding | Protocol break. |
| `encode.packets` with a packet whose `offset + len` is outside its region | Protocol break. |
| `error` without `frame` while an `encode.open` or `encode.flush` is outstanding (for a flush, instead of the batch or of `encode.flushed`) | Job error: EffectCraft sends `cancel`, then `encode.close`. |
| `error` with `packet_too_large`: the helper says the encoder's output does not fit the `out` region it was given, and it did not write past it | Job error. |
| `cancelled` for a job EffectCraft sent `cancel` for | Accepts it; the job's frames are over. |
| `cancelled` for an open job | Protocol break. |
| Any other reply for an **ended** job (late replies for frames that were in flight, a second `cancelled`), and any reply except `cancelled` for a job that has just answered with an `error` and has not been cancelled yet | Dropped and logged. The regions it names are not read. Not a break. |
| Any reply for a job that was **never issued** | Protocol break. |
| A line that does not parse, or is over 1 MiB | Protocol break. |

## Timeouts and errors

- **Per frame, not per export.** A frame is outstanding from `encode.frame` until its reply, until
  `cancelled`, or until the helper has exited. If the oldest outstanding frame waits longer than the
  timeout (30 s by default, set with `EFFECTCRAFT_HELPER_VIDEO_TIMEOUT`, as
  `EFFECTCRAFT_HELPER_TIMEOUT` is for effects), EffectCraft stops (kills) the helper and waits for
  it to exit. A GPU that hangs without exiting ends the helper instead of making the export look
  busy. The timeout detects a hung helper; it is not a strict arrival deadline: a reply that is
  already queued when EffectCraft next reads is accepted even if it came after the timeout.
  `encode.open` and `encode.flush` use the load timeout (600 s); for a flush it covers the
  `encode.packets` and the `encode.flushed` together. After `cancel` the frames of that job have no
  timeout; `cancelled` is due within the same 30 s, or EffectCraft stops the helper.
- **`{"op":"error","job":1,"frame":3,"code":"…","message":"…"}`** answers one request. `frame` is
  there when it answers an `encode.frame` (it ends that frame's timeout) and absent for
  `encode.open` and `encode.flush`. The helper process carries on, but the job is over: EffectCraft
  sends `cancel`, then `encode.close` once `cancelled` arrives. Frames sent before the error are not
  answered; a late reply for one is dropped. Codes: `unsupported` (the settings are declined),
  `device_lost`, `out_of_memory`, `packet_too_large` (the encoder's output for a frame does not fit
  the `out` region), `encoder`. All of them are job errors.
- **EffectCraft stops the helper** when it exits, closes stdout, sends a line that does not parse
  or does not fit, breaks the rules above, or misses a timeout. After 3 stops it is turned off and
  listed in `helper_blocklist.txt` until the user retries in Settings, as for effect helpers.
- **A stopped helper's exchange file is never reused.** A child of the helper can map the exchange
  file too, and on Unix it can leave the process group (`setsid`), so EffectCraft does not try to
  prove that every such process has exited. After a stop it drops the exchange file and gives the
  next helper a new one, so a stray process can only write into a file nobody reads. Regions are
  reused only inside one helper process that is still running, after `encode.flushed` or
  `cancelled`.
- **Stopping the helper is bounded.** EffectCraft kills the helper process and waits for it to
  exit for at most 5 s. If it is stuck (for example in an uninterruptible driver call), EffectCraft
  gives up waiting, logs it, and the export fails with an error instead of hanging. Processes the
  helper started are not tracked or killed: the rule above is what keeps their writes harmless.

What EffectCraft does with a failure:

| When | What happens |
|---|---|
| Before the first frame (`unsupported`, no helper, version mismatch, blocked) | With Hardware Encoding: Auto, the built-in encoder runs and the render log says why. |
| After frames have been sent (a crash, a hang, `device_lost`) | The export ends with an error that says what failed. The format is never changed silently, and hardware and software output are never mixed in one file. |

A hardware export writes to a temporary file and replaces the destination only when it finishes,
so a failed or cancelled export leaves any existing file as it was.

## Differences from RFC-0031

[D3](https://github.com/storytold/craft-contrib/blob/main/rfcs/0031-hardware-video-helper.md)
describes the frame format. This page follows it, with these differences and additions:

- **Plane descriptors** are in every `encode.frame`, as D3 says. Colour information is not: it is
  sent once per job, in `encode.open`, because it cannot change inside a job.
- **Replies for a cancelled or old job** are ignored, as D3 says, but only for ids EffectCraft has
  issued. A reply for a job id it never issued is a protocol break.
- **Limits** D3 leaves open ("a few frames", "a cap on the bytes") are 8 frames and 256 MiB.

## Decoding (phase 2)

`decode.open`, `decode.sample` and the picture replies are reserved for phase 2 and will be
specified with it. They must cover rebuilding the decoder's state after a failure (reference
frames, presentation order, timestamps, seeks, flushes and colour information), as RFC-0031 D6
requires.

## Not decided here

- How Auto chooses between two GPUs that both offer a codec.
- Who builds, signs and releases the helper, and where its code lives.
- 10-bit pictures, zero-copy transfer from GPU textures and codecs other than H.264 and HEVC.

[rfc-0031]: https://github.com/storytold/craft-contrib/blob/main/rfcs/0031-hardware-video-helper.md
