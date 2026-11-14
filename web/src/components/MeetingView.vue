<!-- A meeting link's page (D86): pick a name, then a call grid with its chat.
     No account: the guest joins IRC with just a nick, and chadd hands out the
     call's token to anyone holding the link. -->
<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { createLocalVideoTrack, type LocalVideoTrack } from 'livekit-client'
import { message, useClient } from '../stores/client'
import { useVoice } from '../stores/voice'
import MeetingTile from './MeetingTile.vue'
import MessageList from './MessageList.vue'
import Composer from './Composer.vue'

const NAME_KEY = 'irchad.meeting.name'
const client = useClient()
const voice = useVoice()
const channel = client.meeting!
const host = new URL(client.network).host

const name = ref(load())
const joined = ref(false)
const left = ref(false)
const chat = ref(false)
const preview = ref<HTMLVideoElement>()
let previewTrack: LocalVideoTrack | null = null

function load(): string {
  try {
    return localStorage.getItem(NAME_KEY) ?? ''
  } catch {
    return ''
  }
}

// An IRC nick has no spaces; the call shows it as typed, give or take those.
const nick = computed(() => name.value.trim().replace(/\s+/g, '_'))
const busy = computed(() => client.state?.state === 'connecting')
const inCall = computed(() => voice.call?.channel.toLowerCase() === channel.toLowerCase())
// Roughly square: 2 people side by side, 5 in a 3×2, and so on.
const columns = computed(() => Math.ceil(Math.sqrt(Math.max(1, voice.people.length))))
const sharer = computed(() => voice.people.find((p) => p.screenTrack))
const unread = computed(() => client.buffers.find((b) => b.target.toLowerCase() === channel.toLowerCase())?.unread ?? 0)

async function togglePreview(on: boolean) {
  voice.setCamera(on)
  previewTrack?.stop()
  previewTrack = null
  if (!on) return
  try {
    previewTrack = await createLocalVideoTrack()
    if (preview.value) previewTrack.attach(preview.value)
  } catch (err) {
    voice.setCamera(false)
    client.notify(`No camera: ${message(err)}`)
  }
}

function join() {
  if (!nick.value) return
  try {
    localStorage.setItem(NAME_KEY, name.value.trim())
  } catch {
    // Remembering the name is a nicety.
  }
  previewTrack?.stop()
  previewTrack = null
  left.value = false
  joined.value = true
  if (client.connected) client.joinMeeting(channel)
  else client.connect(nick.value, '')
}

// Connected (first time, or back after a drop): into the meeting.
watch(
  () => client.connected,
  (c) => {
    if (c && joined.value && !left.value && !inCall.value) client.joinMeeting(channel)
  },
)

function leave() {
  voice.leave()
  left.value = true
}

function rejoin() {
  left.value = false
  client.joinMeeting(channel)
}

async function copyLink() {
  const url = `${client.network}/meet/${channel.slice('#meet-'.length)}`
  try {
    await navigator.clipboard.writeText(url)
    client.notify('Meeting link copied.')
  } catch {
    client.notify(url)
  }
}

onBeforeUnmount(() => previewTrack?.stop())
</script>

