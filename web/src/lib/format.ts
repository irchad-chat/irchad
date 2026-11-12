import type { Rgb, Span } from '../wasm/chad_web'

// The same nick gets the same color everywhere, readable on the dark theme.
export function nickColor(nick: string): string {
  let h = 0
  for (const c of nick.toLowerCase()) h = (h * 31 + c.codePointAt(0)!) >>> 0
  return `hsl(${h % 360} 65% 72%)`
}

const rgb = (c: Rgb | null) => (c ? `rgb(${c.join(',')})` : undefined)

export function spanStyle(s: Span) {
  return {
    fontWeight: s.bold ? 'bold' : undefined,
    fontStyle: s.italic ? 'italic' : undefined,
    textDecoration:
      [s.underline && 'underline', s.strike && 'line-through'].filter(Boolean).join(' ') || undefined,
    color: rgb(s.fg),
    background: rgb(s.bg),
  }
}

export const time = (ms: number) =>
  new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })

export const day = (ms: number) =>
  new Date(ms).toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' })

export const sameDay = (a: number, b: number) => new Date(a).toDateString() === new Date(b).toDateString()

// What the @ list inserts for a nick (chad-core's `complete::insert`).
export const insertNick = (nick: string, atStart: boolean) => (atStart ? `${nick}: ` : `${nick} `)
