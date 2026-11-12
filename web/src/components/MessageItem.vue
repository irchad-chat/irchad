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
    :class="{ continued, mention: m.mention, pending: m.pending, notice: m.notice }"
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
          <Avatar :nick="m.nick" :shown="m.shown" :url="m.avatar" :size="40" />
        </template>
        <span v-else class="hover-time dim">{{ time(m.time) }}</span>
      </div>
      <div class="body">
        <div v-if="!continued" class="head">
          <b :style="{ color: nickColor(m.nick) }" :title="m.account ? `${m.nick} (${m.account})` : m.nick">{{
            m.shown
          }}</b>
          <span class="dim small">{{ time(m.time) }}</span>
        </div>
        <div class="text">
          <i v-if="m.redacted" class="dim">deleted</i>
          <template v-else>
            <span v-if="m.action">* {{ m.shown }} </span>
            <Spans :spans="m.spans" />
            <span v-if="m.edited" class="dim small"> (edited)</span>
          </template>
          <span v-if="m.failed" class="err small"> {{ m.failed }}</span>
        </div>
        <Embeds v-if="!m.redacted && m.links.length" :links="m.links" />
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
            <template v-else>{{ r.emoji }}</template>
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
  gap: 4px;
  padding: 1px 8px;
  border-radius: 10px;
}
.reaction.own {
  border-color: var(--accent);
  background: #89b4fa22;
}
.reaction .emoji {
  width: 18px;
  height: 18px;
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
