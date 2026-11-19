<!-- The selected channel's members as the desktop shows them: online then
     away, each with avatar, badges and activity. Click for their card,
     right-click to block. -->
<script setup lang="ts">
import { computed } from 'vue'
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

const icon = (activity: string) =>
  activity.startsWith('Listening to') ? 'ph-music-notes' : activity.startsWith('Watching') ? 'ph-play' : 'ph-game-controller'
const isMe = (m: Member) => m.nick === client.state?.nick

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
        @click="client.showCard($event.currentTarget as HTMLElement, 'left', m)"
        @contextmenu.prevent="block(m)"
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
p {
  margin: 8px;
}
</style>
