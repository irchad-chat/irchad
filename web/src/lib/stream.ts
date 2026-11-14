// Public streams (D87): a share in a LiveKit room of its own, `stream-<code>`.
const NETWORK: string = import.meta.env.VITE_IRCHAD_NETWORK ?? location.origin

/** The call-audio track's name in a stream's room; call members skip it. */
export const CALL_AUDIO = 'call-audio'

/** The code in a stream room's name, or null: only what chadd would make (D14). */
export function codeOfRoom(room: string | undefined): string | null {
  const m = room?.match(/^stream-([0-9a-v]{36})$/)
  return m ? m[1] : null
}

/** chadd's watch-only token for a stream's room. */
export async function streamToken(code: string): Promise<{ livekit: string; token: string }> {
  const res = await fetch(new URL('/api/stream/token', NETWORK), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ code }),
  })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(typeof body.error === 'string' ? body.error : `HTTP ${res.status}`)
  if (typeof body.livekit !== 'string' || typeof body.token !== 'string') throw new Error('The server sent something odd.')
  return body
}
