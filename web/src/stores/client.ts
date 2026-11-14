// The only place that talks to Rust. Components read state and call actions.
// Rust (chad-core) owns the IRC and the state; this mirrors its snapshots.
import { defineStore } from 'pinia'
import { computed, ref, shallowRef, watch } from 'vue'
import {
  Client,
  type Buffer,
  type CustomEmoji,
  type LinkPreview,
  type Member,
  type Msg,
  type NetState,
  isMeeting,
  meetingChannel,
} from '../wasm/chad_web'

// Production serves the page from the network's own origin; dev sets it in .env.development.
const NETWORK: string = import.meta.env.VITE_IRCHAD_NETWORK ?? location.origin
const ORIGIN = new URL(NETWORK).origin
// Proxied images kept as blob: URLs; past this the oldest are let go.
const MAX_MEDIA = 300

export const useClient = defineStore('client', () => {
  const state = shallowRef<NetState>()
  const buffers = shallowRef<Buffer[]>([])
  const messages = shallowRef<Msg[]>([])
  const members = shallowRef<Member[] | null>(null)
  // The voice call's channel's members, to name who's in the call.
  const callMembers = shallowRef<Member[] | null>(null)
  const emoji = shallowRef<CustomEmoji[]>([])
  const selected = ref<string>()
  const error = ref<string>()
  const notices = ref<{ id: number; text: string }[]>([])
  // Invites to answer; one per channel, the newest few.
  const invites = ref<{ from: string; channel: string }[]>([])
  // The composer's mode: replying to or editing a message in the selected buffer.
  const replying = shallowRef<Msg | null>(null)
  const editing = shallowRef<Msg | null>(null)
  const client = new Client(NETWORK, refresh)
  // A meeting link's code (`?meet=`, from chadd's /meet page): this tab is that meeting (D86).
  // In here, not at module level: the wasm isn't loaded until main.ts's `await init()`.
  const meeting: string | null = meetingChannel(new URLSearchParams(location.search).get('meet') ?? '') ?? null
  // Guards markRead against looping if the server never confirms it.
  let marked = ''
  // Opened but not there yet (a JOIN waits for the server): select it when it is.
  let want: string | undefined
  // Channels whose member list was asked for on this connection.
  const asked = new Set<string>()
  let typingTimer: ReturnType<typeof setTimeout> | undefined
  let noticeId = 0

  const buffer = computed(() => buffers.value.find((b) => b.target === selected.value))
  const connected = computed(() => state.value?.state === 'connected')

  function refresh() {
    state.value = client.state()
    if (state.value.state === 'connected') ready.value = true
    if (state.value.state === 'failed') ready.value = false
    buffers.value = client.buffers()
    // shortcut: ASCII folding, not the server's CASEMAPPING; only picks which buffer to show.
    const wanted = want && buffers.value.find((b) => b.target.toLowerCase() === want!.toLowerCase())
    if (wanted) {
      selected.value = wanted.target
      want = undefined
    } else if (!buffers.value.some((b) => b.target === selected.value)) {
      selected.value = (buffers.value.find((b) => b.kind === 'channel') ?? buffers.value[0])?.target
    }
    const sel = selected.value
    messages.value = sel ? client.messages(sel) : []
    emoji.value = client.customEmoji()

    // Member lists go with the connection; ask again once back.
    if (!connected.value) asked.clear()
    const b = buffer.value
    members.value = b?.kind === 'channel' ? membersOf(b.target) : null
    const call = state.value.call?.channel
    callMembers.value = call ? membersOf(call) : null

    const key = `${b?.target}:${b?.unread}`
    if (b?.unread && document.hasFocus() && key !== marked) {
      marked = key
      client.markRead(b.target)
    }

    for (const text of client.takeNotices()) notify(text)
    for (const inv of client.takeInvites())
      invites.value = [...invites.value.filter((i) => i.channel !== inv.channel), inv].slice(-3)

    // Typing indicators expire on their own; look again while any are up.
    clearTimeout(typingTimer)
    if (buffers.value.some((b) => b.typing.length)) typingTimer = setTimeout(refresh, 3000)
  }

  // A channel's members, asking for them once per connection if not there yet.
  function membersOf(channel: string): Member[] | null {
    const m = client.members(channel)
    if (m === null && connected.value && !asked.has(channel)) {
      asked.add(channel)
      client.fetchMembers(channel)
    }
    return m
  }

  function notify(text: string) {
    const id = ++noticeId
    notices.value = [...notices.value.slice(-4), { id, text }]
    setTimeout(() => (notices.value = notices.value.filter((n) => n.id !== id)), 6000)
  }

  // Each wraps a call that throws on bad input; the message shows in the UI.
  function attempt(f: () => void): boolean {
    try {
      f()
      error.value = undefined
      return true
    } catch (err) {
      error.value = message(err)
      return false
    }
  }

  const connect = (nick: string, password: string) => attempt(() => client.connect(nick, password))
  const signUp = (account: string, password: string, email: string) =>
    attempt(() => client.signUp(account, password, email || null))
  const verify = (code: string) => client.verify(code)
  const open = (name: string) =>
    attempt(() => {
      want = name.trim()
      client.open(want)
      refresh()
    })
  watch(selected, () => (replying.value = editing.value = null))
  function select(target: string) {
    selected.value = target
    refresh()
  }
  function close(b: Buffer) {
    if (b.kind === 'channel') client.part(b.target)
    else if (b.kind === 'dm') client.closeDm(b.target)
  }

  const send = (text: string, replyTo: string | null, mentions: string[]) =>
    selected.value && client.send(selected.value, text, replyTo, mentions)
  const edit = (m: Msg, text: string, mentions: string[]) =>
    selected.value && m.msgid && client.edit(selected.value, m.msgid, text, mentions)
  const redact = (m: Msg) => selected.value && m.msgid && client.redact(selected.value, m.msgid)
  function react(m: Msg, emoji: string) {
    if (!selected.value || !m.msgid) return
    const own = m.reactions.some((r) => r.emoji === emoji && r.own)
    if (own) client.unreact(selected.value, m.msgid, emoji)
    else client.react(selected.value, m.msgid, emoji)
  }
  const block = (nick: string, account: string | null) => client.block(nick, account)
  const typing = (active: boolean) => selected.value && client.typing(selected.value, active)
  const loadOlder = () => selected.value && client.loadOlder(selected.value)
  const loadAround = (msgid: string) => selected.value && client.loadAround(selected.value, msgid)
  // Ours to track: chad-core keeps it across reconnects but doesn't report it.
  const away = ref(false)
  function setAway(message: string | null) {
    away.value = message !== null
    client.setAway(message)
  }
  // Has been connected: until then the login screen stays up.
  const ready = ref(false)
  const nicks = (query: string) => (selected.value ? client.nicks(selected.value, query) : [])
  const replaceShortcodes = (text: string, cursor: number) => client.replaceShortcodes(text, cursor)
  const refreshEmoji = () => client.refreshEmoji()
  // Voice: the call runs in stores/voice.ts; chad-core only hears how it went.
  const joinVoice = (channel: string) => client.joinVoice(channel)
  const voiceConnected = () => client.voiceConnected()
  const leaveVoice = (error: string | null) => client.leaveVoice(error)
  // Meetings (D86): chad-core joins the call once the channel is joined.
  function answerInvite(channel: string, join: boolean) {
    invites.value = invites.value.filter((i) => i.channel !== channel)
    if (!join) return
    if (isMeeting(channel)) joinMeeting(channel)
    else open(channel)
  }
  function joinMeeting(channel: string) {
    want = channel
    client.joinMeeting(channel)
  }
  async function createMeeting(): Promise<string | null> {
    try {
      const m = await client.createMeeting()
      want = m.channel
      return m.url
    } catch (err) {
      notify(`Couldn't start a meeting: ${message(err)}`)
      return null
    }
  }

  async function upload(file: File) {
    const target = selected.value
    if (!target) return
    try {
      await client.upload(target, file.name, new Uint8Array(await file.arrayBuffer()))
    } catch (err) {
      notify(`Upload failed: ${message(err)}`)
    }
  }

  const gifs = (query: string, pos?: string) => client.gifs(query, pos ?? null)

  // A URL an <img> can show: the network's own as they are, anything else
  // through chadd so its host never sees the reader (D25).
  const media = new Map<string, Promise<string>>()
  function mediaUrl(url: string): Promise<string> {
    let own = false
    try {
      own = new URL(url).origin === ORIGIN
    } catch {
      return Promise.reject(new Error('not a URL'))
    }
    if (own) return Promise.resolve(url)
    let p = media.get(url)
    if (!p) {
      p = client.media(url).then((bytes) => URL.createObjectURL(new Blob([bytes as BlobPart])))
      p.catch(() => media.delete(url))
      media.set(url, p)
      if (media.size > MAX_MEDIA) {
        const [oldest, gone] = media.entries().next().value!
        media.delete(oldest)
        gone.then(URL.revokeObjectURL, () => {})
      }
    }
    return p
  }

  const previews = new Map<string, Promise<LinkPreview | null>>()
  function preview(url: string): Promise<LinkPreview | null> {
    let p = previews.get(url)
    if (!p) {
      p = client.preview(url).catch(() => null)
      previews.set(url, p)
    }
    return p
  }

  window.addEventListener('focus', refresh)

  return {
    network: ORIGIN,
    meeting,
    state,
    buffers,
    buffer,
    messages,
    members,
    callMembers,
    emoji,
    selected,
    error,
    notices,
    invites,
    answerInvite,
    replying,
    editing,
    connected,
    ready,
    away,
    connect,
    signUp,
    verify,
    open,
    select,
    close,
    send,
    edit,
    redact,
    react,
    block,
    typing,
    loadOlder,
    loadAround,
    setAway,
    nicks,
    replaceShortcodes,
    refreshEmoji,
    joinVoice,
    voiceConnected,
    leaveVoice,
    joinMeeting,
    createMeeting,
    upload,
    gifs,
    mediaUrl,
    preview,
    notify,
  }
})

export function message(err: unknown): string {
  return String(err instanceof Error ? err.message : err)
}
