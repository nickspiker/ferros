# PORTAL — where every request meets the machine
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Nothing acts on its own authority: everything is a granted ask or the answer to one.
**Glossary:** *ira*, *wairua*, *ihi*, handle, handle_proof are defined in [GLOSSARY.md](GLOSSARY.md).

---

## In One Breath

The *portal* is the one boundary where every request to a device arrives as a signed, exact VSF value, is checked against grants the fleet wrote, is answered, and is recorded.
It serves people, other fleet devices, agents and local programs identically.
The terminal was only the human-facing slice of it.

The name is Latin *porta*, from the Proto-Indo-European root *per-*, "to cross over" — the same root as ferry, ford, fare and port, and as the *port* in Photon Transport.
A portal is first a crossing and only second a guarded one, which is how it should feel: in the normal case you are reaching your own things and the guard is never felt.

---

## 1. What a Terminal Actually Was

A terminal is a person, reduced to the minimum a program needs in order to work with them.
It carries four things:

- **Presence** — "a human is here." The kernel reduces this to one yes/no answer (`isatty`).
- **Intent in** — keystrokes.
- **Attention out** — characters for the person to see.
- **Urgency, outside the flow** — stop (Ctrl-C), resized, left (hang-up), don't echo what I type.

It comes in two shapes, stacked by history:

- **The log** — the teletype's paper roll: append-only, a record, a conversation.
- **The surface** — the VT100's movable cursor: a live rectangle a program owns and redraws, with no history.

Its original sin is in-band signalling: text, colour, cursor moves, prompts and window titles all share one byte stream, so any file you `cat` can move the cursor or fake a prompt.
It is the 2600 Hz whistle on the voice line.

Unix's terminal also misses one thing entirely: **who**.
`isatty` answers "is someone watching?" and never "which someone, and what may they do?"; that answer lives elsewhere, in the uid the process happens to run as.

---

## 2. The Portal Is the Distillation

| Terminal part | Unix form | Portal form |
|---|---|---|
| Presence + who | `isatty` + uid | A signed session: the signing device resolves to an identity through the chain |
| Intent in | Keystrokes on a byte stream | Signed VSF requests, exact-matched |
| Attention out | Characters + escape codes | Typed VSF values; the viewer chooses the rendering and number base |
| Urgency | Signals, terminal settings | Distinct message types: cancel, resize, secret-input |
| Left | SIGHUP | The *wairua* dies — the session key is gone, no signal needed |
| Log shape | Paper roll | The ledger: append-only, replayable |
| Surface shape | Cursor addressing | A granted display region: live, no history, revocable |

This is PIPE's **host asks, enclave answers** applied to a whole device.
The device never volunteers; information flows only in answer to a request.

---

## 3. Wire: Strict VSF, Never Text

The device never parses text.
It compares bytes against patterns: a match runs, anything else is refused, with no fallback and no guessing.

- **Shell injection, quoting bugs and spaces-in-filenames do not exist**, because nothing interprets what you meant.
- **In-band signalling does not exist.** Control is a different type marker in strict framing; payload bytes are a value inside a frame and cannot escape it.
- **Number base is only a display choice.** Values are self-describing, so a hex inspector, a G# view, an A# view or dozenal are all valid renderings of the same bytes. The `[base36]#` convention lives entirely in viewers.
- **Inspection needs no ferros.** Any VSF or hex tool can read the traffic and the ledger.
- **Errors are values.** A typed result replaces stdout/stderr and exit codes.

### Canonical encoding is load-bearing

Exact matching and signatures only mean something if each value has exactly one valid byte form.
Otherwise one logical request has several encodings, each with its own valid signature, and the matcher sees "different" requests that decode to the same thing.

