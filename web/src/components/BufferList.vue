<!-- Channels and DMs, joining or messaging someone, and who we are. -->
<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Buffer } from '../wasm/chad_web'
import { useClient } from '../stores/client'
import SignUpDialog from './SignUpDialog.vue'

const client = useClient()
const opening = ref('')
const signingUp = ref(false)
const channels = computed(() => client.buffers.filter((b) => b.kind !== 'dm'))
const dms = computed(() => client.buffers.filter((b) => b.kind === 'dm'))

function open() {
  if (!opening.value.trim()) return
  if (client.open(opening.value)) opening.value = ''
}

function close(b: Buffer) {
  if (b.kind === 'channel' && !confirm(`Leave ${b.target}?`)) return
  client.close(b)
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
        <div v-for="b in group" :key="b.target" class="buf" :class="{ on: b.target === client.selected, unread: b.unread }">
          <button class="name" :title="b.topic" @click="client.select(b.target)">
            <span v-if="b.kind === 'dm'" class="dot" :class="{ online: b.online, away: b.away }" />
            <span v-else class="dim">#</span>
            <span class="ellipsis">{{ b.kind === 'channel' ? b.target.replace(/^#/, '') : b.target }}</span>
            <span v-if="b.mentions" class="badge">{{ b.mentions }}</span>
          </button>
          <button
            v-if="b.kind !== 'server'"
            class="x"
            :title="b.kind === 'channel' ? 'Leave' : 'Close'"
            :aria-label="`${b.kind === 'channel' ? 'Leave' : 'Close'} ${b.target}`"
            @click="close(b)"
          >
            <i class="ph ph-x" />
          </button>
        </div>
      </template>
    </div>
    <form @submit.prevent="open">
      <input v-model="opening" aria-label="Join a channel or message someone" placeholder="#channel or nick" />
    </form>
    <p v-if="client.error" class="err small">{{ client.error }}</p>
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
  padding: 14px 16px;
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
