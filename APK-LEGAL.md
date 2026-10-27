# APK-LEGAL — Running Android Apps on ferros: Decision Log
**Version:** Zil (0)
**Started:** 2026-10-27
**Principle:** Replace, never impersonate. ferros tells the truth about what it is, and every app and service decides what to do with that truth.

---

## What this is

ferros cannot ship without running APKs, including Google apps, banking apps and government identity apps.
This file records every legal judgement the APK plan rests on: the question, the decision, the text it was made against, the reasoning, what it costs, and what would make us revisit it.
Nobody who wrote this is a lawyer, and nothing here is legal advice.
It is our best reading of the public terms and law available on the date of each entry, written down so the reasoning can be checked later, by us or by counsel.

The repository is public, and so is this file.
Every entry records a decision that stays on the right side of the line; if a question cannot be settled that way, it goes under Open questions, not into a decision.

### How to use it

- Each decision has a number (D0, D1, ...) that never changes or gets reused. A reversed decision is struck through and replaced by a new entry that says why.
- Code that implements or depends on a decision says so in one comment line, e.g. `// APK-LEGAL D2: attestation answers truthfully`. Changing that code means revisiting the entry first.
- Every terms document relied on is captured on the day it was read under `legal/snapshots/<date>/`, with its URL and BLAKE3 in that directory's `MANIFEST`. Terms change "from time to time"; what counts is what they said when we decided. A later re-read gets a new dated directory, never an overwrite.
- Quotes below are verbatim from the snapshots (checked by text search on the capture date).

---

## Decisions

### D0 — ferros replaces Android; it does not impersonate it (2026-10-27)

**Decision:** ferros is not Android and does not run Android.
APKs run on ferros's own implementation of the Android app interfaces (the framework APIs apps call, the bionic/Linux system-call personality their native code expects).
ferros ships none of Google's code: no Google APKs, no Google Play Services, no Android system images.
Apps come from the user's own sources under the user's own licences.

**Why:** Reimplementing an interface so existing programs run on a new platform is the situation in *Google LLC v. Oracle America, Inc.*, 593 U.S. 1 (2021), where Google's own reimplementation of the Java API declarations was held to be fair use.
Shipping none of Google's code keeps ferros out of Google's software licence entirely; the licence in the Google ToS is "personal" and "non-assignable" and forbids users to "copy, modify, distribute, sell, or lease any part of our services or software" (snapshot `google-terms.html`, effective July 30, 2026).

**Cost:** Everything Google Play Services provides must be reimplemented or done without (see O0).

**Revisit if:** a court narrows *Google v. Oracle* for platform reimplementation, or ferros ever wants to ship a Google component.

### D1 — APKs are compiled on the device at install, sealed to it, never distributed (2026-10-27)

**Decision:** An APK is never translated or interpreted at runtime.
At install, the device compiles the APK (DEX bytecode and any arm64 native libraries) for itself alone; the compiler checks it (no jumps outside its arcs, no forbidden instructions) and the resulting capsule is sealed and keyed to that device's hardware.
A compiled capsule never leaves the device it was compiled on: no capsule cache, no sharing between devices, no download of precompiled capsules.

**Why:** Security first: runtime translation is the attack surface ferros exists to remove, and the seal proves the capsule passed the checker (SANDBOX.md, trust model A).
Legally, compiling an app to native code on the device is what Android's own runtime has done since Android 5.0 (ART's install-time and profile-guided compilation), so on-device compilation is how every app developer already expects their code to run.
Because the compiled output is personal to one device and never distributed, it stays inside a "personal" licence and outside the "copy, modify, distribute" prohibition in D0's quote.
ferros does not modify or tamper with an app: it runs the app's code as is, after a safety check and an optimisation pass for the hardware, and what the app does is unchanged; turning code into the instructions of the machine that runs it is how all code runs, not a modification of it.
Compiling at install and keeping the result is the same act as compiling at launch, only earlier: every platform that runs an app turns its code into a distinct native form in memory before executing it (ART's ahead-of-time and JIT compilers, and any JIT anywhere), so "compiled for this device" is not a modified app in any sense that running it is not.
The stored form is encrypted and keyed to the one device, with one key that unlocks it, so it is never copied, reproduced or published: nobody but that device can read it, and it is never shared.
Emulating a platform so its software runs elsewhere was held lawful in *Sony Computer Entertainment v. Connectix*, 203 F.3d 596 (9th Cir. 2000); note that apps are licensed rather than sold (*Vernor v. Autodesk*, 621 F.3d 1102 (9th Cir. 2010)), so we lean on the licence terms and fair use, not on 17 U.S.C. §117.

**Cost:** Every device compiles every app it installs; no shared compile cache, ever.

**Revisit if:** a major app's licence forbids running it anywhere but on Android itself. Not seen so far. Modification and tampering clauses (Chase UK's, O6) do not reach this, because ferros does not modify the app.

