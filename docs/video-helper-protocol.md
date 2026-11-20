# Video helper protocol (proposed)

**Status: proposed, not implemented.** This page specifies how EffectCraft will talk to a
hardware video helper, as decided in [RFC-0031][rfc-0031] (hardware video in a helper process).
Nothing here exists in the app yet. Phase 1 covers **encoding**; decoding is reserved for phase 2
([decoding](#decoding-phase-2)).

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
device's `min_size` and `max_size`; other sizes are declined before the encoder opens. Planes are
tightly packed (`stride` = plane width) and sit one after the other in the slot. Colour is given as
[H.273](https://www.itu.int/rec/T-REC-H.273) code points. Only limited range is used at first.

## Encoding

```
encode.open → encode.opened → (encode.frame → encode.packets)… → encode.flush → encode.flushed
              → encode.close
```

| Request | Reply | |
|---|---|---|
| `{"op":"encode.open","job":1,"device":"nvenc:0","codec":"h264","width":1920,"height":1080,"rate":[30,1],"bitrate_kbps":15000,"profile":"high","keyint":60,"colour":{"primaries":1,"transfer":1,"matrix":1,"range":"limited"},"slots":[{"in":{"offset":0,"len":3110400},"out":{"offset":3110400,"len":1048576}},…],"flush_out":{"offset":…,"len":…}}` | `{"op":"encode.opened","job":1,"codec_config":"<base64 avcC>","delay":2}` | `job` is chosen by EffectCraft and is never reused. `slots` is the memory the helper may use, decided by EffectCraft. `codec_config` is the `avcC` (H.264) or `hvcC` (HEVC) record for the MP4 sample entry. `delay` is the number of frames the encoder holds back (B-frames). |
| `{"op":"encode.frame","job":1,"frame":0,"slot":0}` | `{"op":"encode.packets","job":1,"frame":0,"packets":[{"offset":0,"len":41230,"pts":0,"dts":-2,"key":true}]}` | The picture is in `slots[0].in`. The reply means the slot may be reused and says where the packets are, inside `slots[0].out`. It may hold no packets (the encoder is still holding frames) or several. |
| `{"op":"encode.flush","job":1}` | `{"op":"encode.packets","job":1,"frame":null,"packets":[…]}` then `{"op":"encode.flushed","job":1}` | The rest of the stream, inside `flush_out`. |
| `{"op":"encode.close","job":1}` | none | Free everything for the job. |
| `{"op":"cancel","job":1}` | `{"op":"cancelled","job":1}` | Stop now. EffectCraft ignores any other reply for the job after it sends `cancel`. |

- **Timestamps** are in units of 1 / `rate[0]` seconds, so frame `i` has `pts = i × rate[1]`.
  `dts` may be earlier than `pts`.
- **Who owns what.** EffectCraft decides every offset. The helper reads only the slot it is told
  to and writes only that slot's `out` region (or `flush_out`). EffectCraft checks every offset and
  length it gets back with checked arithmetic against the regions it gave out.
- **Backpressure.** At most 8 frames, and at most 256 MiB, are queued plus in flight together. When
  the budget is used up EffectCraft waits for a reply before sending the next frame, so a slow GPU
  slows the export and does not use more memory.
- **Replies are in frame order.** A reply with an unknown `job` or `frame`, outside the regions, or
  larger than its `out` region (`packet_too_large`) is a protocol break.
- **Control lines** are at most 1 MiB. Pictures and packets never go through the pipe.

## Timeouts and errors

- **Per frame, not per export.** A frame is outstanding from `encode.frame` until its reply. If the
  oldest outstanding frame waits longer than the timeout (30 s by default, set with
  `EFFECTCRAFT_HELPER_VIDEO_TIMEOUT`, as `EFFECTCRAFT_HELPER_TIMEOUT` is for effects), EffectCraft
  stops the helper. A GPU that hangs without exiting ends the helper instead
  of making the export look busy. `encode.open` and `encode.flush` use the load timeout (600 s).
- **`{"op":"error","job":1,"code":"…","message":"…"}`** is for one request; the helper carries on.
  Codes: `unsupported` (the settings are declined), `device_lost`, `out_of_memory`,
  `packet_too_large`, `encoder`.
- **EffectCraft stops the helper** when it exits, closes stdout, sends a line that does not parse
  or does not fit, breaks the rules above, or misses a timeout. After 3 stops it is turned off and
  listed in `helper_blocklist.txt` until the user retries in Settings, as for effect helpers.

What EffectCraft does with a failure:

| When | What happens |
|---|---|
| Before the first frame (`unsupported`, no helper, version mismatch, blocked) | With Hardware Encoding: Auto, the built-in encoder runs and the render log says why. |
| After frames have been sent (a crash, a hang, `device_lost`) | The export ends with an error that says what failed. The format is never changed silently, and hardware and software output are never mixed in one file. |

A hardware export writes to a temporary file and replaces the destination only when it finishes,
so a failed or cancelled export leaves any existing file as it was.

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
