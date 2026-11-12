<!-- Standard and the network's custom emoji. Picks the character, or a
     custom one's `:name:`. -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { emojis } from '../wasm/chad_web'
import { useClient } from '../stores/client'
import MediaImg from './MediaImg.vue'

defineProps<{ below?: boolean }>()
const emit = defineEmits<{ pick: [emoji: string]; close: [] }>()
const client = useClient()
const query = ref('')
const search = ref<HTMLInputElement>()

const all = emojis('')
const standard = computed(() => (query.value.trim() ? emojis(query.value) : all))
const custom = computed(() => {
  const q = query.value.trim().toLowerCase()
  return client.emoji.filter((e) => e.name.includes(q))
})

onMounted(() => {
  client.refreshEmoji()
  search.value?.focus()
})
</script>

<template>
  <div class="backdrop" @click="emit('close')" />
  <div class="picker" :class="{ below }" role="dialog" aria-label="Emoji" @keydown.esc="emit('close')">
    <input ref="search" v-model="query" aria-label="Search emoji" placeholder="Search emoji" />
    <div class="grid">
      <button
        v-for="e in custom"
        :key="e.name"
        :title="`:${e.name}:`"
        :aria-label="e.name"
        @click="emit('pick', `:${e.name}:`)"
      >
        <MediaImg class="emoji" :url="e.url" :alt="e.name">:{{ e.name }}:</MediaImg>
      </button>
      <button v-for="e in standard" :key="e.emoji" :title="e.name" :aria-label="e.name" @click="emit('pick', e.emoji)">
        {{ e.emoji }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.picker {
  position: absolute;
  bottom: 100%;
  right: 0;
  z-index: 20;
  width: min(340px, calc(100vw - 32px));
  height: 360px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 8px 24px #0008;
}
.picker.below {
  bottom: auto;
  top: 100%;
}
.grid {
  flex: 1;
  overflow: auto;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(36px, 1fr));
  align-content: start;
}
.grid button {
  font-size: 22px;
  padding: 4px;
  background: none;
  border: none;
  overflow: hidden;
}
.grid button:hover {
  background: var(--surface);
}
.grid .emoji {
  width: 24px;
  height: 24px;
  font-size: 10px;
}
</style>
