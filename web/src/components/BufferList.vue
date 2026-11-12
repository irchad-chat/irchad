<template>
  <nav class="side">
    <header v-if="!!client.state">
      <b>{{ client.state.name }}</b>
      <span class="dim"
        >{{ client.state.nick }} · {{ client.state.state }}</span
      >
      <span
        v-if="client.error && client.state.state !== 'connected'"
        class="err"
        >{{ client.error }}</span
      >
    </header>
    <button
      v-for="b in client.buffers"
      :key="b.target"
      class="buf"
      :class="{ on: b.target === client.selected, unread: b.unread }"
      :title="b.topic"
      @click="client.select(b.target)"
    >
      <span>{{ b.kind === "dm" ? "@" : "" }}{{ b.target }}</span>
      <span v-if="b.mentions" class="badge">{{ b.mentions }}</span>
    </button>
    <form @submit.prevent="open">
      <input
        v-model="opening"
        aria-label="Join a channel or message someone"
        placeholder="#channel or nick"
      />
    </form>
    <p v-if="client.error" class="err">{{ client.error }}</p>
  </nav>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { useClient } from "../stores/client";

const client = useClient();

const opening = ref("");
function open() {
  if (!opening.value.trim()) return;
  client.open(opening.value);
  opening.value = "";
}
</script>
