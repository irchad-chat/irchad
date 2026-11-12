<!-- The selected channel's members as the desktop shows them: online then
     away, each with avatar, badges and activity; a card on hover. Click to
     DM, right-click to block. -->
<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Member } from '../wasm/chad_web'
import { nickColor } from '../lib/format'
import { useClient } from '../stores/client'
import Avatar from './Avatar.vue'

const client = useClient()
// chad-core already orders them: rank, then nick.
const sections = computed(() =>
  (
    [
      ['Online', false],
      ['Away', true],
    ] as const
  )
    .map(([title, away]) => ({ title, list: (client.members ?? []).filter((m) => m.away === away) }))
    .filter((s) => s.list.length),
)

const ROLES: Record<string, string> = { '~': 'Owner', '&': 'Admin', '@': 'Operator', '%': 'Half-op', '+': 'Voice' }
const roles = (prefix: string) => [...prefix].flatMap((c) => ROLES[c] ?? [])
const icon = (activity: string) => (activity.startsWith('Listening to') ? 'ph-music-notes' : 'ph-game-controller')
const isMe = (m: Member) => m.nick === client.state?.nick

// The card's member and where it goes: left of the hovered row.
const card = ref<{ m: Member; top: number; right: number }>()
function hover(m: Member, e: MouseEvent) {
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  card.value = { m, top: Math.min(r.top, innerHeight - 260), right: innerWidth - r.left + 8 }
}

function block(m: Member) {
  if (!isMe(m) && confirm(`Block ${m.shown}? You won't see anything from them on this device.`))
    client.block(m.nick, m.account)
}
</script>

<template>
  <aside class="members">
    <p v-if="!client.members" class="dim small">Loading…</p>
    <template v-for="s in sections" :key="s.title">
      <h3 class="dim small">{{ s.title }} — {{ s.list.length }}</h3>
      <button
        v-for="m in s.list"
        :key="m.nick"
        class="member"
        :class="{ away: m.away }"
        :aria-label="m.shown"
        @click="!isMe(m) && client.open(m.nick)"
        @contextmenu.prevent="block(m)"
        @mouseenter="hover(m, $event)"
        @mouseleave="card = undefined"
      >
        <Avatar :nick="m.nick" :shown="m.shown" :url="m.avatar" :size="32" :status="m.away ? 'away' : 'online'" />
        <span class="lines">
          <span class="name">
            <span class="ellipsis" :style="{ color: nickColor(m.nick) }">{{ m.shown }}</span>
            <i v-if="m.oper" class="ph ph-crown dim" title="IRC operator" />
            <i v-if="!m.account" class="ph ph-ghost dim" title="Guest" />
          </span>
          <span v-if="m.activity" class="ellipsis dim small"><i class="ph" :class="icon(m.activity)" /> {{ m.activity }}</span>
        </span>
      </button>
    </template>
    <div v-if="card" class="card" :style="{ top: `${card.top}px`, right: `${card.right}px` }">
      <div class="who">
        <Avatar :nick="card.m.nick" :shown="card.m.shown" :url="card.m.avatar" :size="64" :status="card.m.away ? 'away' : 'online'" />
        <div>
          <b class="big" :style="{ color: nickColor(card.m.nick) }">{{ card.m.shown }}</b>
          <div v-if="card.m.shown !== card.m.nick" class="dim">{{ card.m.nick }}</div>
        </div>
      </div>
      <div v-if="card.m.activity"><i class="ph" :class="icon(card.m.activity)" /> {{ card.m.activity }}</div>
      <hr />
      <dl>
        <dt>Status</dt>
        <dd>{{ card.m.away ? 'Away' : 'Online' }}</dd>
        <dt>Account</dt>
        <dd v-if="card.m.account">{{ card.m.account }}</dd>
        <dd v-else class="dim"><i class="ph ph-ghost" /> Guest (not logged in)</dd>
        <template v-if="roles(card.m.prefix).length">
          <dt>Channel role</dt>
          <dd>{{ roles(card.m.prefix).join(', ') }}</dd>
        </template>
        <template v-if="card.m.oper">
          <dt>Network</dt>
          <dd><i class="ph ph-crown" /> IRC operator</dd>
        </template>
      </dl>
      <div v-if="!isMe(card.m)" class="dim small">Click to message · right-click to block</div>
    </div>
  </aside>
</template>

<style scoped>
.members {
  width: 220px;
  flex: none;
  overflow: auto;
  padding: 8px;
  background: var(--panel);
}
h3 {
  margin: 12px 6px 4px;
  font-weight: normal;
  text-transform: uppercase;
}
h3:first-child {
  margin-top: 0;
}
.member {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  height: 40px;
  padding: 0 4px 0 6px;
  text-align: left;
  background: none;
  border: none;
  border-radius: 4px;
}
.member:hover {
  background: var(--surface);
}
.member.away .name > .ellipsis {
  opacity: 0.55;
}
.lines {
  display: flex;
  flex-direction: column;
  min-width: 0;
}
.name {
  display: flex;
  gap: 4px;
  min-width: 0;
}
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
  pointer-events: none;
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
  margin: 0 0 4px;
}
dt {
  font-size: 0.85em;
  opacity: 0.6;
}
dd {
  margin: 0;
}
p {
  margin: 8px;
}
</style>
