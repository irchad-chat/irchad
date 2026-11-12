// The only place that talks to Rust. Components read state and call actions.
// Rust (chad-core) owns the IRC and the state; this mirrors its snapshots.
import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import { Client, type Buffer, type Msg, type NetState } from '../wasm/chad_web'

// Production serves the page from the network's own origin; dev sets it in .env.development.
const NETWORK: string = import.meta.env.VITE_IRCHAD_NETWORK ?? location.origin

export const useClient = defineStore('client', () => {
  const state = shallowRef<NetState>()
  const buffers = shallowRef<Buffer[]>([])
  const messages = shallowRef<Msg[]>([])
  const selected = ref<string>()
  const error = ref<string>()
  const client = new Client(NETWORK, refresh)
  // Guards markRead against looping if the server never confirms it.
  let marked = ''
  // Opened but not there yet (a JOIN waits for the server): select it when it is.
  let want: string | undefined

  function refresh() {
    state.value = client.state()
    buffers.value = client.buffers()
    // shortcut: ASCII folding, not the server's CASEMAPPING; only picks which buffer to show.
    const wanted = want && buffers.value.find((b) => b.target.toLowerCase() === want!.toLowerCase())
    if (wanted) {
      selected.value = wanted.target
      want = undefined
    } else if (!buffers.value.some((b) => b.target === selected.value)) {
      selected.value = (buffers.value.find((b) => b.kind === 'channel') ?? buffers.value[0])?.target
    }
    messages.value = selected.value ? client.messages(selected.value) : []
    const b = buffers.value.find((b) => b.target === selected.value)
    const key = `${b?.target}:${b?.unread}`
    if (b?.unread && document.hasFocus() && key !== marked) {
      marked = key
      client.markRead(b.target)
    }
  }

  // Each wraps a call that throws on bad input; the message shows in the UI.
  function attempt(f: () => void) {
    try {
      f()
      error.value = undefined
    } catch (err) {
      error.value = String(err instanceof Error ? err.message : err)
    }
  }

  const connect = (nick: string, password: string) => attempt(() => client.connect(nick, password))
  const open = (name: string) =>
    attempt(() => {
      want = name.trim()
      client.open(want)
      refresh()
    })
  const send = (text: string) => selected.value && client.send(selected.value, text)
  function select(target: string) {
    selected.value = target
    refresh()
  }

  window.addEventListener('focus', refresh)

  return { state, buffers, messages, selected, error, connect, open, send, select }
})
