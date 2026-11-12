<!-- A message's text: formatting, links, @-mentions and custom emoji. -->
<script setup lang="ts">
import type { Span } from '../wasm/chad_web'
import { spanStyle } from '../lib/format'
import MediaImg from './MediaImg.vue'

defineProps<{ spans: Span[] }>()
</script>

<template>
  <template v-for="(s, j) in spans" :key="j">
    <!-- chad-core only flags http(s) links, so the href is safe to use as-is. -->
    <a v-if="s.link" :href="s.text" target="_blank" rel="noopener noreferrer" :style="spanStyle(s)">{{
      s.text
    }}</a>
    <span v-else-if="s.mention" class="mention" :style="spanStyle(s)">@{{ s.mention }}</span>
    <MediaImg v-else-if="s.emoji" class="emoji" :url="s.emoji" :alt="s.text" :title="s.text">{{
      s.text
    }}</MediaImg>
    <code v-else-if="s.mono" :style="spanStyle(s)">{{ s.text }}</code>
    <span v-else :style="spanStyle(s)">{{ s.text }}</span>
  </template>
</template>