- **Landed (vsf `src/ewe.rs`, 2026-09-25):** `write_uint`/`write_int` always emit the narrowest width marker; `read_uint::<T>`/`read_int::<T>` reject any value a narrower marker would hold and any value that doesn't fit `T` (no silent truncation). All lengths and counts now go thru these. No version bump: every writer already emitted minimal lengths, so the strict reader accepts everything valid ever written.
- **The rule, width-agnostic:** each marker is exactly twice the width of the one below, so "a narrower marker would hold this" is "the upper half of the bytes is empty" (unsigned) or "the upper half is only sign extension" (signed). The same check works from `4` to `Z`.
- **Pending:** typed integers (`u3`..`u7`, `i3`..`i7`) are still written at fixed width, so `u4(5)` and `u3(5)` are distinct values. Making them minimal-only is a wire break (existing files hold padded `u6` values) and needs a version bump; it also collapses the width variants in `VsfType`.

---

## 4. Who Is Asking

"Who" is two facts, and Photon already has both.

| Question | Answer | Where it lives |
|---|---|---|
| Which person? | The identity; its public name is the handle_proof | Derived from the handle, which is never stored or sent |
| Which hardware? | The device's *ira* keypair (its brand) | Permanent, public |
| Is this hardware theirs right now? | The membership chain: member, locked, or leased proxy | Written by the fleet, never by the device |

A request is signed by a device's *ira* key; the portal replays the chain to resolve the signer to an unlocked member of an identity's fleet, or to a grantee.
This is the rule Photon's worker already applies to `fanout_put`.

**There is no login.**
Membership is a standing fact in the chain and every message carries its own proof.
The handle is typed once, at genesis or recovery, to derive the identity seed, and never appears in the portal.
The closest thing to a session is the *wairua*, which bounds how long a channel lives, not who you are.

---

## 5. Grants: the Lease, Generalized

The chain already is the privilege list: owned unlocked members hold full authority for their identity, locked members are refused everything, leased proxies act for a guest with no vote and no durable vault.
Anything narrower is a **grant**, which is the device lease ([photon/docs/device-lease.md](../photon/docs/device-lease.md)) generalized:

```
grant = { issuer: fleet signature, subject: handle_proof or process ihi or anyone, patterns: [ ... ], bounds: expiry / count / device, rules: decision tree, confirm: fleet device + trusted path }
```

`patterns` are the matcher's patterns, so **the dispatch table and the capability list are the same structure**.
`rules` and `confirm` are optional; §12 shows both.
For every request the portal checks, in order:

1. The signature is valid.
2. The signer folds as an unlocked member, or is named by a live grant.
3. The request matches one of that signer's patterns exactly.
4. If the grant carries `rules`, they run on the matched request and decide: allow, confirm on a named device, or deny.

Anything that fails is refused.
ferros enforces the list and never writes it — the fleet writes it, the same rule as "the device never assigns its own ownership."

### Patterns

A pattern is exact bytes plus typed holes: the verb matches exactly, each argument slot matches by type (for example "any unsigned in this range", "a key belonging to this fleet").
No regex, no backtracking, no conditionals, so matching stays trivially auditable.
Range checks live in the pattern, not in integer width.

### Discovery and the shared vocabulary

Each device publishes its pattern table as VSF; this replaces `--help` and tab completion.
Front ends read it and build only valid requests, so people get discovery and typo tolerance on the client side while the device stays strict.

For one tap to work against any fleet member, devices need a **small shared verb vocabulary** (list, fetch, and a few more, over typed content such as images, audio and files).
Devices can publish extra verbs of their own on top.
This is the glossary's one-word-per-thing rule applied at the protocol level.

---

## 6. Message Kinds and Shapes

Four kinds cross the portal:

- **Ask** — a signed request.
- **Answer** — a typed result, an error value, or a stream of values.
- **Urgency** — cancel, resize. Distinct types, so no payload can forge them. "Left" is not a message: the session dies with its *wairua*.
- **Secret-input** — a request that the OS collect something the requester must not see drawn by anyone else (see §7).

A session takes one of two shapes:

- **Log** — ledger-backed and append-only; every ask and answer recorded.
- **Surface** — a granted display region; live, no history, revocable.

---

## 7. The Trusted Path

Echo-off and fake password prompts are the same problem: the person must know when the OS itself is asking.

- Only the OS draws credential and confirmation prompts.
- Programs may *request* one with a secret-input message; they can never draw one.
- The OS prompt must look different in a way no granted surface can imitate (screen-edge chrome, a hardware LED).

