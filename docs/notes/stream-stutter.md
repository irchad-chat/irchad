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

## Judder at ~50 fps (2026-11-17)

A one-hour share (publisher RTX 4090, NVENC AV1; viewer RTX 4070 Ti with
the AMD iGPU decoding) put the viewer at 13 rough seconds: 9 of downlink
loss that NACK repaired (the publisher saw 0 loss), 4 freezes, all
downstream of the publisher. None of these explains constant stutter.
What does: the share asks for 60 fps, and every logged second on both
sides ran at 46–58, with 25–39 ms gaps. That's a skipped frame every few
frames, and no single second counts as behind.

First suspect, the 60 Hz polling of Windows capture
(`platform/desktop.rs`) beating against a 60 Hz screen, was wrong. The
10 s summary added for it showed 0 empty polls in 25 minutes:
Windows.Graphics.Capture returns a frame on every poll, and it offered
58.5–59 fps. Our sink sent only 48–58 of them, and its drops matched the
33 ms "late" gaps.

The cause was the sink's rate cap: `next = next.max(now) + interval` with
a quarter-frame slack. A frame that arrived a few ms late (capturing it
takes longer under GPU load) moved the beat later. The next frame, on
time, then looked more than 4 ms early and was dropped.

Fix: `screen::Pace` keeps a fixed beat that a late frame doesn't move. It
admits a frame up to half an interval early and still lets through no more
than one frame per interval over time. The poll loop also polls on a fixed
beat now, which recovers the ~1 fps it lost to sleep overshoot (59 polls a
second instead of 60).

Logging: every 10 s the publisher logs `screen share, last 10 s:` with
captured (sent by us), offered (by the capturer) and libwebrtc-sent fps,
frames over 1.5 frame times late, the longest gap, and polls with how many
were empty. If captured is short of offered, look at the sink. If offered
is short of the asked rate, look at the capturer or the game.

Confirmed the same night (Gambit, 9 minutes): captured 59.4–60.0 fps
against 48–58 before, captured equal to offered, 0–13 late frames per 10 s
against 15–119, and the longest gap 21–35 ms.

## Viewer freezes on AMF AV1 (2026-11-17)

The viewer of that run (RTX 4070 Ti plus an AMD iGPU) still froze about
every 5–40 s. Each freeze came with a keyframe request that reached the
publisher a second later, mostly in a second with no loss, and some
seconds decoded nothing at all (`0 fps, decode 0.0 ms`). libwebrtc asks for
a key frame and drops frames until one comes when `Decode` returns an
error, which points at the decoder. AV1 decodes on the iGPU through AMF
because webrtc-sys's NVDEC factory has no AV1.

Its errors never reached the log: the `libwebrtc` crate forwards all of
libwebrtc's log at debug. Now:

- `monitor::libwebrtc_warnings` adds a second sink (`new_log_sink_at` in
  the vendored webrtc-sys) for warnings and errors, at warn, at most 20
  lines a second.
- `amf_decoder.cpp` stops failing the decode on two `SubmitInput` results
  that aren't errors: `AMF_DECODER_NO_FREE_SURFACES` (drain, then retry, as
  for `AMF_INPUT_FULL`) and `AMF_RESOLUTION_UPDATED` (accepted). Unconfirmed
  as the cause. The next viewer log names the failing call either way.

With the libwebrtc log visible, the next viewer log showed no decoder
errors at all. Each freeze was a burst of upload loss on the publisher
(1–2%, about once a minute), then `No decodable frame in 3 s requesting
keyframe` on the viewer 3 s later. Retransmission doesn't repair the loss
for that viewer, and libwebrtc waits 3 s before asking for a key frame.
Logged since, to see where the repair fails and what the path is:

- Both sides: retransmit requests and the packets resent for them (`N nack
  M resent`), the publisher's per "behind" second, the viewer's per rough
  second.
- Both sides: the chosen candidate pair, without addresses (`host udp over
  wifi to host udp`), when it changes.
- Viewer: freeze length on rough lines, and a 10 s summary like the
  publisher's (fps, frozen ms, loss, retransmissions, keyframe requests).
- The libwebrtc sink drops ICE pings on unreachable adapters, startup codec
  duplicates and the SDK's SDP edits, and keeps one `has no GoP` line a
  second.
