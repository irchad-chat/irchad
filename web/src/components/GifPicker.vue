<!-- GIF search through chadd (D30). Picks the GIF's URL, which goes in the
     message as a link every client can open. -->
<script setup lang="ts">
import { onMounted, ref, shallowRef } from 'vue'
import type { Gif } from '../wasm/chad_web'
import { message, useClient } from '../stores/client'
import MediaImg from './MediaImg.vue'

const emit = defineEmits<{ pick: [url: string]; close: [] }>()
const client = useClient()
const query = ref('')
const gifs = shallowRef<Gif[]>([])
const next = ref<string | null>(null)
const error = ref<string>()
const loading = ref(false)
const search = ref<HTMLInputElement>()
// Which search the results are for, so a slow old one can't land late.
let asked = 0

async function load(more = false) {
  const n = ++asked
  loading.value = true
  try {
    const page = await client.gifs(query.value.trim(), more ? (next.value ?? undefined) : undefined)
    if (n !== asked) return
    gifs.value = more ? [...gifs.value, ...page.gifs] : page.gifs
    next.value = page.next
    error.value = undefined
  } catch (err) {
    if (n === asked) error.value = message(err)
  } finally {
    if (n === asked) loading.value = false
  }
}

let debounce: ReturnType<typeof setTimeout> | undefined
function typed() {
  clearTimeout(debounce)
  debounce = setTimeout(() => load(), 300)
}

function scrolled(e: Event) {
  const el = e.target as HTMLElement
  if (next.value && !loading.value && el.scrollTop + el.clientHeight > el.scrollHeight - 200) load(true)
}

onMounted(() => {
  load()
  search.value?.focus()
})
</script>

<template>
  <div class="backdrop" @click="emit('close')" />
  <div class="picker" role="dialog" aria-label="GIFs" @keydown.esc="emit('close')">
    <input
      ref="search"
      v-model="query"
      :aria-label="`Search ${client.state?.gifs ?? ''}`"
      :placeholder="`Search ${client.state?.gifs ?? 'GIFs'}`"
      @input="typed"
    />
    <p v-if="error" class="err">{{ error }}</p>
    <div class="grid" @scroll="scrolled">
      <button v-for="g in gifs" :key="g.id" :title="g.title" :aria-label="g.title || 'GIF'" @click="emit('pick', g.url)">
        <MediaImg :url="g.preview" :alt="g.title" :style="{ aspectRatio: `${g.width} / ${g.height}` }" />
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
  width: min(420px, calc(100vw - 32px));
  height: 420px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 8px 24px #0008;
}
.grid {
  flex: 1;
  overflow: auto;
  columns: 2;
  column-gap: 4px;
}
.grid button {
  display: block;
  width: 100%;
  padding: 0;
  margin-bottom: 4px;
  border: none;
  background: var(--surface);
  break-inside: avoid;
}
.grid img {
  display: block;
  width: 100%;
  border-radius: 4px;
}
</style>
