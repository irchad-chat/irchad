<!-- Someone's avatar, else their initial on their color; with `status`, the
     online/away dot on its lower right. -->
<script setup lang="ts">
import { ref, watch } from 'vue'
import { nickColor } from '../lib/format'
import MediaImg from './MediaImg.vue'

const props = defineProps<{ nick: string; shown: string; url: string | null; size: number; status?: 'online' | 'away' }>()
const failed = ref(false)
watch(
  () => props.url,
  () => (failed.value = false),
)
</script>

<template>
  <div class="avatar" :style="{ width: `${size}px`, height: `${size}px` }">
    <MediaImg v-if="url && !failed" :url="url" @failed="failed = true">
      <div class="letter" :style="{ background: nickColor(nick) }">{{ shown.slice(0, 1).toUpperCase() }}</div>
    </MediaImg>
    <div v-else class="letter" :style="{ background: nickColor(nick) }">{{ shown.slice(0, 1).toUpperCase() }}</div>
    <span v-if="status" class="dot" :class="status" />
  </div>
</template>

<style scoped>
.avatar {
  position: relative;
  flex: none;
  font-size: calc(v-bind(size) * 0.4px);
}
.avatar img,
.letter {
  width: 100%;
  height: 100%;
  border-radius: 50%;
  object-fit: cover;
}
.letter {
  display: grid;
  place-items: center;
  color: var(--bg);
  font-weight: bold;
}
.dot {
  position: absolute;
  right: 0;
  bottom: 0;
  width: 32%;
  height: 32%;
  border-radius: 50%;
  background: #23a55a;
  box-shadow: 0 0 0 2px var(--panel);
}
.dot.away {
  background: #f0b232;
}
</style>
