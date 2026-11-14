// Public streams (D87): a share in a LiveKit room of its own, `stream-<code>`.
const NETWORK: string = import.meta.env.VITE_IRCHAD_NETWORK ?? location.origin

/** The call-audio track's name in a stream's room; call members skip it. */
export const CALL_AUDIO = 'call-audio'

/** The code in a stream room's name, or null: only what chadd would make (D14). */
export function codeOfRoom(room: string | undefined): string | null {
  const m = room?.match(/^stream-([0-9a-v]{36})$/)
  return m ? m[1] : null
}

/** The network's origin, whose `/invite` page has the downloads. */
export const NETWORK_ORIGIN = new URL(NETWORK).origin

/** chadd's watch-only token for a stream's room, and the network's name. */
export async function streamToken(code: string): Promise<{ livekit: string; token: string; network: string }> {
  const res = await fetch(new URL('/api/stream/token', NETWORK), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ code }),
  })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(typeof body.error === 'string' ? body.error : `HTTP ${res.status}`)
  if (typeof body.livekit !== 'string' || typeof body.token !== 'string' || typeof body.network !== 'string')
    throw new Error('The server sent something odd.')
  // D14: shown as a heading, so one line and capped.
  return { livekit: body.livekit, token: body.token, network: oneLine(body.network) }
}

/** Server text fit for a heading: no control chars, at most 64 chars. */
export function oneLine(s: string): string {
  return [...s.replace(/\p{Cc}/gu, '')].slice(0, 64).join('').trim()
}