Physical presence — a hand on the glass — is a fact no signed message can express.
Some verbs require a signed request **plus** an OS-drawn local confirmation: release, unlock, adding a grant.
This is a second input to the grant check, not a replacement, and it shares its shape with the proximity-gated wake policy.

---

## 8. No Users

The Unix "user" bundled two jobs; ferros already has a separate word for each, plus one Unix never had.

| Concept | What it answers | Source |
|---|---|---|
| **Identity** | Which person is resident and accountable | The chain; one resident identity per device at a time (owner, or a leased guest) |
| **Process *ihi*** | What this code provably is | The oracle at spawn: signed binary + developer key ([ORACLE.md](ORACLE.md)) |
| **Capability** | What this running code may touch right now | Kernel tokens in the process's CSpace ([KERNEL.md](KERNEL.md) Module 2) |

Android reused the UID for app isolation because a phone has one person, and much of its permission mess follows from treating an app like a person who can be granted "access to Photos."
In ferros an app is code with an *ihi*, not a person: it holds only the capabilities it was handed and cannot address or enumerate anything else.

- An app's authority is a slice of the resident identity's authority, granted explicitly thru the portal and recorded in the ledger — for example read-only on one album, time-limited.
- The grant names the app by its process *ihi*. The developer's `option` choice decides what that survives: `hp` for a grant that survives updates, `signature` for one locked to this exact binary.
- An app never signs as the person. Needing more than it holds means asking the portal; anything sensitive goes thru the trusted path.
- When the resident identity changes (lease recall, reboot), everything derived from it becomes invalid, so no app carries authority across a change of person.

---

## 9. Relationship to the Kernel

The kernel does not authenticate, network or display ([KERNEL.md](KERNEL.md), "What the Kernel Explicitly Does Not Do"), so **the portal is a userspace server**, beside TOKEN Auth and Photon.
It is the one place identity authority becomes kernel authority.

| | Portal grant | Kernel capability |
|---|---|---|
| Held by | An identity (thru a device), or a process *ihi* | A process |
| Form | Fleet-signed; patterns name the allowed requests | BLAKE3 token, epoch-bound, single-use nonce |
| Lifetime | Durable, in the chain | In-memory; dies with the process epoch |
| Enforced by | Signature + chain replay | Kernel IPC |

The portal holds kernel capabilities to the servers it can dispatch to.
When a request matches a grant it calls the right server over kernel IPC, first deriving a narrower child capability when the grant is narrower (read-only, time-limited).
**No kernel capability ever leaves the device.**

The kernel's call model already is ask/answer: a call issues a single-use reply capability that exists once and dies after use.
The portal extends that model past the edge of the device.

---

## 10. Clients

The portal is a channel, not a program.
Everything that talks to a device is a client of it:

| Client | Transport |
|---|---|
| `ferros-bridge` on a host | USB (Photon Transport) |
| Another fleet device | Network |
| The on-device UI | Local IPC, plus the trusted path |
| An agent | Any; holds its own grant |
| A local program | Kernel IPC, by its process *ihi* |

A ferros device and a Linux or macOS fleet member speak the same portal; the only difference is where enforcement lives (in the OS itself on ferros, wherever Photon runs elsewhere).

---

## 11. Worked Example: Photos From the MacBook

1. **Intent.** You tap "photos on the MacBook." No command, no syntax.
2. **Discovery.** The phone already holds the MacBook's published pattern table and sees "list images" with typed holes for a date range or folder; the UI fills them from the tap.
3. **Ask.** The phone signs the request with its *ira* key and sends it over Photon.
4. **Check.** The MacBook's portal replays the chain: same identity, unlocked member — allowed. You never see this step.
5. **Dispatch.** The portal calls the storage server over kernel IPC with a read-only capability scoped to photos.
6. **Answer.** The reply comes back on the single-use reply capability; the portal streams the images back as VSF image values, which the phone renders directly.
7. **Record.** Both ledgers hold who asked, what was asked, and what was answered.

