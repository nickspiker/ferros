---
name: ferros-vocabulary
description: "ferros/Photon words, with the owner's quotes: photon, beam, kist, capsule, stow, puck, the deck, Deck/Back, the hold, items, pandok, GPS. Merged 2026-11-12 from seven term memories"
metadata:
  type: feedback
---

Merged 2026-11-12 from photon_terminology, ferros_kist_term, stow_term, hold_term, puck_term, pandok and vocab_gps_not_gnss. Say these in conversation, docs, UI words, code names and commit messages. Definitions also live in docs/GLOSSARY.md. See also [[feedback_device_not_phone]].

**photon / beam** (owner, 2026-10-15): "We call messages in Photon photons, we also call video calls beams." Send a photon; beam someone. On ferros: a photon first (network + client), then a beam (camera, video, audio).

**app, capsule, kist, process** (owner, 2026-11-07: "kist" is the term for the signed, checked and optimised capsules that ferros runs):
- app: what a person gets; capsule: the developer's signed binary as shipped (ORACLE.md);
- kist: that capsule after this device took it in: signature checked, compiled once on device, sealed into the vault (KERNEL.md "Install, compile, seal, go");
- process: one running instance of a kist; kist circle: a kist's circle on the deck.

**stow** (owner, 2026-11-11: "btw the proper term for installing a kist is stow"): stow / stowed / stowing, never "install" in new words. Old identifiers (SYS_SPAWN "install mode", the bridge's `kist install`, payloads/kists "the installer") get renamed when touched or in a dedicated rename.

**puck** (owner, 2026-11-11: "tides on a puck (is that what we should call kists you can't open but are live like the clock and tide? Android calls them widgets I think?)"; 2026-11-12: "I think pucks distinct from kists being that you can't open a puck and it's basically a widget."): a live circle that is its whole UI, never opened (clock, tides; docs/PUCKS.md). Say puck, never widget; pucks are a separate kind from kists in docs, UI and code.

**the deck** (owner, 2026-11-12: "I didn't particularly like the word home screen anyway. Good call on that."): the home screen is the deck; never "home screen" in new words. Its Settings page listing kists, pucks and later contacts is **Deck** (owner first said "items in settings, not kists", then, to keep "item" for the hold, "Deck it is!"). The helm's two buttons are **Deck** (bottom right, was home: tap backgrounds the kist and shows the deck, swipe in fits the deck) and **Back** (owner, 2026-11-12: "So it's 'Deck' and 'Back'."). Code and vault names still say `map` (payloads/map, `map.place.*`, `map.view`).

**the hold / items** (owner, 2026-11-11: "the hold? Is that the right word?" then "Yes, and I'd say not worry about the snag. it's common to hold a bag and to put shit in the hold."): the item store of docs/STORE.md (cards found by kind, maker, time, words; never a directory tree) is the hold; its contents are items. A kist keeps for you = in the hold under the person's key; keeps for itself = its own domain under the kist's key. "Held call" (a doorbell wait) keeps its meaning. "Item" never names a deck thing.

**pandok** (owner, 2026-11-11: "pandok is the place with the things, mostly kists at the moment but we might sell sex toys on there or cheap flights, who knows! Today, kists, tomorrow, the world!"; 2026-11-12: "the public exchange for items is the pandok, just bought pandok.org last night"; "pandok isn't limited to shit online. could be food delivery, home cleaning, airfare, anything."): the public exchange for kists, pucks, hold items, goods and services; home pandok.org (the directory lives at kists.fgtw.org today; moving it is a step to plan, never a DNS change without asking). One source among several (the checked USB offer `ferros send`, later Photon and device to device), all thru the one on-device stower. Entry format stays general: a kind, who offers it, what it is (ideas, not language-bound text, [[ideas_language_independent]]), how it's fulfilled, metrics many-dimensional, never stars ([[kist_catalog]]). No fees, no ads, no gatekeeper. Don't build non-kist listings until asked.

**GPS, not GNSS** (owner, 2026-11-07: the correct name is GPS, Global Positioning System): most orbit and assistance data reaches a device over the ground, not from the satellites, and the use is positioning and timing; "Global" is the one word GNSS got right. GPS names the positioning radio and its whole system. The code rename is done (payloads/gps, docs/GPS.md, `gps.` vault scope).