### D2 — Attestation always tells the truth (2026-10-27)

**Decision:** When an app or service asks ferros to attest to the device's state, the answer is true, even when the truth loses a feature.
ferros never forges an attestation, never relays one from another device, never uses leaked or third-party attestation keys ("keyboxes"), and never extracts keys from the security hardware.
ferros ships no tool that does any of these, and does not tell users how to.

**Why:** The Google Play terms forbid any attempt "to, or assist, authorize or encourage others to circumvent, disable or defeat any of the security features or components that protect, obfuscate or otherwise restrict access to, or the manner or extent of use of, any Content or Google Play" (snapshot `google-play-terms.html`).
The Google ToS forbids "bypassing our systems or protective measures" (snapshot `google-terms.html`).
The Google Wallet User Policy (effective February 12, 2024) forbids users to "misrepresent yourself", including "providing misleading information about a user's identity, qualifications, ownership, purpose, products, services, or business", and forbids "degrading, disabling, or negatively interfering with any aspect of the product or its services" (snapshot `google-wallet-user-policy.html`); a forged attestation is a misrepresentation about the device, while a truthful one leaves Wallet working exactly as Google built it.
Under 17 U.S.C. §1201(a)(3)(A), circumvention means to "avoid, bypass, remove, deactivate, or impair a technological measure, without the authority of the copyright owner": the test is authorisation, not whether security went up overall, so "ferros is more secure" does not excuse a false attestation.
Leaked keyboxes are misappropriated signing keys; using them adds trade-secret and access-device exposure on top of the contract breach.
A truthful attestation defeats nothing: the security components keep working and report exactly what is running, and the service decides.

**Cost:** Services that require Google certification refuse features on ferros. Known now: Google Wallet tap-to-pay (Play Integrity device integrity is built on hardware-backed key attestation on Android 13 and later since May 2025, snapshot `play-integrity-improvements.html`). Passes, tickets and loyalty cards are unaffected.

