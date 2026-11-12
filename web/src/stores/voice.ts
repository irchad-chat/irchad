// The call itself, with livekit-client. chad-core decides who may join and
// tells IRC we're in (`chad/voice`); this runs the LiveKit room. It follows
// desktop's rules (chad-media's room.rs): auto-subscribe off, microphones and
// the soundboard always, a screen share and its audio or a camera only while
// it's watched, and the `watching` attribute for the sharer's count (D22).
import { defineStore } from 'pinia'
import { computed, ref, shallowRef, watch } from 'vue'
import {
  type Participant,
  type RemoteTrack,
  type RemoteTrackPublication,
  type VideoTrack,
  RemoteAudioTrack,
  Room,
  RoomEvent,
  Track,
} from 'livekit-client'
import { message, useClient } from './client'

// D14: the SFU decides who's in the room, so cap what we hold of it.
const MAX_PARTICIPANTS = 500
const MAX_IDENTITY = 64
const WATCHING = 'watching'
// Track names of the extra `Unknown`-source audio tracks (D71).
const EXTRA_AUDIO = ['soundboard', 'whisper']

export type Person = {
  identity: string
  /** Display name, from the call channel's member list. */
  shown: string
  local: boolean
  speaking: boolean
  muted: boolean
  screen: boolean
  camera: boolean
  /** Who watches their screen share, us included. */
  watchers: number
}
export type Watched = { identity: string; camera: boolean }

const short = (s: string) => [...s].slice(0, MAX_IDENTITY).join('')

