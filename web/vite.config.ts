import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Production is served at <network>/chat/ (deploy/native/Caddyfile).
export default defineConfig(({ command }) => ({
  base: command === 'build' ? '/chat/' : '/',
  plugins: [vue()],
  build: {
    // livekit-client alone is ~450 kB; its own chunk stays cached across app releases.
    chunkSizeWarningLimit: 600,
    rolldownOptions: {
      output: { codeSplitting: { groups: [{ name: 'livekit', test: /node_modules[\\/].*livekit-client/ }] } },
    },
  },
}))