**Revisit if:** never for the "no forgery" part. The route to the lost features is persuasion (services accepting ferros's attestation), not engineering around it.

### D3 — Certification is the service's to declare, not ours (2026-10-27)

**Decision:** ferros never claims to be Google-certified, Play Protect certified, or "supported" by any service unless that service says so.
When a service decides ferros does not meet its requirements, that decision stands.

**Why:** The terms themselves name no specific OS, certification or bootloader state; they defer to requirements the service sets and changes.
Google Play: "a Device that meets the system and compatibility requirements for the relevant Content, working Internet access, and compatible software" (snapshot `google-play-terms.html`).
Google Pay/Google Payments Additional Terms of Service (U.S.), last modified July 22, 2026, which govern Google Pay and Google Wallet payments in the US: "a device that meets the Service's system and compatibility requirements, which may change from time to time; working Internet access; and compatible software. Your ability to use Google Pay, and Google Pay's performance, may be affected by these factors. Such system requirements are your responsibility." (snapshot `google-payments-buyer-terms-us.html`).
The same clause, word for word, is in the non-US Google Pay terms in both versions seen on 2026-10-27: last modified April 5, 2021 as served to the capture (snapshot `google-pay-terms-nonus.html`), and last modified December 10, 2025 as shown in the owner's browser the same day (not captured; the served version varies by session).
None of these documents mentions rooted, jailbroken or modified devices, or circumvention; the US terms bind the user to the Google ToS, themselves, and the Google Payments User Policies, so the circumvention and misrepresentation rules in D2 come from those.
ferros running an app correctly is a fair reading of "compatible software", but the "system and compatibility requirements" are the service's to publish, and a service applying them honestly against an honest ferros is the contract working as written, not a breach by anyone.

**Cost:** None beyond D2's.

**Revisit if:** a service's terms define compatibility in a way ferros meets on its face (then we can say so, with the quote).

### D4 — What ferros reports about itself (2026-10-27)

**Decision:** Every field an app can read about the device and OS is filled in truthfully, by kind:
- hardware fields (manufacturer, model, board) report the hardware as it is: on husky, Google, Pixel 8 Pro;
- API level fields report the Android API level ferros actually implements, as a statement of compatibility;
- OS identity fields (build fingerprint, build display, OS name, version strings, user agents) say ferros, and never carry Google's or any vendor's build identity.

**Why:** Hardware fields are facts about the hardware, which really is a Pixel 8 Pro.
An API level is a claim about which interfaces are implemented, true if ferros implements them; this is the same convention as every web browser's user agent beginning "Mozilla/5.0" for compatibility, which has never been treated as deception.
A build fingerprint identifies the operating system build; reporting Google's would be claiming to be stock Android, which is the impersonation D0 rules out and the misrepresentation D2 rules out.
Honest identity also settles who decided: if an app can see exactly what ferros is and runs anyway, that choice is the app's.

**Cost:** Apps that refuse on OS identity alone will refuse.

**Revisit if:** a de-facto convention emerges for non-Android platforms implementing Android APIs (adopt it if it is honest).

### D5 — Secure-world software never holds ferros's secrets (2026-10-27)

**Decision:** Trusty, KeyMint, GSA and the Titan M2 never receive ferros's secrets: no ira, no wairua, no vault keys, no keys derived from them.
ferros already uses the secure world where hardware requires it (the AoC starts only through Trusty), and treats it as untrusted vendor plumbing.

**Why:** The secure-world images on husky ship as Google-signed binaries (`tzsw`, `gsa`, `bl31`, `ldfw`, ...).
Generic Trusty source is public in AOSP (snapshot `trusty-aosp.html`, "open-source"), but Google states "Third-party app development isn't supported in Trusty", and no published source or reproducible build was found for the husky images themselves.
Pixel binary transparency logs those images' hashes (snapshot `pixel-binary-transparency.html`), which proves everyone received the same image, not what it contains.
Code we cannot read does not hold keys we cannot replace.

**Cost:** Hardware-backed key storage for ferros's own secrets comes from ferros's design (ira, KOHI, the vault), not from the vendor TEE.

**Revisit if:** the husky secure-world images become reproducible from published source.

### D6 — The user's agreements are the user's; ferros's part ends at the truth (2026-10-27)

**Decision:** ferros is not a party to the agreements between its user and a bank, a government agency or an app vendor.
ferros's obligation is that everything it reports is true (D2, D4) and that it ships nothing built to defeat a service's checks.
Whether a user may run a given app on a ferros device under their own agreement is between the user and that provider; ferros does not hide that question from them.

**Why:** Google's own US payment terms put it in so many words: "Such system requirements are your responsibility" (snapshot `google-payments-buyer-terms-us.html`), meaning the user's, not the platform's.
Some providers' agreements bar modified or unsupported devices outright (common wording in other banks' terms, not yet captured here); a ferros device falls under that wording whether or not it is honest, and ferros cannot change the user's contract.
Chase's own documents captured so far are narrower: the UK app licence speaks of "an operating system version we no longer support" and forbids using the app "on a device that contains or is open to viruses and other things that may cause damage" (snapshot `chase-uk-app-licence.html`, version 1.1.0), and the US trusted-device page, an educational page and not contract terms, lists "iOS 16.0 or later or Android 11 or later" (snapshot `chase-us-trusted-device.html`).
Chase does check device integrity per feature: on GrapheneOS in July 2026 the credit-score feature alone refused with "For your security, the Chase Mobile app is unavailable on devices that have been 'rooted' or contain malware" while the rest of the app worked (snapshot `grapheneos-chase-thread.json`).
Where the platform tells the truth, the platform's own exposure is close to none; where a platform spoofs its identity to get apps running, the platform becomes the party helping its users get around a check and breach their agreements, which is the exposure this decision exists to avoid.

**Cost:** Some users will not be able to use some apps they want, under their own agreements.

**Revisit if:** a provider's agreement turns out to put obligations on the platform rather than the user.

### D7 — Each device is itself to Google; the identity signs it in; nothing asks the user (2026-10-27)

