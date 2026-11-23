# Linked video preview: rapid-seek stall

Repeated timeline jumps could leave the viewer displaying its previous image, with no green
RAM-cache mark at the requested frame. The interface remained responsive. Deleting and
reimporting the linked item temporarily reset the loaded source/media state.

## Cause and change

Native footage frames run on a shared Rayon decode pool. A movie decoder holds its state
mutex while its codec decodes bands/slices in parallel. A worker waiting for that parallel
work could steal another whole-frame request for the same movie and block on the mutex held
by its own suspended caller. Cancellation cannot release that nested mutex wait.

Movie frame requests now queue by source path **before** entering the codec pool. At most one
whole-frame job per movie is runnable there. Other movies and the codec's own parallel work
remain concurrent. Queued canceled requests skip the decoder, and a completion guard advances
the queue even if a decoder panics. Idle queue entries are removed.

This preserves the existing decoder mutex, frame single-flight, cache-generation and refresh
rules. No UI indicator workaround or global single-thread setting is involved. Completed
linked frames still enter the RAM preview cache; their external dependencies continue to
prevent persistent disk reuse where they cannot yet be keyed safely.

## Decoder regression

The test authors a small 40-frame ProRes HQ movie in Rust. Forty simultaneous out-of-order
requests run through one MediaPool for three cold rounds, comparing every returned picture
with an independent serial reference. A subprocess deadline turns a deadlock into a test
failure rather than leaving the test runner stuck.

- Before the fix: the new regression stalled and failed at its 60-second deadline.
- After the fix: all three rounds completed with matching pictures, in 0.19 seconds total.
- Cancellation/panic recovery also verifies that another movie can finish while one is held,
  canceled queued work never enters the decoder, and later requests survive a decoder panic.
- Complete media unit suite: 179 passed, one pre-existing ignored test.

## Viewer reproduction

The manual diagnostic uses byte-identical saved-project copies and reads the existing media
without modifying it. It exercises the native Frames queue, urgent requests, background
requests and the RAM-cache data used by the timeline's green bar.

The composition contains nine ProRes HQ 10-bit 4:2:2 movies at 4096 × 1716 and 23.976 fps,
linked into a 29.97 fps destination. The diagnostic requests quarter-size destination frames
on the CPU, with disk caching off. Each wave submits 32 nonsequential seeks, including
revisits, 15 ms apart. RAM preview frames are cleared between waves while the media decoders
stay warm. Every wave must finish with the final requested frame cached and no work in flight.

| Run | Outcome |
| --- | --- |
| Before fix | First wave stalled: final frame absent, zero cached frames, 11 requests in flight after 15 seconds |
| After fix, four waves | 128 seeks completed in 12.26 seconds; all final frames cached, all requests drained |
| After fix, 16 waves | 512 seeks completed in 45.63 seconds; all final frames cached, all requests drained |
| After fix, Full destination resolution | Another 128 seeks completed in 12.69 seconds; all final frames cached, all requests drained |

The UI regression separately uses an in-memory linked video source with mismatched frame
rates, deliberately delayed reads and rapid revisits. It requires a superseded read to be
canceled, then verifies the final viewer pixels and its RAM/green-bar cache entry. All ten
linked UI workflow regressions passed.

## Build and remaining validation

The desktop application and CLI were first staged in `target/linked-preview-fixed/debug/`
while the original executable was locked by the running application. After the user saved
and closed the application, both verified binaries were copied back to `target/debug/`.
Continue using `target/debug/effectcraft.exe`. Its SHA-256 was checked against the staged
build before and after copying, and the reopened GUI process was verified to use that path.

The saved destination was then reopened with the native control channel and its existing
preferences. All three linked sources reported ready, with GPU acceleration active on the
RTX 3080 through Vulkan. Four rounds of 32 rapid playhead jumps, clearing only regenerable
memory caches between rounds, each produced newly completed renders without a render error.
The original playhead position was restored; the session remained clean and the saved
project's SHA-256 was unchanged. This live check observes render completion, not the current
frame's RAM entry: that stronger assertion belongs to the viewer reproduction above. Native
window capture/accessibility inspection was unavailable because the computer-use provider
rejected the returned window identity, so no visual green-bar confirmation is claimed here.

The final cached frame was compared with a fresh independent serial render of the copied
project using a separate MediaPool. All pixels matched exactly at both 480 × 270 and Full
1920 × 1080 destination resolution (maximum RGBA byte difference zero). This verifies the
displayed image as well as the RAM-cache marker.

Formatting and strict all-target clippy for media/UI with warnings denied passed, and both crates passed
their `wasm32-unknown-unknown` check. The staged application and CLI build passed. Ten full
linked video/audio export round trips using the staged CLI passed at default parallelism,
including alpha, sound, source refresh, changed dimensions/duration and missing-file recovery.
No single-thread workaround was set for those ten runs.

The earlier full-workspace CI result remains recorded in
[linked-compositions-validation.md](linked-compositions-validation.md); this follow-up does
not claim that unrelated baseline tests or the identity compatibility review are complete.
