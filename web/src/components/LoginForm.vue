<!-- Logging in, signing up, joining as a guest, the emailed code, and Discord's name prompt. -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useClient } from '../stores/client'
import WelcomeCards from './WelcomeCards.vue'

const client = useClient()
const mode = ref<'login' | 'signup' | 'guest'>('login')
const name = ref('')
const password = ref('')
const email = ref('')
const code = ref('')
const discordName = ref('')
watch(
  () => client.discordSuggestion,
  (s) => (discordName.value = s ?? ''),
  { immediate: true },
)
const host = new URL(client.network).host

const busy = computed(() => client.state?.state === 'connecting')
const verifying = computed(() => client.state?.state === 'verifying')

// First visit: what a network is, before asking to sign in to one. A browser
// that won't keep the flag just sees the cards again.
const stored = (run: (s: Storage) => string | null | void) => {
  try {
    return run(localStorage)
  } catch {
    return null
  }
}
const welcome = ref(stored((s) => s.getItem('welcomed')) === null)
function welcomed() {
  welcome.value = false
  stored((s) => s.setItem('welcomed', '1'))
}

function submit() {
  if (mode.value === 'signup') client.signUp(name.value, password.value, email.value)
  else client.connect(name.value, mode.value === 'guest' ? '' : password.value)
}
</script>

<template>
  <main class="login">
    <WelcomeCards
      v-if="welcome && !verifying && client.discordSuggestion === null && !client.discordBusy"
      @done="welcomed"
    />
    <template v-else>
    <h1>{{ client.state?.name || host }}</h1>
    <form v-if="verifying" class="stack" @submit.prevent="client.verify(code)">
      <p>{{ client.state?.error || 'Enter the code from your email.' }}</p>
      <input v-model="code" aria-label="Code" placeholder="Code" autocomplete="one-time-code" required />
      <button class="primary">Verify</button>
    </form>
    <form v-else-if="client.discordSuggestion !== null" class="stack" @submit.prevent="client.discordName(discordName)">
      <p>Pick a name. It's how people see you here, and it can't be changed later.</p>
      <input v-model="discordName" aria-label="Name" placeholder="Name" autocomplete="username" required />
      <button class="primary" :disabled="client.discordBusy">
        {{ client.discordBusy ? 'Signing up…' : 'Continue' }}
      </button>
      <button type="button" @click="client.discordCancel()">Cancel</button>
      <p v-if="client.error" class="err">{{ client.error }}</p>
    </form>
    <form v-else class="stack" @submit.prevent="submit">
      <input
        v-model="name"
        :aria-label="mode === 'guest' ? 'Nickname' : 'Name'"
        :placeholder="mode === 'guest' ? 'Nickname' : 'Name'"
        autocomplete="username"
        required
      />
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
      <p v-if="mode === 'guest'" class="dim small">
        Guests miss what's said while they're away, and anyone can take the nickname.
      </p>
      <button class="primary" :disabled="busy">
        {{ busy ? 'Connecting…' : mode === 'signup' ? 'Sign up' : mode === 'guest' ? 'Continue as guest' : 'Log in' }}
      </button>
      <button
        v-if="client.server?.discord && mode !== 'guest'"
        type="button"
        class="discord"
        :disabled="busy || client.discordBusy"
        @click="client.discordStart()"
      >
        <i class="ph ph-discord-logo" />
        {{ client.discordBusy ? 'Signing in…' : 'Sign in with Discord' }}
      </button>
      <p v-if="client.state?.error && client.state.state !== 'connecting'" class="err">{{ client.state.error }}</p>
      <p v-if="client.error" class="err">{{ client.error }}</p>
      <!-- The other ways in, quieter than the button; guest quietest. -->
      <p v-if="mode !== 'login'" class="other dim">
        Have an account? <button type="button" class="link" :disabled="busy" @click="mode = 'login'">Log in</button>
      </p>
      <p v-if="mode !== 'signup' && client.server?.signUp" class="other dim">
        No account yet? <button type="button" class="link" :disabled="busy" @click="mode = 'signup'">Sign up</button>
      </p>
      <p v-if="mode !== 'guest' && client.server?.guests" class="other">
        <button type="button" class="link dim small" :disabled="busy" @click="mode = 'guest'">Continue as guest</button>
      </p>
    </form>
    </template>
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
.other {
  text-align: center;
  margin: 0;
}
.other .link {
  color: var(--accent);
}
.other .link.dim {
  color: inherit;
}
.discord {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  background: #5865f2;
  border-color: #5865f2;
  color: #fff;
}
</style>
