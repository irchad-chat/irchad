<!-- Your name and picture as others see them, a Discord account's password for
     other IRC clients, and deleting the account. -->
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { message, useClient } from '../stores/client'
import Avatar from './Avatar.vue'

const emit = defineEmits<{ close: [] }>()
const client = useClient()
const dialog = ref<HTMLDialogElement>()
const picker = ref<HTMLInputElement>()
const name = ref(client.state?.displayName ?? '')
const password = ref('')
const passwordNote = ref<{ ok: boolean; text: string }>()
const deleting = ref(false)
const deleteError = ref<string>()
const busy = ref(false)

onMounted(() => dialog.value?.showModal())

function save() {
  if (name.value.trim() !== (client.state?.displayName ?? '')) client.setDisplayName(name.value)
  emit('close')
}

async function picked() {
  const file = picker.value?.files?.[0]
  if (file) await client.uploadAvatar(file)
}

async function savePassword() {
  busy.value = true
  try {
    await client.setPassword(password.value)
    password.value = ''
    passwordNote.value = { ok: true, text: 'Password set.' }
  } catch (err) {
    passwordNote.value = { ok: false, text: message(err) }
  } finally {
    busy.value = false
  }
}

async function deleteAccount() {
  busy.value = true
  try {
    await client.deleteAccount()
  } catch (err) {
    deleteError.value = message(err)
    busy.value = false
  }
}
</script>

<template>
  <dialog ref="dialog" @close="emit('close')">
    <form v-if="deleting" class="stack" @submit.prevent="deleteAccount">
      <h2>Delete your account?</h2>
      <p class="small">Your account is deleted from the server, with your messages and uploads. The name stays reserved.</p>
      <p v-if="deleteError" class="err small">{{ deleteError }}</p>
      <div class="row">
        <button type="button" :disabled="busy" @click="deleting = false">Cancel</button>
        <button class="danger" :disabled="busy">Delete account</button>
      </div>
    </form>
    <div v-else class="stack">
      <h2>Your profile on {{ client.state?.name }}</h2>
      <div class="picture">
        <Avatar :nick="client.state?.nick ?? ''" :shown="name || client.state?.nick || ''" :url="client.state?.avatar ?? null" :size="64" />
        <div class="stack">
          <template v-if="client.state?.uploads">
            <button type="button" :disabled="!client.connected" @click="picker?.click()">Choose a picture…</button>
            <input ref="picker" type="file" accept="image/png,image/jpeg,image/gif,image/webp" hidden @change="picked" />
          </template>
          <button v-if="client.state?.avatar" type="button" :disabled="!client.connected" @click="client.removeAvatar()">
            Remove picture
          </button>
        </div>
      </div>
      <form class="stack" @submit.prevent="save">
        <label class="stack">
          Display name
          <input v-model="name" :placeholder="client.state?.nick" maxlength="64" />
        </label>
        <p class="dim small">People still see that you're {{ client.state?.nick }}.</p>
        <div class="row">
          <button type="button" @click="emit('close')">Cancel</button>
          <button class="primary" :disabled="!client.connected">Save</button>
        </div>
      </form>
      <template v-if="!client.state?.guest">
        <hr />
        <form v-if="client.state?.discord" class="stack" @submit.prevent="savePassword">
          <h3>Password for other IRC clients</h3>
          <p class="dim small">You sign in here with Discord. Other IRC clients log in with your username and this password.</p>
          <input
            v-model="password"
            type="password"
            aria-label="Password"
            placeholder="Password"
            autocomplete="new-password"
            minlength="8"
            required
          />
          <p v-if="passwordNote" class="small" :class="passwordNote.ok ? 'dim' : 'err'">{{ passwordNote.text }}</p>
          <div class="row">
            <button :disabled="busy || !client.connected">Set password</button>
          </div>
        </form>
        <div class="row">
          <button type="button" class="danger" :disabled="!client.connected" @click="deleting = true">Delete account…</button>
        </div>
      </template>
    </div>
  </dialog>
</template>

<style scoped>
h3 {
  margin: 0;
  font-size: 1em;
}
.picture {
  display: flex;
  align-items: center;
  gap: 16px;
}
hr {
  width: 100%;
  border: none;
  border-top: 1px solid var(--border);
}
.danger {
  color: var(--err);
}
</style>
