<!-- Standard and the network's custom emoji, as the desktop's picker. Picks
     the character, or a custom one's `:name:`. With `stickers`, the network's
     stickers have a tab of their own (D103). -->
<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { emojis } from '../wasm/chad_web'
import { useClient } from '../stores/client'
import MediaImg from './MediaImg.vue'

const props = defineProps<{ below?: boolean; stickers?: boolean }>()
const emit = defineEmits<{ pick: [emoji: string]; sticker: [code: string]; close: [] }>()
const client = useClient()
const query = ref('')
const search = ref<HTMLInputElement>()
const scroller = ref<HTMLElement>()

const PER_ROW = 9
const ROW = 36
// Rows past the visible ones kept drawn, so scrolling doesn't show gaps.
const SPARE = 4

const all = emojis('')
const standard = computed(() => (query.value.trim() ? emojis(query.value) : all))
const custom = computed(() => {
  const q = query.value.trim().toLowerCase()
  return client.emoji.filter((e) => !e.sticker && e.name.includes(q))
})
const stickerSet = computed(() => (props.stickers ? client.emoji.filter((e) => e.sticker) : []))
const tab = ref<'emoji' | 'stickers'>('emoji')
// The emoji grid is mounted anew: measure it again.
watch(tab, (t) => t === 'emoji' && scrolled(), { flush: 'post' })

// Only the rows in view are drawn: ~1900 color glyphs at once garble some
// GPU glyph caches, and it's slow anyway.
const scrollTop = ref(0)
const height = ref(0)
const rows = computed(() => Math.ceil(standard.value.length / PER_ROW))
const first = computed(() => Math.max(0, Math.floor(scrollTop.value / ROW) - SPARE))
const shown = computed(() => {
  const last = Math.min(rows.value, Math.ceil((scrollTop.value + height.value) / ROW) + SPARE)
  return standard.value.slice(first.value * PER_ROW, last * PER_ROW)
})
watch(query, () => scroller.value?.scrollTo(0, 0))

function scrolled() {
  scrollTop.value = scroller.value!.scrollTop
  height.value = scroller.value!.clientHeight
}

// A search that names a custom one means it.
function enter() {
  const c = query.value.trim() && custom.value[0]
  const pick = c ? `:${c.name}:` : standard.value[0]?.emoji
  if (pick) emit('pick', pick)
}

onMounted(() => {
  client.refreshEmoji()
  search.value?.focus()
  scrolled()
})
</script>

<template>
  <div class="backdrop" @click="emit('close')" />
  <div class="picker" :class="{ below }" role="dialog" aria-label="Emoji" @keydown.esc="emit('close')">
    <div v-if="stickerSet.length" class="tabs" role="tablist">
      <button role="tab" :aria-selected="tab === 'emoji'" :class="{ on: tab === 'emoji' }" @click="tab = 'emoji'">Emoji</button>
      <button role="tab" :aria-selected="tab === 'stickers'" :class="{ on: tab === 'stickers' }" @click="tab = 'stickers'">
        Stickers
      </button>
    </div>
    <div v-if="tab === 'stickers'" class="scroll stickers">
      <button v-for="e in stickerSet" :key="e.name" :title="`:${e.name}:`" :aria-label="`:${e.name}:`" @click="emit('sticker', `:${e.name}:`)">
        <MediaImg :url="e.url" :alt="e.name"><span class="small ellipsis">{{ e.name }}</span></MediaImg>
      </button>
    </div>
    <template v-else>
    <input
      ref="search"
      v-model="query"
      aria-label="Find an emoji"
      placeholder="Find an emoji"
      @keydown.enter.prevent="enter"
    />
    <template v-if="custom.length">
      <div class="grid custom">
        <button v-for="e in custom" :key="e.name" :title="`:${e.name}:`" :aria-label="`:${e.name}:`" @click="emit('pick', `:${e.name}:`)">
          <MediaImg class="emoji" :url="e.url" :alt="e.name"><span class="small ellipsis">{{ e.name }}</span></MediaImg>
        </button>
      </div>
      <hr />
    </template>
    <div ref="scroller" class="scroll" @scroll.passive="scrolled">
      <div :style="{ height: `${rows * ROW}px` }">
        <div class="grid" :style="{ transform: `translateY(${first * ROW}px)` }">
          <button v-for="e in shown" :key="e.emoji" :title="e.name" :aria-label="e.name" @click="emit('pick', e.emoji)">
            {{ e.emoji }}
          </button>
        </div>
      </div>
      <p v-if="!standard.length && !custom.length" class="dim">No emoji found.</p>
    </div>
    </template>
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
.scroll {
  flex: 1;
  overflow: auto;
}
.grid {
  display: grid;
  grid-template-columns: repeat(9, 1fr);
  grid-auto-rows: 36px;
}
.custom {
  flex: none;
  max-height: 108px;
  overflow: auto;
}
.grid button {
  display: grid;
  place-items: center;
  font-size: 22px;
  line-height: 1;
  padding: 0;
  background: none;
  border: none;
  overflow: hidden;
}
.grid button:hover {
  background: var(--surface);
}
.grid .emoji {
  width: 26px;
  height: 26px;
}
.tabs {
  display: flex;
  gap: 4px;
}
.tabs button {
  padding: 2px 10px;
  background: none;
  border: none;
  border-radius: 4px;
}
.tabs .on {
  background: var(--surface);
}
.stickers {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  grid-auto-rows: 104px;
  align-content: start;
}
.stickers button {
  display: grid;
  place-items: center;
  padding: 6px;
  background: none;
  border: none;
  overflow: hidden;
}
.stickers button:hover {
  background: var(--surface);
}
.stickers img {
  max-width: 100%;
  max-height: 92px;
  object-fit: contain;
}
hr {
  width: 100%;
  margin: 0;
  border: none;
  border-top: 1px solid var(--border);
}
p {
  margin: 8px;
}
</style>
