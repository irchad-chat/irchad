<!-- Writing a message: @ completion, :shortcodes:, replies and edits,
     uploads (button, paste, drop), emoji and GIFs. -->
<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import type { Nick } from '../wasm/chad_web'
import { insertNick } from '../lib/format'
import { useClient } from '../stores/client'
import EmojiPicker from './EmojiPicker.vue'
import GifPicker from './GifPicker.vue'

const client = useClient()
const draft = ref('')
const box = ref<HTMLTextAreaElement>()
const file = ref<HTMLInputElement>()
const picker = ref<'emoji' | 'gif' | null>(null)
// Nicks picked from the @ list (D66); the core keeps those still in the text.
let mentions: string[] = []
// Each buffer keeps its unsent text.
const drafts = new Map<string, string>()

const suggestions = ref<Nick[]>([])
const pick = ref(0)
// Where the `@word` being completed starts.
let atFrom = -1

const enabled = computed(() => !!client.selected && client.connected && client.buffer?.kind !== 'server')
const canUpload = computed(() => enabled.value && client.state?.uploads != null)
const placeholder = computed(() => {
  if (client.editing) return 'Edit message'
  const b = client.buffer
  return b ? `Message ${b.kind === 'dm' ? '@' : ''}${b.target}` : ''
})

watch(
  () => client.selected,
  async (now, was) => {
    if (was) drafts.set(was, draft.value)
    draft.value = (now && drafts.get(now)) || ''
    mentions = []
    suggestions.value = []
    // Not on touch screens: focusing would pop the keyboard over the messages.
    if (!matchMedia('(pointer: fine)').matches) return
    await nextTick()
    box.value?.focus()
  },
)
watch(
  () => client.editing,
  async (m) => {
    if (!m) return
    client.replying = null
    draft.value = m.text
    mentions = [...m.mentions]
    await nextTick()
    box.value?.focus()
  },
)
watch(
  () => client.replying,
  (m) => {
    if (m) {
      client.editing = null
      box.value?.focus()
    }
  },
)

function input() {
  const el = box.value!
  const done = client.replaceShortcodes(draft.value, el.selectionStart)
  if (done) {
    draft.value = done.text
    nextTick(() => el.setSelectionRange(done.cursor, done.cursor))
  }
  if (!client.editing) client.typing(draft.value.trim() !== '')
  complete()
}

function complete() {
  const el = box.value!
  const before = draft.value.slice(0, el.selectionStart)
  const at = /(^|\s)@([^\s@]*)$/.exec(before)
  if (!at) {
    suggestions.value = []
    return
  }
  atFrom = before.length - at[2].length - 1
  suggestions.value = client.nicks(at[2])
  pick.value = 0
}

function choose(n: Nick) {
  const el = box.value!
  const end = el.selectionStart
  const text = insertNick(n.nick, draft.value.slice(0, atFrom).trim() === '')
  draft.value = draft.value.slice(0, atFrom) + text + draft.value.slice(end)
  if (!mentions.includes(n.nick)) mentions.push(n.nick)
  suggestions.value = []
  const cursor = atFrom + text.length
  nextTick(() => {
    el.focus()
    el.setSelectionRange(cursor, cursor)
  })
}

function insert(text: string) {
  const el = box.value!
  const [a, b] = [el.selectionStart, el.selectionEnd]
  draft.value = draft.value.slice(0, a) + text + draft.value.slice(b)
  picker.value = null
  nextTick(() => {
    el.focus()
    el.setSelectionRange(a + text.length, a + text.length)
  })
}

function keydown(e: KeyboardEvent) {
  if (suggestions.value.length) {
    const n = suggestions.value.length
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      pick.value = (pick.value + (e.key === 'ArrowDown' ? 1 : n - 1)) % n
      return e.preventDefault()
    }
    if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
      choose(suggestions.value[pick.value])
      return e.preventDefault()
    }
    if (e.key === 'Escape') {
      suggestions.value = []
      return e.preventDefault()
    }
  }
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    submit()
  } else if (e.key === 'Escape' && (client.replying || client.editing)) {
    cancel()
  } else if (e.key === 'ArrowUp' && draft.value === '') {
    const last = client.messages.findLast((m) => m.own && m.msgid && !m.redacted)
    if (last) {
      e.preventDefault()
      client.editing = last
    }
  }
}

function cancel() {
  if (client.editing) draft.value = ''
  client.replying = client.editing = null
}