A command was never text, so any front end can build one: a tap, voice, an agent, or a hex editor.
This is on-demand asking, distinct from fleet sync, which replicates everything ahead of time.

---

## 12. Worked Example: Buying a Beer

### The inversion

A card number is a bearer secret printed on the front of a plastic rectangle.
Handing it over shares your account: anyone who holds the number can **pull** money, and everything built since (CVV, fraud scoring, chargebacks, card reissue) is damage control for having published the key.

The portal turns this inside out.
The merchant issues a one-time payment **request**, and only you can answer it: you **push** money to the request.

| | Card (pull) | Portal (push) |
|---|---|---|
| What crosses the counter | Your account's bearer secret | The merchant's request for one payment |
| Who can move money | Anyone holding the number, any amount, until it's cancelled | Only whoever answers this request, this amount, once |
| Secret exposure | Every purchase, every merchant, every breach | None; nothing of yours leaves your devices |
| Leak consequence | Account drained, card reissued | Nothing: the request is not secret and only pays the merchant |
| Replay | Same number works again | The nonce is spent; the approval matches one request's hash only |
| Consent | Implied by handing the card over | Explicit, per payment, on a device you chose |

The request is public on purpose.
Anyone could pay it, because paying it only ever sends money to the merchant; that is how a friend could pay for your beer.
NFC's few-centimetre range keeps it to whoever is physically at the counter.

### The consent flow

1. **Tap.** The 7-Eleven terminal sends the phone a signed request over NFC: `{merchant key, amount, currency, nonce, expiry}`, canonical VSF.
2. **Pattern.** The merchant is not in the fleet, so ordinarily it would be refused. One grant makes a narrow exception: *anyone* may send the payment-request pattern, and all it can do by itself is create a pending request. It cannot move money.
3. **Rules.** The grant's ruleset (below) runs on the typed fields of the request and decides: allow, confirm on a named device, or deny.
4. **Trusted path, on the watch.** When the rules say confirm, the watch's OS (never an app) draws the prompt — merchant name, amount, "first time paying this key" when it is — and you tap approve or deny.
5. **Approval.** The watch signs the hash of that exact request with its *ira* key. Canonical encoding is what makes this safe: the request has exactly one byte form, so the approval can only ever match this request. It cannot be reused, altered, or applied to a different amount.
6. **Pay.** The phone's portal sees the request plus a valid watch approval over its hash; the grant is satisfied; it calls the wallet server over kernel IPC with a capability for this one payment. The portal authorizes; moving the funds is the wallet's job and outside this document.
7. **Record.** The nonce is spent. The merchant's ledger and yours both hold the request, the decision, who approved it, and on which device. A deny is an answer too: the terminal gets a typed refusal, and the ledger records that as well.

### The ruleset is yours

The owner builds the ruleset on a device, as a flowchart, not a configuration file.
It is a finite, acyclic decision tree over the request's typed fields and a little local state (spent today, keys paid before).
Every leaf is one of three outcomes: **allow**, **confirm on a named device**, or **deny**.
No loops and no calls out, so any ruleset can be read and audited in full, and its outcome for any request can be computed ahead of time.

One owner's flowchart for payment requests:

```
                    payment request arrives (NFC)
                                 │
                      expired or nonce seen? ──yes──▶ DENY
                                 │ no
                   merchant key on my block list? ──yes──▶ DENY
                                 │ no
               today's spend + amount > daily cap? ──yes──▶ DENY
                                 │ no
                   paid this merchant key before?
                        │ no                 │ yes
                        ▼                    ▼
           CONFIRM on watch, tap      amount ≤ A#20?
           + "first time" banner       │ yes        │ no
                                       ▼            ▼
                         ALLOW, watch buzzes   amount ≤ A#200?
                          (tap within 4 s       │ yes          │ no
                           to cancel)           ▼              ▼
                                        CONFIRM on watch   CONFIRM on watch
                                            (tap)          AND phone (tap)
```

The same flowchart as the grant it compiles into:

