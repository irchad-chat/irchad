<script setup lang="ts">
import { ref } from 'vue'
import { useClient } from './stores/client'
import LoginForm from './components/LoginForm.vue'
import BufferList from './components/BufferList.vue'
import ChatHeader from './components/ChatHeader.vue'
import MessageList from './components/MessageList.vue'
import Composer from './components/Composer.vue'
import MemberList from './components/MemberList.vue'
import Toasts from './components/Toasts.vue'
import Stage from './components/Stage.vue'
import AppNotice from './components/AppNotice.vue'

const client = useClient()
const showMembers = ref(true)
</script>

<template>
  <LoginForm v-if="!client.ready || client.state?.state === 'verifying'" />
  <div v-else class="app">
    <BufferList />
    <main class="chat">
      <ChatHeader :members="showMembers" @toggle-members="showMembers = !showMembers" />
      <p v-if="client.state && client.state.state !== 'connected'" class="banner small">
        {{ client.state.error ? `${client.state.error} ` : '' }}Reconnecting…
      </p>
      <Stage />
      <MessageList />
      <Composer />
    </main>
    <MemberList v-if="showMembers && client.buffer?.kind === 'channel'" />
    <AppNotice />
  </div>
  <Toasts />
</template>

<style>
:root {
  --bg: #1e1e2e;
  --bg-deep: #11111b;
  --panel: #181825;
  --surface: #313244;
  --border: #45475a;
  --text: #cdd6f4;
  --accent: #89b4fa;
  --ok: #a6e3a1;
  --warn: #f9e2af;
  --err: #f38ba8;
  /* The sidebar and chat headers, so their borders line up. */
  --header-height: 52px;
  color-scheme: dark;
}
html,
body,
#app {
  margin: 0;
  height: 100%;
}
#app {
  display: flex;
  background: var(--bg);
  color: var(--text);
  font-family: system-ui, sans-serif;
  font-size: 15px;
}
input,
button,
textarea {
  font: inherit;
  color: inherit;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 6px 8px;
}
button {
  cursor: pointer;
}
button:disabled {
  cursor: default;
  opacity: 0.5;
}
button.primary {
  background: var(--accent);
  color: var(--bg);
  border-color: var(--accent);
  font-weight: bold;
}
button.link {
  background: none;
  border: none;
  padding: 0 4px;
}
dialog {
  background: var(--panel);
  color: inherit;
  border: 1px solid var(--border);
  border-radius: 8px;
  width: min(340px, calc(100vw - 32px));
}
dialog::backdrop {
  background: #0008;
}
.stack {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.stack .row {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.stack h2,
.stack p {
  margin: 0;
}
.app {
  flex: 1;
  display: flex;
  min-width: 0;
}
.chat {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
}
.banner {
  margin: 0;
  padding: 4px 16px;
  background: #f9e2af22;
  color: var(--warn);
}
.backdrop {
  position: fixed;
  inset: 0;
  z-index: 10;
}
.emoji {
  display: inline-block;
  width: 1.4em;
  height: 1.4em;
  vertical-align: middle;
  object-fit: contain;
}
.ph {
  vertical-align: -0.125em;
}
.mention {
  background: #89b4fa33;
  color: var(--accent);
  border-radius: 3px;
  padding: 0 2px;
}
code {
  font-family: ui-monospace, monospace;
  background: var(--bg-deep);
  border-radius: 3px;
  padding: 0 3px;
}
.badge {
  margin-left: auto;
  background: var(--err);
  color: var(--bg);
  border-radius: 8px;
  padding: 0 6px;
  font-size: 0.8em;
  font-weight: bold;
}
.ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dim {
  opacity: 0.6;
}
.small {
  font-size: 0.85em;
}
.err {
  color: var(--err);
}
a {
  color: var(--accent);
}
</style>
