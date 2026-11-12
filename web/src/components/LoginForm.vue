<!-- Logging in, signing up, joining as a guest, and the emailed code. -->
<script setup lang="ts">
import { computed, ref } from 'vue'
import { useClient } from '../stores/client'

const client = useClient()
const mode = ref<'login' | 'signup' | 'guest'>('login')
const name = ref('')
const password = ref('')
const email = ref('')
const code = ref('')
const host = new URL(client.network).host

const busy = computed(() => client.state?.state === 'connecting')
const verifying = computed(() => client.state?.state === 'verifying')

function submit() {
  if (mode.value === 'signup') client.signUp(name.value, password.value, email.value)
  else client.connect(name.value, mode.value === 'guest' ? '' : password.value)
}
</script>

<template>
  <main class="login">
    <h1>{{ client.state?.name || host }}</h1>
    <form v-if="verifying" class="stack" @submit.prevent="client.verify(code)">
      <p>{{ client.state?.error || 'Enter the code from your email.' }}</p>
      <input v-model="code" aria-label="Code" placeholder="Code" autocomplete="one-time-code" required />
      <button class="primary">Verify</button>
    </form>
    <form v-else class="stack" @submit.prevent="submit">
      <div class="tabs" role="tablist">
        <button
          v-for="[m, label] in [['login', 'Log in'], ['signup', 'Sign up'], ['guest', 'Guest']] as const"
          :key="m"
          type="button"
          role="tab"
          :aria-selected="mode === m"
          :class="{ on: mode === m }"
          @click="mode = m"
        >
          {{ label }}
        </button>
      </div>
      <input v-model="name" aria-label="Name" placeholder="Name" autocomplete="username" required />
      <input
        v-if="mode !== 'guest'"
        v-model="password"
        type="password"
        aria-label="Password"
        placeholder="Password"
        :autocomplete="mode === 'signup' ? 'new-password' : 'current-password'"
        required
      />
      <input
        v-if="mode === 'signup'"
        v-model="email"
        type="email"
        aria-label="Email"
        placeholder="Email (if the server asks for one)"
        autocomplete="email"
      />
      <button class="primary" :disabled="busy">
        {{ busy ? 'Connecting…' : mode === 'signup' ? 'Sign up' : mode === 'guest' ? 'Join as a guest' : 'Log in' }}
      </button>
      <p v-if="client.state?.error && client.state.state !== 'connecting'" class="err">{{ client.state.error }}</p>
      <p v-if="client.error" class="err">{{ client.error }}</p>
    </form>
  </main>
</template>

<style scoped>
.login {
  margin: auto;
  width: min(340px, 100% - 32px);
}
h1 {
  text-align: center;
}
.tabs {
  display: flex;
  gap: 4px;
}
.tabs button {
  flex: 1;
  background: none;
}
.tabs button.on {
  background: var(--surface);
  border-color: var(--accent);
}
</style>
