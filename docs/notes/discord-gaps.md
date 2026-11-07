# Features Discord won't build that IrChad could

Research from 2026-11-06. These are things Discord can't do because of how it makes money, moderates or is built, not things it just hasn't gotten to yet.
None of them are scheduled. To take one on, move it into `docs/phases/7-later.md` or a phase.

Already shipped, so worth marketing: source-quality streams with no viewer cap, operator-set upload limits, E2EE text DMs, Wayland screen share with audio and PTT, no ID checks or ads, and any IRC client or bot works.

## 1. Whisper and side conversations in voice

On Discord you talk to the whole channel or you leave it. TeamSpeak's whisper is the common comparison.
Hold a key to speak only to chosen people or a saved sub-group.
On LiveKit, use track subscription permissions or a separate audio track sent to a subset of the room. Plain IRC clients don't notice.

## 2. Local full-text search and real export

Discord's search runs on its servers, and getting your data out means requesting the account data package.
Add a local index (SQLite FTS) filled from CHATHISTORY. It works offline, and can export to plain text, JSON or HTML.
Open question: whether to keep decrypted E2EE DMs at rest. Settle it in `e2ee-threat-model.md` first.

## 3. Large encrypted group chats

Discord caps group DMs at 10, and none of its text is end-to-end encrypted.
MLS is already in the parking lot. An interim step is Olm fan-out for small groups, on a private channel.

## 4. Hi-fi "music mode" voice

Discord's audio quality depends on server boost level.
Add a per-user toggle for stereo, high-bitrate Opus with AEC, noise suppression and AGC turned off. The operator sets the cap.
It builds on the music-sharing work.

## 5. Opt-in read receipts

Discord has never shipped read receipts. We already sync read position across devices with `draft/read-marker`.
Add a mutual, opt-in receipt in DMs as a `+chad/read` tag. It's off by default, and only shown if both sides enable it.
**Done** on desktop, Android and iOS (D68).

## 6. Shareable theme files

Discord forbids client mods, so BetterDiscord and Vencord themes break its rules.
Load egui `Visuals` from a TOML theme file that people can share. It's a small change.

## 7. Schedule or send later

It runs client-side and the client must be online when the message is due. It's small and nice to have, but not a headline feature.

## 8. Multi-track call recording with consent

Communities record podcasts with the Craig bot.
LiveKit Egress could record each speaker to their own track on the server, with a mandatory "recording" indicator for everyone in the call.
It needs the most server work and is the most sensitive for privacy, so it comes last. It would also need a chadd endpoint, so check whether it can be IRC-native first.

## Sources

- [Discord: Meet DAVE](https://discord.com/blog/meet-dave-e2ee-for-audio-video) (messages stay unencrypted)
- [EFF on Discord E2EE](https://www.eff.org/deeplinks/2024/09/discords-end-end-encryption-voice-and-video-step-forward-privacy-all)
- [Discord community: Add E2E for text](https://support.discord.com/hc/en-us/community/posts/4405387758615-Add-end-to-end-encryption-E2E-Voice-and-Text)
- [Discord community: Go Live viewer limit](https://support.discord.com/hc/da/community/posts/360056116232-Go-Live-Viewer-Limit)
- [Discord community: Static stream upload rate](https://support.discord.com/hc/ko/community/posts/18289341959447-Please-Allow-Users-to-Set-a-Static-Stream-Upload-Rate)
- [Dexerto: requested features and group DM cap](https://www.dexerto.com/entertainment/discord-rolls-out-huge-batch-of-highly-requested-features-after-asking-users-for-ideas-3410116/)
- [MakeUseOf: features TeamSpeak has that Discord lacks](https://www.makeuseof.com/back-teamspeak-2026-found-features-discord-missing/)
- [GamingOnLinux: Canary Wayland screen share rollback](https://www.gamingonlinux.com/2025/01/discord-canary-on-linux-breaks-wayland-screen-sharing-support-for-now/page=1/)
- [TechCrunch: age verification delay](https://techcrunch.com/2026/02/24/discord-delays-global-rollout-of-age-verification-after-backlash/)
- [10scopes: BetterDiscord and the ToS](https://10scopes.com/is-betterdiscord-against-tos)
