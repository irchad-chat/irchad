# ergo capability check

**Verdict: GO.** Probed ergo **v2.19.1** on the dev stack on 2026-10-27, with `deploy/tools/ergo-probe.py` (re-run it after any ergo upgrade).
The raw output is in the appendix.

## Findings

### Capabilities we rely on are all present
`draft/chathistory` (`CHATHISTORY=1000` per request), `draft/event-playback`, `draft/read-marker`, `draft/message-redaction`, `draft/multiline` (4096 bytes / 100 lines),
`draft/metadata-2` **and** `draft/metadata-3` (both `before-connect`, max-subs 100, max-keys 100), `draft/account-registration=before-connect`,
`labeled-response`, `echo-message`, `message-tags`, `server-time`, `batch`, `account-tag`, `extended-join`, `away-notify`, `account-notify`,
`no-implicit-names`, `draft/pre-away`, `draft/persistence`, and `sasl=PLAIN,EXTERNAL,SCRAM-SHA-256`.
ISUPPORT: `CASEMAPPING=ascii`, `UTF8ONLY`, `CHANTYPES=#`, `PREFIX=(qaohv)~&@%+`, `MSGREFTYPES=msgid,timestamp`, `NICKLEN=32`, `CHANNELLEN=64`, `TOPICLEN=390`.

Not offered: `draft/webpush`, because `webpush.enabled` is false in our config. Turn it on in Phase 6.

### Metadata limits
- **key + value ≤ 350 bytes**, hardcoded (`maxCombinedMetadataLenBytes`).
  A 340-byte value under `chad/probe` (10 bytes) is accepted, and 341 bytes gets `FAIL METADATA INVALID_VALUE … :Value is too long`.
  There's also an undocumented `metadata.max-value-length` config knob that can only lower this.
- **Max 100 keys per target** and 100 subscriptions (configurable).
- **Keys must match `^[a-z0-9_./-]+$`**, so they're lowercase only. `Chad/Bad` gets `FAIL METADATA INVALID_KEY`. Our `chad/<name>` convention fits.
- **Rate limit: 10 SETs per 2 minutes per client** by default (`metadata.client-throttle`). The error is `FAIL METADATA RATE_LIMITED <target> <key> <secs>`.
  Deletes count too. ⚠ Phase 2 sets `chad/voice` on every voice join and leave, so hopping between rooms will hit this.
  Either raise the throttle in our recommended ergo config (e.g. 30 per minute) or debounce in the client. Decide in Phase 2.
- ⚠ **Phase 4:** 350 bytes is enough for an Olm identity key pair (curve25519 + ed25519, about 90 bytes base64) plus a signature, but not a batch of one-time keys.
  OTKs have to go one key per metadata entry (`chad/otk/<id>`, which uses up the 100-key budget) or be fetched another way. Design this in Phase 4.

### Client-only tags survive chathistory ✅
`@+chad/e2e=… PRIVMSG #dev` and `… PRIVMSG bob` are echoed with the tag, and `CHATHISTORY LATEST` replays it byte-for-byte for both the channel and the DM.
That includes a **3200-character** tag value, roughly an Olm pre-key message. Multiple client tags (`+draft/reply` alongside it) are preserved.
The tag-data limit is ergo's 4094 bytes for client tags: a 4100-char tag gets `417 :Input line contained excess tag data`, and the message is dropped.
This is what Phase 4 depends on, so it's a go. Keep E2EE payloads under about 3.5 KB of base64 per message, or split them across a `draft/multiline` batch.

### SASL with device IDs ✅
- `PLAIN` with authcid `bob@laptop` works, but **authzid must be empty** (or equal to the full `bob@laptop`).
  ergo strips `@device` from the authcid only, then compares authzid against the unstripped authcid.
- `SCRAM-SHA-256` with username `bob@laptop` works, and so does a plain `bob`.
- The HTTP API's `check_auth` does **not** strip device IDs (see `ergo-api.md`).

### Always-on behaviour ✅
- With no connected sessions, `WHOIS bob` shows `301 … :User is currently disconnected`. So auto-away (`opt-out`) marks disconnected always-on users away.
- **Reattach burst** (`bob@laptop`, caps include `no-implicit-names`), after the 001–005 numerics, LUSERS and MOTD:
  - `AWAY` (clearing our own away), then a `metadata` batch for ourselves
  - then per channel: our own `JOIN #chan` (extended-join form), `MARKREAD #chan *` (the read marker, `*` when unset), and `332`/`333` topic.
  - then a `metadata *ALL` batch.
  - There is **no `NAMES`** because we asked for `no-implicit-names`. Without that cap you get 353/366 per channel. The client should request it and send `NAMES` or `WHO` lazily per visible channel.
  - **No message replay** (no `autoreplay-missed`), which matches D11. The client backfills with chathistory.
- `CHATHISTORY TARGETS timestamp=<last seen> timestamp=<far future> 50` listed both the DM from alice **sent while bob was offline** and `#general`.
  That's the gap-fill mechanism the architecture assumes.

