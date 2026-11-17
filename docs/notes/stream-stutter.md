# Stream stutter after a capture stall

Reported as viewers seeing a share stutter. The publisher's log from
2026-11-16 (Windows, RTX 4090, NVENC AV1 1080p60, 8 Mbps cap) placed it with
`chad_media::monitor`:

1. 05:06:49–05:07:08: capture fell from ~50 to 11–16 frames a second, with
   gaps of 150–300 ms (the game or compositor, not us; our convert time rose
   from 5 to 23–59 ms at the same time, so the whole box was busy).
2. 05:07:14–16, once frames came back: 12.4–12.9 Mbps sent against a 5.2 Mbps
   target (above the 8 Mbps cap), with packets waiting 440–735 ms in the pacer.

Step 2 is ours. NVENC's CBR (and AMF's, and VAAPI's) gives each frame
bitrate ÷ frame rate, counted by frames, not by time. The encoders passed
libwebrtc's `framerate_fps` through `SetRates`. That figure is the input
rate measured over the last second, so after a stall the encoder thought it
was running at ~12 fps and gave each frame ~5× its share until the
estimate caught up.

## Repro

`DIP=1 cargo run --release -p chad-media --example encoder_check` (Linux with
`CUDA_HOME=/opt/cuda`, or Windows with NVENC built). It sends 4 s at 60 fps,
5 s at 12, then 60 again, with a band of noise so the encoder runs at its
target. Before the fix (AV1, RTX 4090, loopback):

```
 9.1s: 16 frames at 2962 of 2500 kbps, queued 1 ms
10.1s: 63 frames at 5398 of 1582 kbps, queued 332 ms
11.1s: 63 frames at 5514 of 1884 kbps, queued 362 ms
12.1s: 63 frames at 2730 of 2299 kbps, queued 0 ms
```

After it, all of H.264, H.265 and AV1:

```
 9.1s: 16 frames at 773 of 2500 kbps, queued 0 ms
10.1s: 63 frames at 3212 of 2500 kbps, queued 0 ms
11.1s: 63 frames at 3018 of 2310 kbps, queued 0 ms
```

## Fix

`SetRates` keeps the frame rate from `InitEncode` (the share's max). While
capture really is slow, frames get less than they could, so they're softer,
but nothing queues. A real change of max rate comes through `InitEncode`.
Listed in `vendor/README.md`. The AMF change is untested (Windows/Radeon
only), and so is the VAAPI one (no VAAPI encoder here).

## Logging since

- The publisher's monitor now reports a "capture" stall from a 100 ms gap
  (4 frames at 60 fps) instead of 400 ms. A second that all but stops right
  after a moving one also counts, so a stall's onset is logged even when
  fewer than fps/4 frames arrive.
- Viewers log `watched stream rough (<why>)` for a second with a freeze, a PLI,
  >2% loss, dropped frames, decoding slower than two frames, or >150 ms in the
  jitter buffer. Line it up with the publisher's log by time. A rough second
  on the viewer with nothing on the publisher's side happened past their
  upload (the SFU or the viewer's network).