<template>
  <main v-if="!joined || (!client.ready && !busy)" class="prejoin">
    <h1>Join the meeting</h1>
    <p class="dim">on {{ client.state?.name || host }}</p>
    <div class="preview">
      <video v-show="voice.camera" ref="preview" autoplay playsinline muted class="mirror" />
      <i v-if="!voice.camera" class="ph ph-video-camera-slash dim" />
    </div>
    <form class="stack" @submit.prevent="join">
      <input v-model="name" aria-label="Your name" placeholder="Your name" maxlength="32" autocomplete="nickname" required />
      <div class="toggles">
        <button
          type="button"
          :class="{ off: voice.muted }"
          :aria-pressed="!voice.muted"
          @click="voice.setMuted(!voice.muted)"
        >
          <i class="ph" :class="voice.muted ? 'ph-microphone-slash' : 'ph-microphone'" />
          {{ voice.muted ? 'Mic off' : 'Mic on' }}
        </button>
        <button type="button" :class="{ off: !voice.camera }" :aria-pressed="voice.camera" @click="togglePreview(!voice.camera)">
          <i class="ph" :class="voice.camera ? 'ph-video-camera' : 'ph-video-camera-slash'" />
          {{ voice.camera ? 'Camera on' : 'Camera off' }}
        </button>
      </div>
      <button class="primary" :disabled="busy || !nick">{{ busy ? 'Joining…' : 'Join now' }}</button>
      <p v-if="client.state?.error && client.state.state === 'failed'" class="err">{{ client.state.error }}</p>
      <p v-if="client.error" class="err">{{ client.error }}</p>
      <p class="dim small">No account needed. Others in the meeting see the name you pick, marked as a guest.</p>
    </form>
  </main>

  <main v-else-if="left" class="prejoin">
    <h1>You left the meeting</h1>
    <div class="stack">
      <button class="primary" @click="rejoin">Rejoin</button>
    </div>
  </main>

  <div v-else class="meeting">
    <section class="stage" aria-label="Meeting">
      <p v-if="client.state && client.state.state !== 'connected'" class="banner small">
        {{ client.state.error ? `${client.state.error} ` : '' }}Reconnecting…
      </p>
      <p v-else-if="!voice.call?.connected" class="dim center">Joining…</p>
      <div v-if="sharer" class="spotlight">
        <MeetingTile :person="sharer" :track="sharer.screenTrack" screen />
      </div>
      <div class="grid" :class="{ strip: sharer }" :style="sharer ? {} : { gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }">
        <MeetingTile v-for="p in voice.people" :key="p.identity" :person="p" :track="p.cameraTrack" />
      </div>
    </section>
    <aside v-if="chat" class="chat" aria-label="Meeting chat">
      <MessageList />
      <Composer />
    </aside>
    <footer class="controls">
      <button v-if="voice.blocked" class="primary" @click="voice.startAudio()">Turn on sound</button>
      <button
        :class="{ off: voice.muted }"
        :title="voice.muted ? 'Unmute' : 'Mute'"
        :aria-label="voice.muted ? 'Unmute' : 'Mute'"
        :aria-pressed="voice.muted"
        @click="voice.setMuted(!voice.muted)"
      >
        <i class="ph" :class="voice.muted ? 'ph-microphone-slash' : 'ph-microphone'" />
      </button>
      <button
        :class="{ off: !voice.camera }"
        :title="voice.camera ? 'Turn camera off' : 'Turn camera on'"
        :aria-label="voice.camera ? 'Turn camera off' : 'Turn camera on'"
        :aria-pressed="voice.camera"
        :disabled="!inCall"
        @click="voice.setCamera(!voice.camera)"
      >
        <i class="ph" :class="voice.camera ? 'ph-video-camera' : 'ph-video-camera-slash'" />
      </button>
      <button
        :class="{ on: voice.screen }"
        :title="voice.screen ? 'Stop sharing' : 'Share your screen'"
        :aria-label="voice.screen ? 'Stop sharing' : 'Share your screen'"
        :aria-pressed="voice.screen"
        :disabled="!voice.call?.connected"
        @click="voice.setScreen(!voice.screen)"
      >
        <i class="ph ph-screencast" />
      </button>
      <button :class="{ on: chat }" title="Chat" aria-label="Chat" :aria-pressed="chat" @click="chat = !chat">
        <i class="ph ph-chat-circle-text" />
        <span v-if="unread && !chat" class="badge">{{ unread }}</span>
      </button>
      <button title="Copy meeting link" aria-label="Copy meeting link" @click="copyLink">
        <i class="ph ph-link" />
      </button>
      <button class="hangup" title="Leave" aria-label="Leave the meeting" @click="leave">
        <i class="ph ph-phone-disconnect" />
      </button>
    </footer>
  </div>
</template>

<style scoped>
.prejoin {
  margin: auto;
  width: min(380px, 100% - 32px);
  text-align: center;
}
.prejoin h1 {
  margin-bottom: 0;
}
.prejoin > p {
  margin-top: 4px;
}
.preview {
  display: grid;
  place-items: center;
  aspect-ratio: 16 / 9;
  margin: 16px 0;
  background: var(--panel);
  border-radius: 10px;
  overflow: hidden;
  font-size: 2.5em;
}
.preview video {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.mirror {
  transform: scaleX(-1);
}
.toggles {
  display: flex;
  gap: 8px;
}
.toggles button {
  flex: 1;
}
.off {
  color: var(--err);
}
.on {
  color: var(--accent);
}
.meeting {
  flex: 1;
  display: grid;
  grid-template-columns: 1fr auto;
  grid-template-rows: 1fr auto;
  min-width: 0;
  min-height: 0;
  height: 100%;
}
.stage {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  min-width: 0;
  min-height: 0;
  overflow: auto;
}
.center {
  text-align: center;
}
.spotlight {
  flex: 1;
  min-height: 0;
}
.grid {
  flex: 1;
  display: grid;
  gap: 8px;
  align-content: center;
}
.grid.strip {
  flex: none;
  grid-template-columns: repeat(auto-fill, minmax(160px, 200px));
  justify-content: center;
}
.chat {
  grid-row: 1;
  grid-column: 2;
  width: min(360px, 100vw);
  display: flex;
  flex-direction: column;
  min-height: 0;
  border-left: 1px solid var(--border);
  background: var(--panel);
}
.controls {
  grid-column: 1 / -1;
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 8px;
  padding: 10px;
  background: var(--bg-deep);
  border-top: 1px solid var(--border);
}
.controls button:not(.primary) {
  position: relative;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  font-size: 1.25em;
}
.controls .hangup {
  background: var(--err);
  border-color: var(--err);
  color: var(--bg);
}
.controls .badge {
  position: absolute;
  top: -4px;
  right: -4px;
  font-size: 0.6em;
}
@media (max-width: 640px) {
  .meeting {
    grid-template-columns: 1fr;
  }
  .chat {
    position: fixed;
    inset: 0 0 66px 0;
    width: auto;
    z-index: 5;
  }
  .grid:not(.strip) {
    grid-template-columns: minmax(0, 1fr) !important;
  }
}
</style>
