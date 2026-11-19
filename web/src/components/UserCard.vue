<!-- What clicking someone shows, from the member list or a message: a bigger
     avatar, names, activity and what we know, and "Send DM". -->
<script setup lang="ts">
import { onUnmounted, ref, watch } from 'vue'
import type { Member } from '../wasm/chad_web'
import { nickColor } from '../lib/format'
import { useClient } from '../stores/client'
import Avatar from './Avatar.vue'

const client = useClient()
const el = ref<HTMLElement>()

const ROLES: Record<string, string> = { '~': 'Owner', '&': 'Admin', '@': 'Operator', '%': 'Half-op', '+': 'Voice' }
const roles = (prefix: string) => [...prefix].flatMap((c) => ROLES[c] ?? [])
const icon = (activity: string) =>
  activity.startsWith('Listening to') ? 'ph-music-notes' : activity.startsWith('Watching') ? 'ph-play' : 'ph-game-controller'

// Closes on a press outside it; a press on what opened it is that toggle's to handle.
function outside(e: PointerEvent) {
  const t = e.target as Node
  if (!el.value?.contains(t) && !client.card?.anchor.contains(t)) client.card = null
}
const escape = (e: KeyboardEvent) => e.key === 'Escape' && (client.card = null)
document.addEventListener('pointerdown', outside)
document.addEventListener('keydown', escape)

// The track's progress, ticking while it's open.
const now = ref(Date.now() / 1000)
let tick: ReturnType<typeof setInterval> | undefined
watch(
  () => client.card,
  (c) => {
    clearInterval(tick)
    now.value = Date.now() / 1000
    if (c?.m.music?.length) tick = setInterval(() => (now.value = Date.now() / 1000), 1000)
  },
  { immediate: true },
)
onUnmounted(() => {
  clearInterval(tick)
  document.removeEventListener('pointerdown', outside)
  document.removeEventListener('keydown', escape)
})
const at = (m: NonNullable<Member['music']>) => Math.min(Math.max(now.value - m.start, 0), m.length)
function clock(s: number) {
  const [h, m, sec] = [Math.floor(s / 3600), Math.floor(s / 60) % 60, Math.floor(s) % 60]
  const pad = (n: number) => String(n).padStart(2, '0')
  return h ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`
}

function dm(nick: string) {
  client.card = null
  client.open(nick)
}
</script>

<template>
  <div
    v-if="client.card"
    ref="el"
    class="card"
    role="dialog"
    :aria-label="client.card.m.shown"
    :style="client.card.pos"
  >
    <template v-for="c in [client.card]" :key="c.m.nick">
      <div class="who">
        <Avatar
          :nick="c.m.nick"
          :shown="c.m.shown"
          :url="c.m.avatar"
          :size="64"
          :status="c.present ? (c.m.away ? 'away' : 'online') : undefined"
        />
        <div>
          <b class="big" :style="{ color: nickColor(c.m.nick) }">{{ c.m.shown }}</b>
          <div v-if="c.m.shown !== c.m.nick" class="dim">{{ c.m.nick }}</div>
        </div>
      </div>
      <div v-if="c.m.activity">
        <i class="ph" :class="icon(c.m.activity)" />
        <a v-if="c.m.music?.url" :href="c.m.music.url" target="_blank" rel="noopener noreferrer">{{ c.m.activity }}</a>
        <template v-else>{{ c.m.activity }}</template>
      </div>
      <div v-if="c.m.music?.length" class="track dim small">
        {{ clock(at(c.m.music)) }}
        <progress :value="at(c.m.music)" :max="c.m.music.length" />
        {{ clock(c.m.music.length) }}
      </div>
      <hr />
      <dl>
        <template v-if="c.present">
          <dt>Status</dt>
          <dd>{{ c.m.away ? 'Away' : 'Online' }}</dd>
        </template>
        <dt>Account</dt>
        <dd v-if="c.m.account">{{ c.m.account }}</dd>
        <dd v-else class="dim"><i class="ph ph-ghost" /> Guest (not logged in)</dd>
        <template v-if="roles(c.m.prefix).length">
          <dt>Channel role</dt>
          <dd>{{ roles(c.m.prefix).join(', ') }}</dd>
        </template>
        <template v-if="c.m.oper">
          <dt>Network</dt>
          <dd><i class="ph ph-crown" /> IRC operator</dd>
        </template>
      </dl>
      <button v-if="c.m.nick !== client.state?.nick" @click="dm(c.m.nick)">Send DM</button>
    </template>
  </div>
</template>

<style scoped>
.card {
  position: fixed;
  z-index: 30;
  min-width: 240px;
  max-width: 320px;
  padding: 12px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 8px 24px #0008;
}
.track {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-top: 4px;
}
.track progress {
  width: 120px;
  height: 4px;
}
.who {
  display: flex;
  gap: 12px;
  align-items: center;
  margin-bottom: 4px;
}
.big {
  font-size: 1.3em;
}
hr {
  border: none;
  border-top: 1px solid var(--border);
}
dl {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 4px 12px;
  margin: 0 0 8px;
}
dt {
  font-size: 0.85em;
  opacity: 0.6;
}
dd {
  margin: 0;
}
</style>
