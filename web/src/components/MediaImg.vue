<!-- An image by URL: the network's own directly, any other through chadd (D25).
     Shows its slot until the image is there. -->
<script setup lang="ts">
import { ref, watchEffect } from 'vue'
import { useClient } from '../stores/client'

const props = defineProps<{ url: string; alt?: string }>()
const emit = defineEmits<{ failed: [] }>()
const client = useClient()
const src = ref<string>()

watchEffect(() => {
  const url = props.url
  src.value = undefined
  client.mediaUrl(url).then(
    (s) => url === props.url && (src.value = s),
    () => emit('failed'),
  )
})
</script>

<template>
  <img v-if="src" :src="src" :alt="alt ?? ''" loading="lazy" @error="emit('failed')" />
  <!-- What to show until the image is there, or if it never is. -->
  <slot v-else />
</template>
