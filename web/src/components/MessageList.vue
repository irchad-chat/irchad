<!-- The selected buffer's messages: sticks to the bottom, loads older ones
     scrolling up, and jumps to replies. -->
<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import type { Msg } from '../wasm/chad_web'
import { day, sameDay } from '../lib/format'
import { useClient } from '../stores/client'
import MessageItem from './MessageItem.vue'

const client = useClient()
const el = ref<HTMLElement>()
const content = ref<HTMLElement>()
// At the bottom, as of the reader's last scroll: images, previews and avatars
// that load later grow the log, and must not push the newest message out of view.
let stuck = true
const atBottom = (e: HTMLElement) => e.scrollHeight - e.scrollTop - e.clientHeight < 40
// Only the reader un-sticks the log: browsers also fire scroll events for layout
// (scroll anchoring, clamping, late content), and reading those as intent left it unstuck.
// shortcut: find-in-page while stuck gets pulled back down; track it if anyone minds.
let reading = 0
let dragging = false
const touched = () => (reading = performance.now())
function onScroll() {
  const e = el.value
  if (!e) return
  if (atBottom(e)) stuck = true
  else if (dragging || performance.now() - reading < 1000) stuck = false
  else if (stuck) e.scrollTop = e.scrollHeight
  maybeOlder()
}
// Scrollbar drags start on the log and can run as long as the button is held.
function grab() {
  dragging = true
  window.addEventListener('pointerup', () => ((dragging = false), touched()), { once: true })
}
const grown = new ResizeObserver(() => {
  if (stuck && el.value) el.value.scrollTop = el.value.scrollHeight
})
// Coming back to the tab: catch up on growth we didn't get to see.
function shown() {
  if (!document.hidden && stuck && el.value) el.value.scrollTop = el.value.scrollHeight
}
onMounted(() => {
  if (content.value) grown.observe(content.value)
  // The log itself shrinking (composer growing, reply bar, banner) hides the bottom too.
  if (el.value) grown.observe(el.value)
  document.addEventListener('visibilitychange', shown)
})
onUnmounted(() => {
  grown.disconnect()
  document.removeEventListener('visibilitychange', shown)
})
// Messages from the same person this close together read as one.
const GROUP_MS = 5 * 60_000

const byId = computed(() => {
  const m = new Map<string, Msg>()
  for (const x of client.messages) if (x.msgid) m.set(x.msgid, x)
  return m
})

function continued(i: number): boolean {
  const [a, b] = [client.messages[i - 1], client.messages[i]]
  return (
    !!a &&
    a.nick === b.nick &&
    !b.replyTo &&
    a.notice === b.notice &&
    b.time - a.time < GROUP_MS &&
    sameDay(a.time, b.time)
  )
}

const typing = computed(() => {
  const t = client.buffer?.typing ?? []
  if (t.length === 0) return ''
  if (t.length === 1) return `${t[0]} is typing…`
  if (t.length <= 3) return `${t.slice(0, -1).join(', ')} and ${t.at(-1)} are typing…`
  return 'Several people are typing…'
})

// Scrollback: one page per change in what's loaded, so an empty answer can't loop.
let askedAt = -1
function maybeOlder() {
  const e = el.value
  if (!e || client.buffer?.historyStart || !client.connected) return
  if (e.scrollTop < 300 && askedAt !== client.messages.length) {
    askedAt = client.messages.length
    client.loadOlder()
  }
}

// Where to jump once `loadAround` brings it in.
let jumping: string | undefined
function jump(msgid: string) {
  const row = el.value?.querySelector<HTMLElement>(`[data-msgid="${CSS.escape(msgid)}"]`)
  if (row) {
    jumping = undefined
    stuck = false
    row.scrollIntoView({ block: 'center', behavior: 'smooth' })
    row.classList.add('flash')
    setTimeout(() => row.classList.remove('flash'), 1500)
  } else {
    jumping = msgid
    client.loadAround(msgid)
  }
}

// Keep the view where the reader left it as messages come and go.
let before = { bottom: true, height: 0, top: 0, first: '' }
watch(
  () => client.messages,
  () => {
    const e = el.value
    if (!e) return
    before = {
      // The reader's intent, not the layout: in a background tab the
      // ResizeObserver doesn't run, so grown content leaves us short of the bottom.
      bottom: stuck,
      height: e.scrollHeight,
      top: e.scrollTop,
      first: before.first,
    }
  },
  { flush: 'pre' },
)
watch(
  () => client.messages,
  async (msgs) => {
    await nextTick()
    const e = el.value
    if (!e) return
    const first = msgs[0]?.key ?? ''
    if (before.bottom) e.scrollTop = e.scrollHeight
    else if (first !== before.first) e.scrollTop = e.scrollHeight - before.height + before.top
    before.first = first
    if (jumping && byId.value.has(jumping)) jump(jumping)
    maybeOlder()
  },
)
watch(
  () => client.selected,
  async () => {
    askedAt = -1
    jumping = undefined
    before = { bottom: true, height: 0, top: 0, first: '' }
    stuck = true
    await nextTick()
    if (el.value) el.value.scrollTop = el.value.scrollHeight
    maybeOlder()
  },
)
</script>

<template>
  <div ref="el" class="log" role="log" aria-live="polite" @scroll.passive="onScroll"
    @wheel.passive="touched"
    @touchmove.passive="touched"
    @keydown="touched"
    @pointerdown="grab"
  >
    <div ref="content">
    <p v-if="client.buffer?.historyStart" class="start dim">This is the start of {{ client.selected }}.</p>
    <template v-for="(m, i) in client.messages" :key="m.key">
      <div v-if="i === 0 || !sameDay(client.messages[i - 1].time, m.time)" class="day dim small">
        <span>{{ day(m.time) }}</span>
      </div>
      <MessageItem
        :msg="m"
        :continued="continued(i)"
        :parent="m.replyTo ? byId.get(m.replyTo) : undefined"
        :seen="!!m.msgid && client.buffer?.seen === m.msgid"
        @reply="client.replying = m"
        @edit="client.editing = m"
        @jump="jump"
      />
    </template>
    </div>
  </div>
  <div class="typing dim small">{{ typing }}</div>
</template>

<style scoped>
.log {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  padding-bottom: 8px;
}
.start {
  padding: 16px;
}
.day {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 16px 16px 0;
}
.day::before,
.day::after {
  content: '';
  flex: 1;
  border-top: 1px solid var(--border);
}
.typing {
  height: 1.3em;
  padding: 0 16px;
}
:deep(.flash) {
  animation: flash 1.5s;
}
@keyframes flash {
  from {
    background: #89b4fa44;
  }
}
</style>