### Ops notes for the dev stack
- With rootless podman port forwarding, every host connection arrives from the compose network. That tripped ergo's per-IP connection throttle (32 per 10 minutes) during probing.
  The dev config now exempts `10.89.77.0/24`. It's dev only, because in production Caddy forwards real IPs via `X-Forwarded-For`.
- `NickServ` / `ChanServ` have no `SAREGISTER` for channels. The seed script uses oper `SAMODE` and then `CS REGISTER`.

## Appendix: probe output


## CAP LS 302

```
account-notify
account-tag
away-notify
batch
cap-notify
chghost
draft/account-registration=before-connect
draft/channel-rename
draft/chathistory
draft/event-playback
draft/extended-isupport
draft/languages=17,en,~bs,~de,~el,~en-AU,~es,~fi,~fr-FR,~it,~nl,~no,~pl,~pt-BR,~ro,~sq-AL,~tr-TR,~zh-CN
draft/message-redaction
draft/metadata-2=before-connect,max-subs=100,max-keys=100
draft/metadata-3=before-connect,max-subs=100,max-keys=100
draft/multiline=max-bytes=4096,max-lines=100
draft/persistence
draft/pre-away
draft/read-marker
draft/relaymsg=/
draft/whoami
echo-message
ergo.chat/nope
extended-join
extended-monitor
invite-notify
labeled-response
message-tags
multi-prefix
no-implicit-names
sasl=PLAIN,EXTERNAL,SCRAM-SHA-256
server-time
setname
standard-replies
userhost-in-names
znc.in/playback
znc.in/self-message
```

## ISUPPORT (005)

```
@time=2026-10-28T01:54:14.871Z :chat.localhost 005 alice AWAYLEN=390 BOT=B CASEMAPPING=ascii CHANLIMIT=#:100 CHANMODES=Ibe,k,fl,CEMRUimnstu CHANNELLEN=64 CHANTYPES=# CHATHISTORY=1000 ELIST=U EXCEPTS EXTBAN=,m FORWARD=f INVEX :are supported by this server
@time=2026-10-28T01:54:14.871Z :chat.localhost 005 alice KICKLEN=390 MAXLIST=beI:100 MAXTARGETS=4 MODES MONITOR=100 MSGREFTYPES=msgid,timestamp NETWORK=ErgoTest NICKLEN=32 PREFIX=(qaohv)~&@%+ SAFELIST SAFERATE STATUSMSG=~&@%+ TARGMAX=NAMES:1,LIST:1,KICK:,WHOIS:1,USERHOST:10,PRIVMSG:4,TAGMSG:4,NOTICE:4,MONITOR:100 :are supported by this server
@time=2026-10-28T01:54:14.871Z :chat.localhost 005 alice TOPICLEN=390 UTF8ONLY WHOX draft/CHATHISTORY=1000 :are supported by this server
```

SASL PLAIN as `alice@probe`: ok

## Metadata limits

```
SET chad/probe value=300B (key+value=310B) -> @time=2026-10-28T01:54:15.871Z :chat.localhost 761 alice alice chad/probe * xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
SET chad/probe value=340B (key+value=350B) -> @time=2026-10-28T01:54:15.871Z :chat.localhost 761 alice alice chad/probe * xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
SET chad/probe value=341B (key+value=351B) -> @time=2026-10-28T01:54:15.872Z :chat.localhost FAIL METADATA INVALID_VALUE chad/probe :Value is too long
SET chad/probe value=400B (key+value=410B) -> @time=2026-10-28T01:54:15.872Z :chat.localhost FAIL METADATA INVALID_VALUE chad/probe :Value is too long
SET Chad/Bad (uppercase key) -> @time=2026-10-28T01:54:15.872Z :chat.localhost FAIL METADATA INVALID_KEY Chad/Bad :Invalid key name
rate limit: 6 SETs before -> @time=2026-10-28T01:54:19.376Z :chat.localhost FAIL METADATA RATE_LIMITED alice chad/rl 117 :Please wait at least 1m56.495s and try again
```

## Client-only tags on PRIVMSG through CHATHISTORY