**Decision:** Every device in a fleet appears to Google (and to every service) as the device it is, under the identity's own account: its own check-in, its own device-bound tokens, its own push registration.
The user's one action that brings a device into the fleet is the only action: ferros then installs every app on it (compiled and checked on that machine, D1), grants the identity's permissions, and signs it in to each service on the identity's behalf, with no per-service step of any kind.
Sign-in uses the identity's passkey for each service, held by ferros as the identity's passkey provider and present on every fleet device.
Sync is the default for every app and every account; the owner may uncheck an app or an account for a given device, and nothing else turns it off.

**Why:** A device in the fleet is owned by the identity, so the fleet join is the owner's verification, and a passkey sign-in that follows from it honestly reports a verified user.
Passkeys are built for exactly this: one person's credential synced across their own devices by a passkey provider (as Google Password Manager, iCloud Keychain and password managers do), with the service holding only the public key.
Presenting several devices as one would break D4 (report the hardware as it is), and in practice breaks push (registrations collide) and trips account-takeover defences (one "device" in several places at once); copying one device's tokens to another adds a single point of compromise for the whole fleet.
Google's own model is already one person with many devices under one account, so nothing about the fleet needs hiding.

**Cost:** ferros must implement the Android credential-provider interface well enough that Google's sign-in (route B, O0) and every app take passkeys from it. A service can still issue its own "verify it's you" challenge when its risk system fires; ferros never creates a step, but cannot stop a third party from demanding one.

**Revisit if:** a service refuses passkeys from providers other than its own, or its terms bar the account on more than some number of devices.

---

## App classes

The same platform behaviour carries different weight depending on who is on the other end.

| Class | Examples | What governs | Line |
|---|---|---|---|
| Google apps | Voice, Wallet, Maps | Google ToS + Play terms + service terms (contract) | D0–D4; Wallet runs without tap-to-pay |
| Banking and payments | Chase and other banks | Each bank's agreement with the user; banks answer to financial regulators and pursue misuse harder than app vendors do | D2, D4, D6 |
| Government identity | state mobile driver's licences, Login.gov, ID.me | Agency terms, and statutes on identity documents and false statements to government | Truthful identity is a hard rule here, not a preference; misrepresenting the platform to one of these may be more than a contract matter |

---

## Open questions

**O0 — Google Play Services.** Decided by the owner 2026-10-27; stays open until route B runs.
Google apps never sign in themselves: they ask Play Services (package `com.google.android.gms`), which holds the account, fetches per-app tokens naming the app's package and signing certificate, and checks the device in with Google; many apps also verify that Play Services is signed by Google.
- Route A, microG, is **out**. microG takes Google's package name and needs the OS to tell apps it carries Google's signature ("signature spoofing", which its own documentation describes as letting an app "pretend that it has been signed by a different developer than it actually was"). That is the misrepresentation D2 and D4 rule out; without the spoof, apps will not bind to it.
- Route B, the target: Google's own unmodified Play Services and Play Store APKs run as ordinary sandboxed apps (GrapheneOS's GmsCompat model). The signature apps see is truly Google's; ferros ships none of it (D0); the user installs it; it is compiled, checked and sealed like any app (D1) and holds no ferros secret. Check-in reports the hardware and ferros honestly (D4); Google will see an uncertified device. Cost: Play Services assumes real Android underneath, so ferros's Android API layer must be deep enough for the hardest app there is.
- Route C, now: apps that need no Google account. Google Voice is the first Google app planned, so it is the first real requirement for route B.

**O1 — Attestation witness.** Apps that accept non-Google attestations (the route GrapheneOS asks app developers to take: check `verifiedBootState` = `SelfSigned` and a published `verifiedBootKey`, because "apps using it directly can support aftermarket operating systems", snapshot `grapheneos-attestation-guide.html`) need a hardware-signed statement, and the only keys banks already trust live in KeyMint/Titan behind the secure world.
Choice for the owner: relay a truthful hardware attestation with the secure world as the bank's witness only (D5 still holds: it receives no ferros secret), or refuse attestation entirely and accept that apps requiring it will not run.

**O2 — Release key and owner key.** BOOT.md plans boot integrity by relocking with the owner's key.
If every owner's key differs, no app can allowlist "ferros" by key hash; GrapheneOS publishes one project key for this reason.
Needs a design that lets an attestation say both "this is ferros" and "locked to this owner", or a decision that ferros gives up app allowlisting.

