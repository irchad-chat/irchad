<!-- Channels and DMs, joining or messaging someone, and who we are. -->
<script setup lang="ts">
import { computed, ref } from 'vue'
import { useClient } from '../stores/client'
import SignUpDialog from './SignUpDialog.vue'
import VoiceMembers from './VoiceMembers.vue'
import VoiceBar from './VoiceBar.vue'
import { useVoice } from '../stores/voice'
import { isMeeting } from '../wasm/chad_web'

const client = useClient()
const voice = useVoice()
const opening = ref('')
const signingUp = ref(false)
const channels = computed(() => client.buffers.filter((b) => b.kind !== 'dm'))
const dms = computed(() => client.buffers.filter((b) => b.kind === 'dm'))

function open() {
  if (!opening.value.trim()) return
  if (client.open(opening.value)) opening.value = ''
}

async function newMeeting() {
  const url = await client.createMeeting()
  if (!url) return
  try {
    await navigator.clipboard.writeText(url)
    client.notify('Meeting link copied. Anyone with it can join.')
  } catch {
    client.notify(`Meeting link: ${url}`)
  }
}

async function copyLink(channel: string) {
  const url = `${client.network}/meet/${channel.slice('#meet-'.length).toLowerCase()}`
  await navigator.clipboard.writeText(url).catch(() => client.notify(url))
}

const toggleAway = () => client.setAway(client.away ? null : 'Away')

const status = computed(() => {
  const s = client.state
  if (!s) return ''
  return { connecting: 'Connecting…', verifying: 'Check your email', connected: '', disconnected: 'Reconnecting…', failed: 'Signed out' }[
    s.state
  ]
})
</script>

<template>
  <nav class="side">
    <header>
      <b>{{ client.state?.name }}</b>
    </header>
    <div class="list">
      <template v-for="group in [channels, dms]" :key="group === dms ? 'dms' : 'channels'">
        <h3 v-if="group.length" class="dim small">{{ group === dms ? 'Direct messages' : 'Channels' }}</h3>
        <template v-for="b in group" :key="b.target">
        <div class="buf" :class="{ on: b.target === client.selected, unread: b.unread }">
          <button class="name" :title="b.topic" @click="client.select(b.target)">
            <span v-if="b.kind === 'dm'" class="dot" :class="{ online: b.online, away: b.away }" />
            <i v-else-if="isMeeting(b.target)" class="ph ph-video-camera dim" />
            <span v-else class="dim">#</span>
            <span class="ellipsis">{{ isMeeting(b.target) ? 'Meeting' : b.kind === 'channel' ? b.target.replace(/^#/, '') : b.target }}</span>
            <span v-if="b.mentions" class="badge">{{ b.mentions }}</span>
          </button>
          <button
            v-if="b.voice && !client.state?.guest && voice.call?.channel !== b.target"
            class="join"
            title="Join voice"
            :aria-label="`Join voice in ${b.target}`"
            :disabled="!client.connected"
            @click="voice.join(b.target)"
          >
            <i class="ph ph-speaker-high" />
          </button>
          <button
            v-if="isMeeting(b.target)"
            class="join"
            title="Copy meeting link"
            :aria-label="`Copy the link to ${b.target}`"
            @click="copyLink(b.target)"
          >
            <i class="ph ph-link" />
          </button>
          <button
            v-if="b.kind === 'dm'"
            class="x"
            title="Close"
            :aria-label="`Close ${b.target}`"
            @click="client.close(b)"
          >
            <i class="ph ph-x" />
          </button>
        </div>
        <VoiceMembers v-if="b.kind === 'channel'" :buffer="b" />
        </template>
      </template>
    </div>
    <button v-if="client.state?.meetings" class="new-meeting" :disabled="!client.connected" @click="newMeeting">
      <i class="ph ph-video-camera" /> New meeting
    </button>
    <form @submit.prevent="open">
      <input v-model="opening" aria-label="Join a channel or message someone" placeholder="#channel or nick" />
    </form>
    <p v-if="client.error" class="err small">{{ client.error }}</p>
    <a class="get-app" :href="`${client.network}/invite`" target="_blank" rel="noopener">
      <i class="ph ph-download-simple" /> Get the desktop app
    </a>
    <VoiceBar />
    <footer>
      <div class="me">
        <b class="ellipsis">{{ client.state?.nick }}</b>
        <span class="dim small">{{ status || (client.state?.guest ? 'Guest' : client.away ? 'Away' : 'Online') }}</span>
      </div>
      <button v-if="client.state?.guest" :disabled="!client.connected" @click="signingUp = true">Sign up</button>
      <button v-else :disabled="!client.connected" :title="client.away ? 'Come back' : 'Go away'" @click="toggleAway">
        {{ client.away ? 'Back' : 'Away' }}
      </button>
    </footer>
    <SignUpDialog v-if="signingUp" @close="signingUp = false" />
  </nav>
</template>

<style scoped>
.side {
  width: 240px;
  flex: none;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  min-height: 0;
}
header {
  box-sizing: border-box;
  height: var(--header-height);
  display: flex;
  align-items: center;
  padding: 0 16px;
  border-bottom: 1px solid var(--border);
}
.list {
  flex: 1;
  overflow: auto;
  padding: 8px;
}
h3 {
  margin: 12px 8px 4px;
  font-weight: normal;
  text-transform: uppercase;
}
.buf {
  display: flex;
  border-radius: 6px;
  opacity: 0.7;
}
.buf.unread {
  opacity: 1;
  font-weight: bold;
}
.buf.on {
  background: var(--surface);
  opacity: 1;
}
.buf:hover {
  background: var(--surface);
}
.name {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  text-align: left;
  background: none;
  border: none;
}
.join {
  background: none;
  border: none;
}
.new-meeting {
  margin: 0 8px 8px;
}
.x {
  visibility: hidden;
  background: none;
  border: none;
}
.buf:hover .x,
.x:focus {
  visibility: visible;
}
.dot {
  width: 8px;
  height: 8px;
  flex: none;
  border-radius: 50%;
  border: 1px solid currentColor;
}
.dot.online {
  background: var(--ok);
  border-color: var(--ok);
}
.dot.online.away {
  background: var(--warn);
  border-color: var(--warn);
}
form {
  padding: 0 8px 8px;
}
form input {
  width: 100%;
  box-sizing: border-box;
}
.get-app {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 8px 8px;
  padding: 8px 12px;
  border-radius: 6px;
  background: #89b4fa22;
  color: var(--accent);
  font-weight: bold;
  text-decoration: none;
}
.get-app:hover {
  background: #89b4fa33;
}
footer {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: var(--bg-deep);
}
.me {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.err {
  padding: 0 12px;
}
</style>
