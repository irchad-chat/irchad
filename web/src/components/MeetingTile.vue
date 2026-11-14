<!-- One person on the meeting grid: their camera (or screen), else their initial. -->
<script setup lang="ts">
import { ref, watch } from 'vue'
import type { VideoTrack } from 'livekit-client'
import type { Person } from '../stores/voice'
import Avatar from './Avatar.vue'

const props = defineProps<{ person: Person; track: VideoTrack | null; screen?: boolean }>()
const video = ref<HTMLVideoElement>()

watch(
  [() => props.track, video],
  ([track, el], [old, oldEl]) => {
    if (old && oldEl) (old as VideoTrack).detach(oldEl)
    if (track && el) track.attach(el)
  },
  { flush: 'post' },
)

const fullscreen = () => video.value?.requestFullscreen().catch(() => {})
</script>

<template>
  <figure class="tile" :class="{ speaking: person.speaking && !screen, screen }">
    <video
      v-if="track"
      ref="video"
      autoplay
      playsinline
      muted
      :class="{ mirror: person.local && !screen }"
      @dblclick="fullscreen"
    />
    <Avatar v-else :nick="person.identity" :shown="person.shown" :url="null" :size="72" />
    <figcaption>
      <i v-if="person.muted && !screen" class="ph ph-microphone-slash err" aria-label="Muted" />
      <span class="ellipsis">{{ screen ? `${person.shown}'s screen` : person.local ? `${person.shown} (you)` : person.shown }}</span>
    </figcaption>
  </figure>
</template>

<style scoped>
.tile {
  position: relative;
  margin: 0;
  display: grid;
  place-items: center;
  min-width: 0;
  min-height: 0;
  aspect-ratio: 16 / 9;
  background: var(--panel);
  border: 2px solid transparent;
  border-radius: 10px;
  overflow: hidden;
}
.tile.speaking {
  border-color: var(--ok);
}
.tile.screen {
  aspect-ratio: auto;
  height: 100%;
  background: #000;
}
video {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.screen video {
  object-fit: contain;
}
video.mirror {
  transform: scaleX(-1);
}
figcaption {
  position: absolute;
  left: 8px;
  bottom: 8px;
  max-width: calc(100% - 16px);
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 6px;
  background: #000a;
  font-size: 0.85em;
}
</style>
