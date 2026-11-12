<!-- The selected channel's members; ops first, away dimmed. Click to DM. -->
<script setup lang="ts">
import { computed } from 'vue'
import { nickColor } from '../lib/format'
import { useClient } from '../stores/client'

const client = useClient()
const RANK = '~&@%+'
const sorted = computed(() => {
  const rank = (p: string) => (p ? RANK.indexOf(p[0]) : RANK.length)
  return [...(client.members ?? [])].sort(
    (a, b) => rank(a.prefix) - rank(b.prefix) || a.shown.localeCompare(b.shown),
  )
})
</script>

<template>
  <aside class="members">
    <h3 class="dim small">Members{{ client.members ? ` — ${client.members.length}` : '' }}</h3>
    <p v-if="!client.members" class="dim small">Loading…</p>
    <button
      v-for="m in sorted"
      :key="m.nick"
      class="member"
      :class="{ away: m.away }"
      :title="`${m.nick}${m.account ? ` (${m.account})` : ' (guest)'}${m.away ? ', away' : ''}`"
      @click="client.open(m.nick)"
    >
      <span class="prefix dim">{{ m.prefix.slice(0, 1) }}</span>
      <span class="ellipsis" :style="{ color: nickColor(m.nick) }">{{ m.shown }}</span>
    </button>
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
  margin: 8px;
  font-weight: normal;
  text-transform: uppercase;
}
.member {
  display: flex;
  gap: 4px;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
}
.member:hover {
  background: var(--surface);
}
.member.away {
  opacity: 0.5;
}
.prefix {
  width: 1ch;
  flex: none;
}
p {
  margin: 8px;
}
</style>
