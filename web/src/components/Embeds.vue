<!-- What a message's links show under it: images, our own videos and files,
     and a preview card for the first page. -->
<script setup lang="ts">
import { computed, ref, watchEffect } from 'vue'
import type { Link, LinkPreview } from '../wasm/chad_web'
import { useClient } from '../stores/client'
import MediaImg from './MediaImg.vue'

const props = defineProps<{ links: Link[] }>()
// An image that's showing, so its link in the text can go.
const emit = defineEmits<{ shown: [url: string] }>()
const client = useClient()
const own = (url: string) => URL.canParse(url) && new URL(url).origin === client.network
const failed = ref(new Set<string>())

// A few at most: a message full of links shouldn't fill the screen.
const shown = computed(() =>
  props.links
    .filter((l) => l.kind !== 'page' && !failed.value.has(l.url) && (l.kind !== 'video' || own(l.url)))
    .slice(0, 4),
)
const page = computed(() => props.links.find((l) => l.kind === 'page'))
const card = ref<LinkPreview | null>(null)
watchEffect(() => {
  const url = page.value?.url
  card.value = null
  if (url) client.preview(url).then((p) => url === page.value?.url && (card.value = p))
})
const fileName = (url: string) => decodeURIComponent(new URL(url).pathname.split('/').pop() || url)
const fail = (url: string) => (failed.value = new Set(failed.value).add(url))
</script>

<template>
  <div v-if="shown.length || card" class="embeds">
    <template v-for="l in shown" :key="l.url">
      <a v-if="l.kind === 'image'" :href="l.url" target="_blank" rel="noopener noreferrer">
        <MediaImg class="embed-img" :url="l.url" @failed="fail(l.url)" @loaded="emit('shown', l.url)" />
      </a>
      <video v-else-if="l.kind === 'video'" class="embed-img" :src="l.url" controls preload="metadata" />
      <a v-else class="file" :href="l.url" target="_blank" rel="noopener noreferrer" download><i class="ph ph-file" /> {{ fileName(l.url) }}</a>
    </template>
    <a v-if="card && page" class="card" :href="page.url" target="_blank" rel="noopener noreferrer">
      <div>
        <div v-if="card.site" class="dim small">{{ card.site }}</div>
        <b v-if="card.title">{{ card.title }}</b>
        <div v-if="card.description" class="desc">{{ card.description }}</div>
      </div>
      <MediaImg v-if="card.image" class="thumb" :url="card.image" />
    </a>
  </div>
</template>

<style scoped>
.embeds {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  margin-top: 4px;
}
.embed-img {
  display: block;
  max-width: min(400px, 100%);
  max-height: 300px;
  border-radius: 6px;
}
.file,
.card {
  display: flex;
  gap: 12px;
  padding: 8px 12px;
  background: var(--surface);
  border-left: 3px solid var(--accent);
  border-radius: 6px;
  color: inherit;
  text-decoration: none;
  max-width: min(480px, 100%);
}
.card b {
  color: var(--accent);
}
.desc {
  font-size: 0.9em;
  opacity: 0.85;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.thumb {
  width: 80px;
  height: 80px;
  object-fit: cover;
  border-radius: 4px;
  flex: none;
}
</style>
