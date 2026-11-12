<!-- A guest makes an account and carries on as it, keeping their channels. -->
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useClient } from '../stores/client'

const emit = defineEmits<{ close: [] }>()
const client = useClient()
const account = ref(client.state?.nick ?? '')
const password = ref('')
const email = ref('')
const dialog = ref<HTMLDialogElement>()

onMounted(() => dialog.value?.showModal())

function submit() {
  if (client.signUp(account.value, password.value, email.value)) emit('close')
}
</script>

<template>
  <dialog ref="dialog" @close="emit('close')">
    <form class="stack" @submit.prevent="submit">
      <h2>Make an account</h2>
      <p class="dim small">You'll keep your channels and messages.</p>
      <input v-model="account" aria-label="Name" placeholder="Name" autocomplete="username" required />
      <input
        v-model="password"
        type="password"
        aria-label="Password"
        placeholder="Password"
        autocomplete="new-password"
        required
      />
      <input v-model="email" type="email" aria-label="Email" placeholder="Email (if the server asks for one)" autocomplete="email" />
      <p v-if="client.error" class="err">{{ client.error }}</p>
      <div class="row">
        <button type="button" @click="emit('close')">Cancel</button>
        <button class="primary">Sign up</button>
      </div>
    </form>
  </dialog>
</template>