function submit() {
  const text = draft.value.trim()
  if (!text && !client.editing) return
  if (client.editing) {
    // Emptied out: that's a delete, which `Delete` already does with a confirm.
    if (text && text !== client.editing.text) client.edit(client.editing, text, mentions)
  } else {
    client.send(text, client.replying?.msgid ?? null, mentions)
  }
  draft.value = ''
  mentions = []
  client.replying = client.editing = null
  client.typing(false)
}

function upload(files: FileList | null | undefined) {
  for (const f of files ?? []) client.upload(f)
}

function paste(e: ClipboardEvent) {
  if (!canUpload.value || !e.clipboardData?.files.length) return
  e.preventDefault()
  upload(e.clipboardData.files)
}

function drop(e: DragEvent) {
  if (!canUpload.value || !e.dataTransfer?.files.length) return
  e.preventDefault()
  upload(e.dataTransfer.files)
}

// A sticker is posted as its `:name:`, a message of its own (D103).
function gif(url: string) {
  picker.value = null
  client.send(url, client.replying?.msgid ?? null, [])
  client.replying = null
}
</script>

<template>
  <div class="composer" @dragover.prevent @drop="drop">
    <div v-if="client.replying || client.editing" class="mode small">
      <span v-if="client.editing">Editing your message</span>
      <span v-else
        >Replying to <b>{{ client.replying!.shown }}</b></span
      >
      <button class="link" aria-label="Cancel" @click="cancel"><i class="ph ph-x" /></button>
    </div>
    <ul v-if="suggestions.length" class="suggest" role="listbox">
      <li
        v-for="(n, i) in suggestions"
        :key="n.nick"
        role="option"
        :aria-selected="i === pick"
        :class="{ on: i === pick }"
        @mousedown.prevent="choose(n)"
      >
        {{ n.shown }} <span v-if="n.shown !== n.nick" class="dim">{{ n.nick }}</span>
      </li>
    </ul>
    <div class="bar">
      <button v-if="canUpload" title="Upload a file" aria-label="Upload a file" @click="file?.click()"><i class="ph ph-paperclip" /></button>
      <input ref="file" type="file" multiple hidden @change="upload(file?.files); file!.value = ''" />
      <textarea
        ref="box"
        v-model="draft"
        rows="1"
        :aria-label="placeholder"
        :placeholder="placeholder"
        :disabled="!enabled"
        @input="input"
        @click="complete"
        @keydown="keydown"
        @paste="paste"
        @blur="suggestions = []"
      />
      <div class="pickers">
        <button
          v-if="client.state?.gifs"
          title="GIFs"
          aria-label="GIFs"
          :disabled="!enabled"
          @click="picker = picker === 'gif' ? null : 'gif'"
        >
          GIF
        </button>
        <button title="Emoji" aria-label="Emoji" :disabled="!enabled" @click="picker = picker === 'emoji' ? null : 'emoji'">
          <i class="ph ph-smiley" />
        </button>
        <EmojiPicker v-if="picker === 'emoji'" stickers @pick="insert" @sticker="gif" @close="picker = null" />
        <GifPicker v-if="picker === 'gif'" @pick="gif" @close="picker = null" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.composer {
  position: relative;
  padding: 0 16px 16px;
}
.mode {
  display: flex;
  justify-content: space-between;
  padding: 4px 12px;
  background: var(--panel);
  border-radius: 8px 8px 0 0;
}
.bar {
  display: flex;
  align-items: flex-end;
  gap: 4px;
  padding: 4px;
  background: var(--surface);
  border-radius: 8px;
}
.bar > button,
.pickers > button {
  background: none;
  border: none;
  min-width: 36px;
  height: 36px;
}
.bar .ph {
  font-size: 18px;
  vertical-align: middle;
}
textarea {
  flex: 1;
  resize: none;
  field-sizing: content;
  max-height: 40vh;
  min-height: 36px;
  border: none;
  background: none;
  line-height: 1.4;
  outline: none;
}
.pickers {
  position: relative;
  display: flex;
}
.suggest {
  position: absolute;
  bottom: 100%;
  left: 16px;
  right: 16px;
  margin: 0;
  padding: 4px;
  list-style: none;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 8px;
  max-height: 240px;
  overflow: auto;
}
.suggest li {
  padding: 4px 8px;
  border-radius: 4px;
  cursor: pointer;
}
.suggest li.on {
  background: var(--surface);
}
</style>
