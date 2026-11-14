<!-- Our call, above who we are: where, and mute, deafen, hang up. -->
<script setup lang="ts">
import { useVoice } from '../stores/voice'
import { useClient } from '../stores/client'

const voice = useVoice()
const client = useClient()
</script>

<template>
  <section v-if="voice.call" class="voice-bar" aria-label="Voice call">
    <div class="where">
      <b :class="voice.call.connected ? 'ok' : 'warn'" class="small">
        {{ voice.call.connected ? 'Voice connected' : 'Joining voice…' }}
      </b>
      <button class="link ellipsis dim small" @click="client.select(voice.call.channel)">{{ voice.call.channel }}</button>
    </div>
    <button v-if="voice.blocked" class="primary small" @click="voice.startAudio()">Turn on sound</button>
    <p v-if="voice.publicAudio" class="err small public">
      <i class="ph ph-broadcast" />
      {{ voice.publicAudio.local ? "You're" : `${voice.publicAudio.shown} is` }} streaming this call's audio publicly
    </p>
    <button
      :class="{ off: voice.muted }"
      :aria-pressed="voice.muted"
      :title="voice.muted ? 'Unmute' : 'Mute'"
      :aria-label="voice.muted ? 'Unmute' : 'Mute'"
      @click="voice.setMuted(!voice.muted)"
    >
      <i class="ph" :class="voice.muted ? 'ph-microphone-slash' : 'ph-microphone'" />
    </button>
    <button
      :class="{ off: voice.deafened }"
      :aria-pressed="voice.deafened"
      :title="voice.deafened ? 'Undeafen' : 'Deafen'"
      :aria-label="voice.deafened ? 'Undeafen' : 'Deafen'"
      @click="voice.setDeafened(!voice.deafened)"
    >
      <i class="ph" :class="voice.deafened ? 'ph-speaker-slash' : 'ph-headphones'" />
    </button>
    <button title="Leave voice" aria-label="Leave voice" @click="voice.leave()">
      <i class="ph ph-phone-disconnect" />
    </button>
  </section>
</template>

<style scoped>
.public {
  order: 10;
  flex-basis: 100%;
  margin: 0;
}
.voice-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px;
  padding: 8px 12px;
  background: var(--bg-deep);
  border-bottom: 1px solid var(--border);
}
.where {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
}
.where .link {
  padding: 0;
  max-width: 100%;
}
.ok {
  color: var(--ok);
}
.warn {
  color: var(--warn);
}
button:not(.link, .primary) {
  background: none;
  border: none;
  padding: 4px 6px;
}
button.off {
  color: var(--err);
}
</style>
