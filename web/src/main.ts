import { createApp } from 'vue'
import { createPinia } from 'pinia'
// Phosphor's regular icons, the font the desktop bundles.
import '@phosphor-icons/web/regular/style.css'
import init from './wasm/chad_web'
import App from './App.vue'

// Load the Rust core before anything can call into it.
await init()
createApp(App).use(createPinia()).mount('#app')