```
grant = {
  subject:  anyone,
  patterns: [ payment-request ],
  bounds:   { one use per nonce, expiry ≤ 128 s },
  rules: [
    expired | nonce seen            → deny,
    merchant ∈ blocklist            → deny,
    spent_today + amount > cap      → deny,
    merchant ∉ paid_before          → confirm(watch, banner: first-time),
    amount ≤ A#20                   → allow(notify: watch, cancel window: 4 s),
    amount ≤ A#200                  → confirm(watch),
    otherwise                       → confirm(watch, phone),
  ],
  confirm:  { devices: fleet members only, drawn by: OS trusted path },
}
```

Rules are evaluated top to bottom and the first match wins, so the flowchart and the list are the same tree.
The ruleset is itself a grant, so changing it follows the grant rules: the fleet writes it, and adding or loosening one needs an OS-drawn confirmation with a hand on the glass (§7).
Tightening (a lower cap, another blocked key) can be allowed without one, which makes "lock it down" fast when a device goes missing.

### What this example asks of the design

- **`confirm` names a device other than the one that received the request.** Physical presence is not tied to the device doing the talking; the watch is a fleet member and its OS owns its own trusted path.
- **Rules read local state** (spend so far, keys paid before). That state lives in the vault and its updates go to the ledger, so a rule's decision can be replayed and checked later.
- **An unreachable confirm device is a leaf too.** If the watch is dead or out of range, the ruleset decides: fall back to confirm on the phone, or deny. The default is deny.

### Open problems

- **Relay attacks.** NFC range is physics, but an attacker can relay NFC traffic over the internet, and this is a known attack on contactless cards today. The watch prompt covers most of it — you see who you are paying and how much, so a relay can only get you to approve a real request you can see. Distance bounding (a round trip too fast for a relay) closes the rest, where the NFC hardware supports it.
- **Merchant names.** The watch shows "7-Eleven," but the request only proves it came from some key. Something has to vouch that the key is that store: your own petnames ("I've paid this key before"), a merchant directory the fleet trusts, or a prominent first-time banner. This is the handle/petname problem Photon solves for people, applied to shops.

---

## 13. Where the Code Is Today

- **The seed exists.** [ferros_pt/src/command.rs](ferros_pt/src/command.rs) defines `[cap: 32][op: 1][params]` with dev caps that are unauthenticated BLAKE3 hashes of well-known names. Dispatch runs inside the kernel's USB event loop in [ferros_kernel/src/main.rs](ferros_kernel/src/main.rs), in two copies (M1 and husky).
- **There is no userspace yet**, so the portal starts as a kernel module that replaces both dispatch loops, shaped to move out into a server once processes and IPC exist.
- **Wire format changes.** [BRIDGE.md](BRIDGE.md) plans for production to "delegate real credentials over the identical wire format," which would send capability tokens as bearer tokens: whoever holds one can use it, and nothing ties a request to an identity for the ledger. The portal instead sends signed VSF requests and keeps capabilities on the device. The op/params shape can carry over.
- **`ferros-bridge terminal`** (one line per PT transfer) becomes a text front end that builds VSF requests from the published pattern table.

---

## 14. Build Order

Everything except step 3 can be built and tested now over USB, with a hardcoded member key standing in for the chain.

1. **Wire** — strict canonical VSF. Integer lengths/counts done; typed-integer canonicalization pending (§3).
2. **Matcher** — exact-byte patterns with typed holes; the table published as VSF.
3. **Authority** — chain replay to resolve signers. Needs the vault, which needs UFS on husky ([UFS.md](UFS.md)).
4. **Record** — every ask and answer to the ledger.
5. **Trusted path** — OS-drawn confirmation for the verbs that need a hand on the glass.
6. **Clients** — the bridge first, then the on-device UI, fleet peers and agents.

---

## 15. Open Questions

- **Shared verb vocabulary.** Which verbs are common to every fleet member, and over which content types.
- **Session state.** Fully stateless requests (everything explicit) or REPL-style state (working directory, variables)? Leaning stateless, with front ends holding any convenience state.
- **Typed-integer canonicalization.** Whether and when to take the version bump (§3).
- **Big integers.** Markers above `7` are valid EWE but have no native reader; add one only if something needs it.
