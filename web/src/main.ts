import { createApp } from 'vue'
import { createPinia } from 'pinia'
import init from './wasm/chad_web'
import App from './App.vue'

// Load the Rust core before anything can call into it.
await init()
createApp(App).use(createPinia()).mount('#app')
