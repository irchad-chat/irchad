<!-- A public stream's link (D87): someone's screen share, watch only.
     No IRC and no account: chadd hands out a hidden, subscribe-only token. -->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import {
  Room,
  RoomEvent,
  Track,
  type RemoteParticipant,
  type RemoteTrack,
  type RemoteTrackPublication,
} from 'livekit-client'
import { NETWORK_ORIGIN, oneLine, streamToken } from '../lib/stream'

const code = new URLSearchParams(location.search).get('watch') ?? ''
// The screen's app audio and the call's: D14 caps what a room can make us play.
const MAX_AUDIO = 4

const video = ref<HTMLVideoElement>()
const audio = ref<HTMLDivElement>()
const status = ref<'connecting' | 'waiting' | 'live' | 'ended'>('connecting')
const error = ref<string>()
const muted = ref(false)
// The browser refused to start the video without a click (autoplay blocked).
const blocked = ref(false)
// Ours: the sound is in the hidden <audio> elements, so the video's own mute does nothing.
const silenced = ref(false)
// Who's streaming: their display name and account, once they're in the room.
const streamer = ref<{ name: string; account: string }>()
const network = ref('')
// The web client: this page without `?watch=`.
const webClient = location.pathname
const pitch = computed(() => {
  const on = network.value ? ` on ${network.value}` : ''
  return streamer.value ? `Hang out with ${streamer.value.name}${on}` : `Join the chat${on}`
})
// No adaptiveStream: one video, and it would pause the track while the element is still hidden.
const room = new Room()

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

// Viewers are hidden, so the one participant we see is the streamer (chadd's
// token: identity = account, name = display name).
function meet(p: RemoteParticipant) {
  const account = oneLine(p.identity)
  if (!account) return
  streamer.value = { name: oneLine(p.name ?? '') || account, account }
}

function attach(track: RemoteTrack, _pub?: unknown, p?: RemoteParticipant) {
  if (p) meet(p)
  // Subscribed events and the first look at the room can both bring a track.
  if (attached.has(track)) return
  attached.add(track)
  if (track.kind === Track.Kind.Video && video.value && !video.value.srcObject) {
    track.attach(video.value)
    status.value = 'live'
    noAv1.value = false
    // Attached while hidden (v-show): browsers don't start it on their own once shown.
    void nextTick(() => video.value?.play().catch(() => (blocked.value = true)))
  } else if (track.kind === Track.Kind.Audio && audio.value && audio.value.childElementCount < MAX_AUDIO) {
    const el = track.attach()
    el.muted = silenced.value
    audio.value.append(el)
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
    network.value = body.network
    room
      .on(RoomEvent.ParticipantConnected, meet)
      .on(RoomEvent.TrackSubscribed, attach)
      .on(RoomEvent.TrackUnsubscribed, detach)
      .on(RoomEvent.TrackPublished, published)
      .on(RoomEvent.AudioPlaybackStatusChanged, () => (muted.value = !room.canPlaybackAudio))
      .on(RoomEvent.Disconnected, () => (status.value = 'ended'))
    await room.connect(body.livekit, body.token)
    // Tracks can arrive before connect() resolves: don't undo a 'live' they set.
    if (status.value === 'connecting') status.value = 'waiting'
    for (const p of room.remoteParticipants.values()) {
      meet(p)
      for (const pub of p.trackPublications.values()) {
        published(pub)
        if (pub.track) attach(pub.track)
      }
    }
    muted.value = !room.canPlaybackAudio
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
    status.value = 'ended'
  }
}

// A click is the one thing every autoplay policy accepts: start the picture and the sound.
function play() {
  blocked.value = false
  video.value?.play().catch(() => (blocked.value = true))
  void room.startAudio()
}

function silence(on: boolean) {
  silenced.value = on
  audio.value?.querySelectorAll('audio').forEach((a) => (a.muted = on))
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
  <div class="page">
    <header>
      <i class="ph ph-screencast live" :class="{ on: status === 'live' }" />
      <div class="who">
        <template v-if="streamer">
          <b :title="streamer.name">{{ streamer.name }}</b>
          <span v-if="streamer.name !== streamer.account" class="dim">{{ streamer.account }}</span>
          <span class="dim">{{ status === 'live' ? 'is streaming' : 'streamed' }}{{ network ? ` on ${network}` : '' }}</span>
        </template>
        <span v-else-if="network" class="dim">A stream on {{ network }}</span>
      </div>
    </header>
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
      <button v-if="status === 'live' && blocked" class="primary start" @click="play">
        <i class="ph ph-play" /> Click to watch
      </button>
    </div>
    <footer>
      <span class="pitch">{{ pitch }} in IrChad: chat, voice and screen sharing.</span>
      <div class="ctas">
        <a class="button" :href="webClient"><i class="ph ph-browser" /> Open in browser</a>
        <a class="button primary" :href="`${NETWORK_ORIGIN}/invite`">
          <i class="ph ph-download-simple" /> Get the app
        </a>
      </div>
      <div v-if="status === 'live'" class="controls">
        <button v-if="muted" class="primary" @click="room.startAudio()">
          <i class="ph ph-speaker-slash" /> Turn on sound
        </button>
        <button
          :title="silenced ? 'Unmute' : 'Mute'"
          :aria-label="silenced ? 'Unmute' : 'Mute'"
          :aria-pressed="silenced"
          @click="silence(!silenced)"
        >
          <i class="ph" :class="silenced ? 'ph-speaker-slash' : 'ph-speaker-high'" />
        </button>
        <button title="Full screen" aria-label="Full screen" @click="fullscreen">
          <i class="ph ph-corners-out" />
        </button>
      </div>
    </footer>
  </div>
</template>

<style scoped>
/* The banners float over the video, so the page never grows past the window. */
.page {
  flex: 1;
  position: relative;
  display: flex;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
header,
footer {
  position: absolute;
  left: 0;
  right: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;
  color: #fff;
}
header {
  top: 0;
  background: linear-gradient(#000c, transparent);
}
footer {
  bottom: 0;
  flex-wrap: wrap;
  background: linear-gradient(transparent, #000c);
}
.live {
  font-size: 22px;
  color: var(--border);
}
.live.on {
  color: var(--err);
}
.who {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
  flex-wrap: wrap;
}
.who b {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 100%;
}
.pitch {
  overflow-wrap: anywhere;
  text-shadow: 0 1px 2px #000;
}
.ctas {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--surface);
  color: inherit;
  text-decoration: none;
  white-space: nowrap;
}
.button.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--bg);
  font-weight: bold;
}
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
.start {
  position: absolute;
  font-size: 1.25em;
  padding: 12px 20px;
}
.controls {
  margin-left: auto;
  display: flex;
  gap: 8px;
}
</style>