**O3 — US Google Wallet terms.** Resolved 2026-10-27.
The dedicated Wallet terms URL returned HTTP 500, but Google Pay and Wallet payments in the US are governed by the Google Pay/Google Payments Additional Terms of Service (U.S.), last modified July 22, 2026 (snapshot `google-payments-buyer-terms-us.html`), with the Google Wallet User Policy (snapshot `google-wallet-user-policy.html`).
Neither carries the original 2011 Google Wallet terms' reported bar on any OS "modified or customized in any way"; the device requirement is the "system and compatibility requirements" clause quoted in D3, and the conduct rules are quoted in D2.

**O4 — Chase US Digital Services Agreement.** Not yet read; D6's Chase facts come from the UK licence and a US educational page.

**O5 — Emergency services.** Owner's rule (2026-10-27): ferros has no POTS, SMS, MMS or 911, ever; emergencies go through Photon EMS (photons, beams and waves), and calls to phone numbers go through Google Voice once route B (O0) runs.
No conflict with 911 law: the FCC's wireless 911 duties fall on providers of interconnected voice service, and a data-only device on a data-only plan (an LTE laptop or tablet today) is not one; ferros's target is the Glyph (GLYPH.md), which has no POTS capability at all, and husky runs it the same way, data only.
The same holds for the MVNO: a mobile virtual network operator owes no emergency service unless it offers interconnected voice (and, under the FCC's text-to-911 rules, interconnected texting to phone numbers); ferros's MVNO offers neither, by the owner's rule above.
Where an app brings its own interconnected calling (Google Voice), that app's provider carries the 911 duties for its service; ferros's rule means ferros provides no 911, not that ferros blocks an app's own emergency dialling.
Google's two Voice documents disagree: the Voice Additional Terms say "Google Voice isn't capable of placing or receiving emergency services calls" (snapshot `google-voice-additional-terms.html`), while the Voice Personal terms, last modified June 9, 2026, say "Dialing to emergency services is supported by Google Voice Personal" with VoIP limitations (snapshot `google-voice-personal-terms.html`).
If Photon EMS ever reaches public-safety answering points directly, FCC 911 rules for internet telephony (location, call-back) likely apply to it; that needs its own entry before Photon EMS ships.

**O6 — Does install-time compilation "modify" an app?** Resolved 2026-10-27 by the owner: no.
Chase UK's app licence forbids users to "copy, reproduce, publish, modify or tamper with our app" (snapshot `chase-uk-app-licence.html`).
Compiling before running is what every runtime does (JIT compiles before the code runs, and the compiled code exists in memory in a distinct form); compiling at install only does it earlier.
The result is encrypted and keyed to one device with one key, never shared and never readable elsewhere, so nothing is copied, reproduced or published, and the app's meaning is unchanged (the checker refuses code, it does not rewrite it). The reasoning is now part of D1.

**O7 — Counsel before release.** Proposed ship gate: before ferros runs banking or government identity apps for anyone but the owner, one paid review of this log by a technology lawyer, recorded here with the date.

---

## Cases and statutes cited

- *Google LLC v. Oracle America, Inc.*, 593 U.S. 1 (2021) — reimplementing API declarations held fair use.
- *Sony Computer Entertainment, Inc. v. Connectix Corp.*, 203 F.3d 596 (9th Cir. 2000) — copying during reverse engineering to build an emulator held fair use.
- *Vernor v. Autodesk, Inc.*, 621 F.3d 1102 (9th Cir. 2010) — software under a restrictive licence is licensed, not sold.
- *Bowers v. Baystate Technologies, Inc.*, 320 F.3d 1317 (Fed. Cir. 2003) — a contract term can waive what fair use would otherwise allow; read the terms, not just the statute.
- *Van Buren v. United States*, 593 U.S. 374 (2021) — breaking terms of use alone is generally not "exceeding authorized access" under the CFAA.
- 17 U.S.C. §1201(a)(3)(A) — the definition of circumvention ("without the authority of the copyright owner").
- 17 U.S.C. §1201(f) — reverse engineering for interoperability of an independently created program.

---

## Snapshots

`legal/snapshots/2026-10-27/` — Google ToS, Google Play terms, Google Pay terms (non-US), Google Pay/Google Payments Additional Terms (U.S.), Google Wallet User Policy, Google Voice Personal terms, Google Voice Additional Terms, Play Integrity improvements, Pixel binary transparency, Trusty (AOSP), Chase UK app licence, Chase US trusted-device page, GrapheneOS attestation compatibility guide, GrapheneOS forum thread on Chase. URLs and BLAKE3 hashes in its `MANIFEST`.
