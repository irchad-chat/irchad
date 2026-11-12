<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import type { Rgb, Span } from "./wasm/chad_web";
import { useClient } from "./stores/client";
import BufferList from "./components/BufferList.vue";

const client = useClient();
const nick = ref("webchad");
const password = ref("");
const input = ref("");
const logEl = ref<HTMLElement>();

const rgb = (c: Rgb | null) => (c ? `rgb(${c.join(",")})` : undefined);
function style(s: Span) {
  return {
    fontWeight: s.bold ? "bold" : undefined,
    fontStyle: s.italic ? "italic" : undefined,
    textDecoration:
      [s.underline && "underline", s.strike && "line-through"]
        .filter(Boolean)
        .join(" ") || undefined,
    fontFamily: s.mono ? "monospace" : undefined,
    color: rgb(s.fg),
    background: rgb(s.bg),
  };
}
const time = (ms: number) =>
  new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

function send() {
  if (!input.value.trim()) return;
  client.send(input.value);
  input.value = "";
}

watch(
  () => client.messages,
  async () => {
    await nextTick();
    logEl.value?.scrollTo({ top: logEl.value.scrollHeight });
  },
);
</script>

<template>
  <form
    v-if="!client.state || client.state.state === 'failed'"
    class="login"
    @submit.prevent="client.connect(nick, password)"
  >
    <h1>IrChad</h1>
    <input
      v-model="nick"
      aria-label="Name"
      placeholder="Name"
      autocomplete="username"
    />
    <input
      v-model="password"
      type="password"
      aria-label="Password"
      placeholder="Password (empty for a guest)"
      autocomplete="current-password"
    />
    <button>Connect</button>
    <p v-if="client.state?.error" class="err">{{ client.state.error }}</p>
    <p v-if="client.error" class="err">{{ client.error }}</p>
  </form>

  <div v-else class="app">
    <BufferList />
    <main>
      <div ref="logEl" class="log">
        <div
          v-for="m in client.messages"
          :key="m.key"
          class="msg"
          :class="{ mention: m.mention, pending: m.pending }"
        >
          <span class="dim">{{ time(m.time) }}</span>
          <b v-if="m.action">* {{ m.nick }}</b>
          <b v-else-if="m.notice">-{{ m.nick }}-</b>
          <b v-else>{{ m.nick }}</b>
          <i v-if="m.redacted" class="dim">deleted</i>
          <template v-else>
            <template v-for="(s, j) in m.spans" :key="j">
              <!-- chad-core only flags http(s) links, so the href is safe to use as-is. -->
              <a
                v-if="s.link"
                :href="s.text"
                target="_blank"
                rel="noopener noreferrer"
                :style="style(s)"
                >{{ s.text }}</a
              >
              <span v-else :style="style(s)">{{ s.text }}</span>
            </template>
            <span v-if="m.edited" class="dim"> (edited)</span>
          </template>
          <span v-if="m.failed" class="err"> {{ m.failed }}</span>
        </div>
      </div>
      <form class="bar" @submit.prevent="send">
        <input
          v-model="input"
          :aria-label="`Message ${client.selected ?? ''}`"
          :placeholder="client.selected ? `Message ${client.selected}` : ''"
          :disabled="!client.selected || client.state.state !== 'connected'"
        />
      </form>
    </main>
  </div>
</template>

<style>
html,
body,
#app {
  margin: 0;
  height: 100%;
}
#app {
  display: flex;
  background: #1e1e2e;
  color: #cdd6f4;
  font-family: system-ui, sans-serif;
}
input,
button {
  font: inherit;
  color: inherit;
  background: #313244;
  border: 1px solid #45475a;
  border-radius: 6px;
  padding: 6px 8px;
}
.login {
  margin: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: min(320px, 100% - 32px);
}
.app {
  flex: 1;
  display: flex;
  min-width: 0;
}
.side {
  width: 220px;
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px;
  background: #181825;
  overflow: auto;
}
.side header {
  display: flex;
  flex-direction: column;
  margin-bottom: 8px;
}
.buf {
  display: flex;
  justify-content: space-between;
  text-align: left;
  background: none;
  border: none;
  opacity: 0.7;
}
.buf.unread {
  opacity: 1;
  font-weight: bold;
}
.buf.on {
  background: #313244;
  opacity: 1;
}
.badge {
  background: #f38ba8;
  color: #1e1e2e;
  border-radius: 8px;
  padding: 0 6px;
}
main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
}
.log {
  flex: 1;
  overflow: auto;
  padding: 8px;
}
.msg {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  padding: 1px 0;
}
.msg > * + * {
  margin-left: 0.5em;
}
.msg.mention {
  background: #45475a55;
}
.msg.pending {
  opacity: 0.6;
}
.bar {
  display: flex;
  padding: 8px;
}
.bar input {
  flex: 1;
}
.dim {
  opacity: 0.6;
}
.err {
  color: #f38ba8;
}
a {
  color: #89b4fa;
}
</style>
