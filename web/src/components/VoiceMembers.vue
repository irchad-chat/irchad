<!-- Who's in a channel's voice room, under its row. In our own call the live
     list (speaking, muted, sharing) replaces the `chad/voice` one. -->
<script setup lang="ts">
import { computed } from 'vue'
import type { Buffer } from '../wasm/chad_web'
import { useVoice } from '../stores/voice'

const props = defineProps<{ buffer: Buffer }>()
const voice = useVoice()
const live = computed(() => voice.call?.connected && voice.call.channel === props.buffer.target)
const isWatched = (identity: string, camera: boolean) =>
  voice.watching?.identity === identity && voice.watching.camera === camera
const toggle = (identity: string, camera: boolean) =>
  voice.watch(isWatched(identity, camera) ? null : { identity, camera })
</script>

<template>
  <ul v-if="live" class="voice">
    <li v-for="p in voice.people" :key="p.identity" :class="{ speaking: p.speaking }">
      <span class="ellipsis">{{ p.shown }}</span>
      <i v-if="p.muted" class="ph ph-microphone-slash dim" title="Muted" />
      <button
        v-if="p.camera && !p.local"
        :class="{ on: isWatched(p.identity, true) }"
        :aria-pressed="isWatched(p.identity, true)"
        :title="`Watch ${p.shown}'s camera`"
        :aria-label="`Watch ${p.shown}'s camera`"
        @click="toggle(p.identity, true)"
      >
        <i class="ph ph-video-camera" />
      </button>
      <button
        v-if="p.screen && !p.local"
        class="live"
        :class="{ on: isWatched(p.identity, false) }"
        :aria-pressed="isWatched(p.identity, false)"
        :title="`Watch ${p.shown}'s screen${p.watchers ? ` (${p.watchers} watching)` : ''}`"
        :aria-label="`Watch ${p.shown}'s screen`"
        @click="toggle(p.identity, false)"
      >
        LIVE<template v-if="p.watchers"> · {{ p.watchers }}</template>
      </button>
    </li>
  </ul>
  <ul v-else-if="buffer.inVoice.length" class="voice">
    <li v-for="nick in buffer.inVoice" :key="nick">
      <span class="ellipsis">{{ nick }}</span>
    </li>
  </ul>
</template>

<style scoped>
.voice {
  list-style: none;
  margin: 0 0 4px;
  padding: 0 0 0 24px;
  font-size: 0.9em;
}
li {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 4px;
  border-radius: 6px;
  border: 1px solid transparent;
}
li > span {
  flex: 1;
  min-width: 0;
}
li.speaking {
  border-color: var(--ok);
}
button {
  background: none;
  border: none;
  padding: 0 4px;
}
button.on {
  background: var(--surface);
}
.live {
  background: var(--err);
  color: var(--bg);
  font-size: 0.75em;
  font-weight: bold;
}
.live.on {
  background: var(--accent);
}
</style>
