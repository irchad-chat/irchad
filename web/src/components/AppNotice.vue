<!-- The desktop app pitch, once per browser. The sidebar's link stays for good. -->
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useClient } from '../stores/client'

const client = useClient()
const dialog = ref<HTMLDialogElement>()
const KEY = 'irchad.appNotice'

onMounted(() => {
  let seen = false
  try {
    seen = localStorage.getItem(KEY) !== null
    localStorage.setItem(KEY, '1')
  } catch {
    // No storage (private window): show it every load rather than never.
  }
  if (!seen) dialog.value?.showModal()
})
</script>

<template>
  <dialog ref="dialog" class="notice">
    <form class="stack" method="dialog">
      <i class="ph ph-desktop big" />
      <h2>Get the IrChad app</h2>
      <p>You're on the web version. The app does a lot more:</p>
      <ul>
        <li>Encrypted DMs</li>
        <li>Push-to-talk, the soundboard and per-person volume</li>
        <li>Staying logged in</li>
      </ul>
      <p class="dim small">For Windows, Linux and Android.</p>
      <div class="row">
        <button>Keep using the web</button>
        <a class="button primary" :href="`${client.network}/invite`" target="_blank" rel="noopener">
          <i class="ph ph-download-simple" /> Get the app
        </a>
      </div>
    </form>
  </dialog>
</template>

<style scoped>
.notice {
  width: min(420px, calc(100vw - 32px));
  padding: 24px;
}
.big {
  font-size: 48px;
  color: var(--accent);
  align-self: center;
}
h2 {
  text-align: center;
}
ul {
  margin: 0;
  padding-left: 20px;
  line-height: 1.6;
}
.button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border-radius: 6px;
  text-decoration: none;
  background: var(--accent);
  color: var(--bg);
  font-weight: bold;
}
</style>
