<!-- First visit: the core's cards, one at a time, then `done` (the sign-in form). Skipping is `done` too. -->
<script setup lang="ts">
import { computed, ref } from 'vue'
import { welcomeCards } from '../wasm/chad_web'

const emit = defineEmits<{ done: [] }>()
const cards = welcomeCards()
const at = ref(0)
const last = computed(() => at.value >= cards.length - 1)
const next = () => (last.value ? emit('done') : at.value++)
</script>

<template>
  <section class="welcome stack">
    <h1>{{ cards[at]?.title }}</h1>
    <p class="dim">{{ cards[at]?.body }}</p>
    <p class="dots dim" role="img" :aria-label="`Step ${at + 1} of ${cards.length}`">
      <span v-for="(_, i) in cards" :key="i" :class="{ on: i === at }" />
    </p>
    <button class="primary" autofocus @click="next">{{ last ? 'Get started' : 'Next' }}</button>
    <p class="other">
      <button v-if="at > 0" type="button" class="link" @click="at--">Back</button>
      <button v-if="!last" type="button" class="link dim small" @click="emit('done')">Skip</button>
    </p>
  </section>
</template>

<style scoped>
.welcome {
  text-align: center;
}
.welcome > p {
  margin: 0;
}
.dots {
  display: flex;
  justify-content: center;
  gap: 8px;
}
.dots span {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  border: 1px solid currentColor;
}
.dots span.on {
  background: var(--accent);
  border-color: var(--accent);
}
.other {
  min-height: 1.5em;
}
.other .link:not(.dim) {
  color: var(--accent);
}
</style>
