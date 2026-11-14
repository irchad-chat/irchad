import { createApp } from 'vue'
import { createPinia } from 'pinia'
// Phosphor's regular icons, the font the desktop bundles.
import '@phosphor-icons/web/regular/style.css'
import init from './wasm/chad_web'
import App from './App.vue'
import WatchView from './components/WatchView.vue'

// Load the Rust core before anything can call into it.
await init()
// A public stream's link (D87) only watches: no IRC client behind it.
const root = new URLSearchParams(location.search).has('watch') ? WatchView : App
createApp(root).use(createPinia()).mount('#app')
