<!-- The screen share or camera we're watching, above the chat. -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { VideoTrack } from 'livekit-client'
import { useVoice } from '../stores/voice'

const voice = useVoice()
const video = ref<HTMLVideoElement>()
const who = computed(() => voice.people.find((p) => p.identity === voice.watching?.identity))

// Attach the current track to the element, and let go of the last one.
watch(
  [() => voice.stage, video],
  ([track, el], [old, oldEl]) => {
    if (old && oldEl) (old as VideoTrack).detach(oldEl)
    if (track && el) track.attach(el)
  },
  { flush: 'post' },
)

const fullscreen = () => video.value?.requestFullscreen().catch(() => {})
</script>

<template>
  <section v-if="voice.watching" class="stage" aria-label="Watching">
    <header>
      <span class="ellipsis">
        {{ who?.shown ?? voice.watching.identity }}'s {{ voice.watching.camera ? 'camera' : 'screen' }}
      </span>
      <button title="Full screen" aria-label="Full screen" @click="fullscreen">
        <i class="ph ph-corners-out" />
      </button>
      <button title="Stop watching" aria-label="Stop watching" @click="voice.watch(null)">
        <i class="ph ph-x" />
      </button>
    </header>
    <video ref="video" autoplay playsinline muted />
    <p v-if="!voice.stage" class="dim small loading">Loading…</p>
  </section>
</template>

<style scoped>
.stage {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 55%;
  min-height: 160px;
  background: var(--bg-deep);
  border-bottom: 1px solid var(--border);
}
header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px 4px 16px;
}
header span {
  flex: 1;
  min-width: 0;
}
header button {
  background: none;
  border: none;
}
video {
  flex: 1;
  min-height: 0;
  width: 100%;
  object-fit: contain;
  background: #000;
}
.loading {
  position: absolute;
  inset: 50% 0 auto;
  text-align: center;
  margin: 0;
}
</style>
