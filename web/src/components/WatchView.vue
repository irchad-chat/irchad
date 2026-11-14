<!-- A public stream's link (D87): someone's screen share, watch only.
     No IRC and no account: chadd hands out a hidden, subscribe-only token. -->
<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { Room, RoomEvent, Track, type RemoteTrack, type RemoteTrackPublication } from 'livekit-client'
import { streamToken } from '../lib/stream'

const code = new URLSearchParams(location.search).get('watch') ?? ''
// The screen's app audio and the call's: D14 caps what a room can make us play.
const MAX_AUDIO = 4

const video = ref<HTMLVideoElement>()
const audio = ref<HTMLDivElement>()
const status = ref<'connecting' | 'waiting' | 'live' | 'ended'>('connecting')
const error = ref<string>()
const muted = ref(false)
const room = new Room({ adaptiveStream: true })

// Nvidia sharers publish AV1 with no backup codec (D90). A browser whose
// WebRTC won't take AV1 (LibreWolf) rejects the video without an error, so
// an AV1 track that never starts playing gets a hint about the browser.
const NO_VIDEO_MS = 5000
const noAv1 = ref(false)
let av1Timer: ReturnType<typeof setTimeout> | undefined
function published(pub: RemoteTrackPublication) {
  if (pub.kind !== Track.Kind.Video || !pub.mimeType?.toLowerCase().endsWith('/av1')) return
  clearTimeout(av1Timer)
  av1Timer = setTimeout(() => (noAv1.value = status.value === 'waiting'), NO_VIDEO_MS)
}

// The room only holds the streamer's screen and its audio, and maybe the
// call's (chadd's grants), but D14: one video, a few audio tracks.
const attached = new Set<RemoteTrack>()
function attach(track: RemoteTrack) {
  // Subscribed events and the first look at the room can both bring a track.
  if (attached.has(track)) return
  attached.add(track)
  if (track.kind === Track.Kind.Video && video.value && !video.value.srcObject) {
    track.attach(video.value)
    status.value = 'live'
    noAv1.value = false
  } else if (track.kind === Track.Kind.Audio && audio.value && audio.value.childElementCount < MAX_AUDIO) {
    audio.value.append(track.attach())
  }
}

function detach(track: RemoteTrack) {
  attached.delete(track)
  track.detach().forEach((e) => e !== video.value && e.remove())
  if (track.kind === Track.Kind.Video) status.value = 'waiting'
}

async function start() {
  try {
    const body = await streamToken(code)
    room
      .on(RoomEvent.TrackSubscribed, attach)
      .on(RoomEvent.TrackUnsubscribed, detach)
      .on(RoomEvent.TrackPublished, published)
      .on(RoomEvent.AudioPlaybackStatusChanged, () => (muted.value = !room.canPlaybackAudio))
      .on(RoomEvent.Disconnected, () => (status.value = 'ended'))
    await room.connect(body.livekit, body.token)
    status.value = 'waiting'
    for (const p of room.remoteParticipants.values())
      for (const pub of p.trackPublications.values()) {
        published(pub)
        if (pub.track) attach(pub.track)
      }
    muted.value = !room.canPlaybackAudio
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
    status.value = 'ended'
  }
}

function fullscreen() {
  video.value?.requestFullscreen().catch(() => {})
}

onMounted(start)
onBeforeUnmount(() => {
  clearTimeout(av1Timer)
  room.disconnect()
})
</script>

<template>
  <div class="watch">
    <video ref="video" autoplay playsinline muted v-show="status === 'live'" @dblclick="fullscreen" />
    <div ref="audio" hidden />
    <div v-if="status !== 'live'" class="note">
      <p v-if="status === 'connecting'">Connecting…</p>
      <template v-else-if="status === 'waiting' && noAv1">
        <h1>This browser can't play this stream</h1>
        <p class="dim">
          It's AV1 video, which this browser's WebRTC doesn't accept.<br />
          Open the link in a Chromium-based browser such as Chrome, Edge or Brave.
        </p>
      </template>
      <p v-else-if="status === 'waiting'">Waiting for the stream to start…</p>
      <template v-else>
        <h1>{{ error ? "Can't watch this stream" : 'The stream has ended' }}</h1>
        <p class="dim">{{ error ?? 'Ask whoever sent the link for a new one if they share again.' }}</p>
      </template>
    </div>
    <div v-if="status === 'live'" class="controls">
      <button v-if="muted" class="primary" @click="room.startAudio()">
        <i class="ph ph-speaker-slash" /> Turn on sound
      </button>
      <button title="Full screen" aria-label="Full screen" @click="fullscreen">
        <i class="ph ph-corners-out" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.watch {
  flex: 1;
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #000;
  min-width: 0;
}
video {
  width: 100%;
  height: 100%;
  object-fit: contain;
}
.note {
  text-align: center;
  padding: 16px;
}
.controls {
  position: absolute;
  right: 16px;
  bottom: 16px;
  display: flex;
  gap: 8px;
}
</style>
