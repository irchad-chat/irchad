<!-- One message: who and when (unless it continues the one before), its
     reply, text, embeds and reactions, and actions on hover. -->
<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Msg } from '../wasm/chad_web'
import { nickColor, time } from '../lib/format'
import { useClient } from '../stores/client'
import Spans from './Spans.vue'
import Embeds from './Embeds.vue'
import MediaImg from './MediaImg.vue'
import EmojiPicker from './EmojiPicker.vue'
import Avatar from './Avatar.vue'

const props = defineProps<{ msg: Msg; continued: boolean; parent?: Msg; seen: boolean }>()
const emit = defineEmits<{ reply: []; edit: []; jump: [msgid: string] }>()
const client = useClient()
const picking = ref(false)

const m = computed(() => props.msg)
const canAct = computed(() => !!m.value.msgid && !m.value.redacted && client.connected)
// Links whose image shows under the message: the image stands in for them.
const shown = ref(new Set<string>())
const spans = computed(() => {
  if (!shown.value.size) return m.value.spans
  const kept = m.value.spans.filter((s) => !(s.link && shown.value.has(s.text)))
  const blank = (i: number) => !kept[i]?.text.trim()
  let [a, b] = [0, kept.length]
  while (a < b && blank(a)) a++
  while (b > a && blank(b - 1)) b--
  return kept.slice(a, b)
})
const snippet = (t: string) => (t.length > 120 ? `${t.slice(0, 120)}…` : t)

function react(e: string) {
  picking.value = false
  client.react(m.value, e)
}
function remove() {
  if (confirm('Delete this message?')) client.redact(m.value)
}
</script>

<template>
  <div
    class="msg"
    :class="{ continued, mention: m.mention && client.buffer?.kind !== 'dm', pending: m.pending, notice: m.notice }"
    :data-msgid="m.msgid ?? undefined"
  >
    <button
      v-if="m.replyTo"
      class="reply-to dim"
      :title="parent ? 'Jump to the message' : 'Load the message'"
      @click="emit('jump', m.replyTo)"
    >
      <i class="ph ph-arrow-bend-down-right" />
      <template v-if="parent">
        <b :style="{ color: nickColor(parent.nick) }">{{ parent.shown }}</b>
        {{ parent.redacted ? 'deleted message' : snippet(parent.text) }}
      </template>
      <i v-else>Earlier message</i>
    </button>
    <div class="row">
      <div class="gutter">
        <template v-if="!continued">
          <button class="who" :aria-label="m.shown" @click="client.showCard($event.currentTarget as HTMLElement, 'right', m)">
            <Avatar :nick="m.nick" :shown="m.shown" :url="m.avatar" :size="40" />
          </button>
        </template>
        <span v-else class="hover-time dim">{{ time(m.time) }}</span>
      </div>
      <div class="body">
        <div v-if="!continued" class="head">
          <button
            class="who name"
            :style="{ color: nickColor(m.nick) }"
            :title="m.account ? `${m.nick} (${m.account})` : m.nick"
            @click="client.showCard($event.currentTarget as HTMLElement, 'right', m)"
          >
            {{ m.shown }}
          </button>
          <span class="dim small">{{ time(m.time) }}</span>
        </div>
        <div class="text">
          <i v-if="m.redacted" class="dim">deleted</i>
          <template v-else>
            <span v-if="m.action">* {{ m.shown }} </span>
            <MediaImg v-if="m.sticker" class="sticker" :url="m.sticker" :alt="m.text" :title="m.text">
              <Spans :spans="spans" />
            </MediaImg>
            <Spans v-else :spans="spans" />
            <span v-if="m.edited" class="dim small"> (edited)</span>
          </template>
          <span v-if="m.failed" class="err small"> {{ m.failed }}</span>
        </div>
        <Embeds v-if="!m.redacted && m.links.length" :links="m.links" @shown="(u) => (shown = new Set(shown).add(u))" />
        <div v-if="m.reactions.length" class="reactions">
          <button
            v-for="r in m.reactions"
            :key="r.emoji"
            class="reaction"
            :class="{ own: r.own }"
            :title="r.nicks.join(', ')"
            :disabled="!canAct"
            @click="client.react(m, r.emoji)"
          >
            <MediaImg v-if="r.image" class="emoji" :url="r.image" :alt="r.emoji">{{ r.emoji }}</MediaImg>
            <span v-else class="emoji">{{ r.emoji }}</span>
            {{ r.nicks.length }}
          </button>
        </div>
        <div v-if="seen" class="dim small"><i class="ph ph-eye" /> Seen</div>
      </div>
    </div>
    <div v-if="canAct" class="actions" :class="{ open: picking }">
      <button title="React" aria-label="React" @click="picking = !picking"><i class="ph ph-smiley" /></button>
      <button title="Reply" aria-label="Reply" @click="emit('reply')"><i class="ph ph-arrow-bend-up-left" /></button>
      <button v-if="m.own" title="Edit" aria-label="Edit" @click="emit('edit')"><i class="ph ph-pencil-simple" /></button>
      <button v-if="m.own" title="Delete" aria-label="Delete" @click="remove"><i class="ph ph-trash" /></button>
      <EmojiPicker v-if="picking" below @pick="react" @close="picking = false" />
    </div>
  </div>
</template>

<style scoped>
.who {
  padding: 0;
  background: none;
  border: none;
  cursor: pointer;
}
.who.name {
  font-weight: bold;
}
.who.name:hover {
  text-decoration: underline;
}
.msg {
  position: relative;
  padding: 2px 16px;
  margin-top: 12px;
}
.msg.continued {
  margin-top: 0;
}
.msg:hover {
  background: #ffffff08;
}
.msg.mention {
  background: #f9e2af14;
  box-shadow: inset 2px 0 var(--warn);
}
.msg.pending {
  opacity: 0.6;
}
.msg.notice .text {
  opacity: 0.8;
}
.row {
  display: flex;
  gap: 12px;
}
.gutter {
  width: 40px;
  flex: none;
  text-align: right;
}
.hover-time {
  display: block;
  visibility: hidden;
  white-space: nowrap;
  font-size: 0.7em;
  /* One text line (1.4 at full size), so it never makes the row taller. */
  line-height: 2em;
}
.msg:hover .hover-time {
  visibility: visible;
}
.body {
  min-width: 0;
  flex: 1;
}
.head {
  display: flex;
  gap: 8px;
  align-items: baseline;
}
.text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  line-height: 1.4;
}
.reply-to {
  display: block;
  margin-left: 52px;
  padding: 0;
  background: none;
  border: none;
  font-size: 0.85em;
  text-align: left;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: calc(100% - 52px);
}
.reactions {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 4px;
}
.reaction {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 8px;
  border-radius: 8px;
  font-size: 14px;
}
.reaction.own {
  border-color: var(--accent);
  background: #89b4fa22;
}
.sticker {
  display: block;
  max-width: 160px;
  max-height: 160px;
  object-fit: contain;
}
.reaction .emoji {
  width: 18px;
  height: 18px;
  font-size: 16px;
  line-height: 18px;
  text-align: center;
}
.actions {
  position: absolute;
  top: -14px;
  right: 16px;
  display: none;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 6px;
}
.msg:hover .actions,
.actions.open,
.actions:focus-within {
  display: flex;
}
.actions > button {
  background: none;
  border: none;
  padding: 2px 8px;
}
.actions > button:hover {
  background: var(--surface);
}
</style>
