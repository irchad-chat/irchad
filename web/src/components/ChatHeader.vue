<script setup lang="ts">
import { useClient } from '../stores/client'

defineProps<{ members: boolean }>()
const emit = defineEmits<{ toggleMembers: [] }>()
const client = useClient()
</script>

<template>
  <header v-if="client.buffer" class="chat-header">
    <b>{{ client.buffer.target }}</b>
    <span v-if="client.buffer.kind === 'dm' && client.buffer.online !== null" class="dim small">
      {{ client.buffer.online ? (client.buffer.away ? 'away' : 'online') : 'offline' }}
    </span>
    <span class="topic dim ellipsis" :title="client.buffer.topic">{{ client.buffer.topic }}</span>
    <button
      v-if="client.buffer.kind === 'channel'"
      :class="{ on: members }"
      :aria-pressed="members"
      title="Members"
      aria-label="Members"
      @click="emit('toggleMembers')"
    >
      👥
    </button>
  </header>
</template>

<style scoped>
.chat-header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  min-height: 24px;
}
.topic {
  flex: 1;
  min-width: 0;
}
button {
  background: none;
  border: none;
}
button.on {
  background: var(--surface);
}
</style>
