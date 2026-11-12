<script setup lang="ts">
import { useClient } from '../stores/client'

const client = useClient()
</script>

<template>
  <div class="toasts" role="status" aria-live="polite">
    <div v-for="n in client.notices" :key="n.id" class="toast">{{ n.text }}</div>
  </div>
</template>

<style scoped>
.toasts {
  position: fixed;
  top: 16px;
  right: 16px;
  z-index: 50;
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-width: min(400px, calc(100vw - 32px));
  pointer-events: none;
}
.toast {
  padding: 8px 12px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-left: 3px solid var(--err);
  border-radius: 6px;
  box-shadow: 0 4px 12px #0008;
}
</style>