export const useVoice = defineStore('voice', () => {
  const client = useClient()
  // Not reactive: the snapshot below is.
  let room: Room | null = null
  // Bumped per join/leave, so a stale connect or disconnect does nothing.
  let gen = 0
  const people = shallowRef<Person[]>([])
  const muted = ref(false)
  const deafened = ref(false)
  const watching = ref<Watched | null>(null)
  // The video on the stage: the watched share or camera, once it arrives.
  const stage = shallowRef<VideoTrack | null>(null)
  // The browser blocked playback until a click.
  const blocked = ref(false)
  const call = computed(() => client.state?.call ?? null)
  const audio = document.createElement('div')
  audio.hidden = true
  document.body.append(audio)

  function shown(identity: string): string {
    // shortcut: ASCII-only account match, as desktop's nick_of; else the account itself.
    const m = client.callMembers?.find((m) => m.account?.toLowerCase() === identity.toLowerCase())
    return m?.shown ?? identity
  }

  function wanted(pub: RemoteTrackPublication, identity: string): boolean {
    const w = watching.value
    switch (pub.source) {
      case Track.Source.Microphone:
        return true
      case Track.Source.Unknown:
        return pub.kind === Track.Kind.Audio && EXTRA_AUDIO.includes(pub.trackName)
      case Track.Source.ScreenShare:
      case Track.Source.ScreenShareAudio:
        return w?.identity === identity && !w.camera
      case Track.Source.Camera:
        return w?.identity === identity && w.camera
    }
    return false
  }

  function subscribe(pub: RemoteTrackPublication, p: Participant) {
    const want = wanted(pub, short(p.identity))
    if (want !== pub.isSubscribed) pub.setSubscribed(want)
  }

  function resubscribe() {
    for (const p of room?.remoteParticipants.values() ?? [])
      for (const pub of p.trackPublications.values()) subscribe(pub as RemoteTrackPublication, p)
  }

  function onSubscribed(track: RemoteTrack) {
    if (track.kind === Track.Kind.Audio) {
      audio.append(track.attach())
      if (deafened.value && track instanceof RemoteAudioTrack) track.setVolume(0)
    }
    sync()
  }

  function onUnsubscribed(track: RemoteTrack) {
    track.detach().forEach((e) => e.remove())
    sync()
  }

  // The snapshot the UI reads; called after anything in the room changed.
  function sync() {
    if (!room) {
      people.value = []
      stage.value = null
      return
    }
    const remotes = [...room.remoteParticipants.values()].slice(0, MAX_PARTICIPANTS)
    const local = room.localParticipant
    const localId = short(local.identity)
    const counts = new Map<string, number>()
    for (const p of remotes) {
      const w = p.attributes[WATCHING]
      if (w) counts.set(short(w), (counts.get(short(w)) ?? 0) + 1)
    }
    const w = watching.value
    if (w && !w.camera && w.identity !== localId) counts.set(w.identity, (counts.get(w.identity) ?? 0) + 1)
    const person = (p: Participant, isLocal: boolean): Person => {
      const identity = short(p.identity)
      return {
        identity,
        shown: shown(identity),
        local: isLocal,
        speaking: p.isSpeaking,
        muted: isLocal ? muted.value : !p.isMicrophoneEnabled,
        screen: !!p.getTrackPublication(Track.Source.ScreenShare),
        camera: !!p.getTrackPublication(Track.Source.Camera),
        watchers: counts.get(identity) ?? 0,
      }
    }
    people.value = [person(local, true), ...remotes.map((p) => person(p, false))]

    // The watched one left or stopped sharing: stop watching.
    const target = w && remotes.find((p) => short(p.identity) === w.identity)
    const source = w?.camera ? Track.Source.Camera : Track.Source.ScreenShare
    const pub = target?.getTrackPublication(source)
    if (w && !pub) {
      watch_(null)
      return
    }
    stage.value = (pub?.videoTrack as VideoTrack | undefined) ?? null
  }

  async function join(channel: string) {
    if (room && call.value?.channel === channel) return
    disconnect()
    const g = ++gen
    let join
    try {
      join = await client.joinVoice(channel)
    } catch (err) {
      client.notify(message(err))
      return
    }
    if (!join || g !== gen) return
    const r = new Room({ adaptiveStream: true, dynacast: true })
    room = r
    r.on(RoomEvent.TrackPublished, subscribe)
      .on(RoomEvent.TrackSubscribed, onSubscribed)
      .on(RoomEvent.TrackUnsubscribed, onUnsubscribed)
      .on(RoomEvent.ParticipantConnected, sync)
      .on(RoomEvent.ParticipantDisconnected, sync)
      .on(RoomEvent.TrackUnpublished, sync)
      .on(RoomEvent.TrackMuted, sync)
      .on(RoomEvent.TrackUnmuted, sync)
      .on(RoomEvent.LocalTrackPublished, sync)
      .on(RoomEvent.ActiveSpeakersChanged, sync)
      .on(RoomEvent.ParticipantAttributesChanged, sync)
      .on(RoomEvent.AudioPlaybackStatusChanged, () => (blocked.value = !r.canPlaybackAudio))
      .on(RoomEvent.Disconnected, () => {
        // Not us hanging up: it dropped.
        if (room === r) {
          disconnect()
          client.leaveVoice('Lost the voice connection.')
        }
      })
    try {
      await r.connect(join.url, join.token, { autoSubscribe: false })
    } catch (err) {
      if (room === r) {
        disconnect()
        client.leaveVoice(`Couldn't join voice: ${message(err)}`)
      }
      return
    }
    if (room !== r) return
    client.voiceConnected()
    blocked.value = !r.canPlaybackAudio
    resubscribe()
    sync()
    await mic(!muted.value)
  }

  // Drops the room without telling chad-core.
  function disconnect() {
    gen++
    const r = room
    room = null
    watching.value = null
    blocked.value = false
    audio.replaceChildren()
    r?.disconnect()
    sync()
  }

  function leave() {
    if (!call.value && !room) return
    disconnect()
    client.leaveVoice(null)
  }

  async function mic(on: boolean) {
    try {
      await room?.localParticipant.setMicrophoneEnabled(on)
    } catch (err) {
      muted.value = true
      client.notify(`No microphone: ${message(err)}`)
    }
    sync()
  }

  function setMuted(m: boolean) {
    muted.value = m
    if (!m && deafened.value) {
      deafened.value = false
      applyDeafen()
    }
    void mic(!m)
  }

  // Deafened hears nothing and says nothing; undeafening puts the mute back as it was.
  let mutedBefore = false
  function setDeafened(d: boolean) {
    if (d === deafened.value) return
    if (d) mutedBefore = muted.value
    deafened.value = d
    applyDeafen()
    muted.value = d || mutedBefore
    void mic(!muted.value)
  }

  function applyDeafen() {
    for (const p of room?.remoteParticipants.values() ?? [])
      for (const pub of p.audioTrackPublications.values())
        if (pub.audioTrack instanceof RemoteAudioTrack) pub.audioTrack.setVolume(deafened.value ? 0 : 1)
  }

  // `watch_`: `watch` is Vue's.
  function watch_(target: Watched | null) {
    if (target?.identity === watching.value?.identity && target?.camera === watching.value?.camera) return
    watching.value = target
    stage.value = null
    if (!room) return
    resubscribe()
    // Tells the sharer how many watch (D22); cameras don't count.
    const id = target && !target.camera ? target.identity : ''
    room.localParticipant.setAttributes({ [WATCHING]: id }).catch(() => {})
    sync()
  }

  const startAudio = () => room?.startAudio().then(() => (blocked.value = false))

  // The call ended in chad-core (IRC dropped, voice turned off, joined elsewhere).
  watch(call, (c) => {
    if (!c && room) disconnect()
  })
  // Display names arrive with the member list.
  watch(() => client.callMembers, sync)
  // Best effort: clear `chad/voice` now rather than on the next connect.
  window.addEventListener('pagehide', leave)

  return {
    call,
    people,
    muted,
    deafened,
    watching,
    stage,
    blocked,
    join,
    leave,
    setMuted,
    setDeafened,
    watch: watch_,
    startAudio,
  }
})