echo-message lines: 3; tag present on echoes: [True, True, True]
CHATHISTORY LATEST #dev * 2:
```
@time=2026-10-28T01:54:22.181Z :chat.localhost BATCH +3 chathistory #dev
@account=alice;msgid=scjg5wsvxbrichvzgw6gd8wu3s;time=2026-10-28T01:54:21.178Z;batch=3;+chad/e2e=AwogQUJD :alice!~u@vzgc9c3e6jsn4.irc PRIVMSG #dev :[probe small tag]
@account=alice;msgid=m8rbb66wupfxxuerbhkyj4r7uw;time=2026-10-28T01:54:21.180Z;batch=3;+chad/e2e=GrdNO8phrBss+m7+pQRHP/UeZI/5O6Mnu7PXUKpMe4VocAe2u+fXb0KzKx1dJ6kAcro7ARcwdpBnvhbLSVbJrBWQGWwklVh02LcmdYF37o9cMOWQZ3SoJudLn2JZjDCo1JeHD2+OVTfprC7m0xnobK/XWssGq7boMNk9l1EYfufz+hv98kO6hX0SATT+CQpN8+m5hrCMIVkrLynf2mJjHELVVxKmjUDu615noG/Y8w4WC3ldfJq6pmSdDDnCGW+4itCq4085N8avgvk8VDJPd1B9pCVi3Ubp9XB9hSrBTdpymaAP… (3354 bytes)
@time=2026-10-28T01:54:22.181Z :chat.localhost BATCH -3
```
big tag (3200 chars) survives replay intact: True
CHATHISTORY LATEST bob * 1:
```
@time=2026-10-28T01:54:23.181Z :chat.localhost BATCH +4 chathistory bob
@account=alice;msgid=3rttne5aup4dknrrwejhmgx8i6;time=2026-10-28T01:54:21.180Z;batch=4;+chad/e2e=AwogQUJD;+draft/reply=fake :alice!~u@vzgc9c3e6jsn4.irc PRIVMSG bob :[probe dm tag]
@time=2026-10-28T01:54:23.181Z :chat.localhost BATCH -4
```
oversize (4100-char) tag:
```
@time=2026-10-28T01:54:24.181Z :chat.localhost 417 alice :Input line contained excess tag data
```

## SASL usernames with device IDs

- PLAIN as `bob@laptop`: ok
- SCRAM-SHA-256 as `bob@laptop`: ok
- SCRAM-SHA-256 as `bob`: ok

## Always-on: offline DMs and attach burst

WHOIS bob while bob has no sessions (away = auto-away working):
```
@time=2026-10-28T01:54:27.190Z :chat.localhost 311 alice bob ~u vzgc9c3e6jsn4.irc * bob
@time=2026-10-28T01:54:27.190Z :chat.localhost 330 alice bob bob :is logged in as
@time=2026-10-28T01:54:27.190Z :chat.localhost 301 alice bob :User is currently disconnected
@time=2026-10-28T01:54:27.190Z :chat.localhost 318 alice bob :End of /WHOIS list
```
bob@laptop reattach (SCRAM ok=True); burst after 001, counts by command:
```
001=1, 002=1, 003=1, 004=1, 005=3, 221=1, 251=1, 252=1, 253=1, 254=1, 255=1, 265=1, 266=1, 332=3, 333=3, 372=35, 375=1, 376=1, 900=1, 903=1, AUTHENTICATE=3, AWAY=1, BATCH=4, CAP=3, JOIN=3, MARKREAD=3
```
```
@account=bob;time=2026-10-28T01:54:27.692Z :bob!~u@vzgc9c3e6jsn4.irc AWAY
@time=2026-10-28T01:54:27.692Z :chat.localhost BATCH +1 metadata bob
@time=2026-10-28T01:54:27.692Z :chat.localhost BATCH -1
@time=2026-10-28T01:54:27.692Z :bob!~u@vzgc9c3e6jsn4.irc JOIN #general bob bob
@time=2026-10-28T01:54:27.692Z :chat.localhost MARKREAD #general *
@time=2026-10-28T01:54:27.692Z :chat.localhost 332 bob #general :General chat
@time=2026-10-28T01:54:27.692Z :chat.localhost 333 bob #general alice!~u@vzgc9c3e6jsn4.irc 1793151934
@time=2026-10-28T01:54:27.692Z :bob!~u@vzgc9c3e6jsn4.irc JOIN #random bob bob
@time=2026-10-28T01:54:27.692Z :chat.localhost MARKREAD #random *
@time=2026-10-28T01:54:27.692Z :chat.localhost 332 bob #random :Off topic
@time=2026-10-28T01:54:27.692Z :chat.localhost 333 bob #random alice!~u@vzgc9c3e6jsn4.irc 1793151936
@time=2026-10-28T01:54:27.692Z :bob!~u@vzgc9c3e6jsn4.irc JOIN #dev bob bob
@time=2026-10-28T01:54:27.692Z :chat.localhost MARKREAD #dev *
@time=2026-10-28T01:54:27.692Z :chat.localhost 332 bob #dev :IrChad development
@time=2026-10-28T01:54:27.692Z :chat.localhost 333 bob #dev alice!~u@vzgc9c3e6jsn4.irc 1793151938
@time=2026-10-28T01:54:27.692Z :chat.localhost BATCH +2 metadata *ALL
@time=2026-10-28T01:54:27.692Z :chat.localhost BATCH -2
```
CHATHISTORY TARGETS since 2026-10-28T01:54:25.000Z:
```
@draft/chathistory-end;time=2026-10-28T01:54:29.693Z :chat.localhost BATCH +3 draft/chathistory-targets
@time=2026-10-28T01:54:29.693Z;batch=3 :chat.localhost CHATHISTORY TARGETS alice 2026-10-28T01:54:26.689Z
@time=2026-10-28T01:54:29.693Z;batch=3 :chat.localhost CHATHISTORY TARGETS #general 2026-10-28T01:54:26.690Z
@time=2026-10-28T01:54:29.693Z :chat.localhost BATCH -3
```
