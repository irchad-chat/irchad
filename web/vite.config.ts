import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Production is served at <network>/chat/ (deploy/native/Caddyfile).
export default defineConfig(({ command }) => ({ base: command === 'build' ? '/chat/' : '/', plugins: [vue()] }))
