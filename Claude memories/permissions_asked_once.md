---
name: permissions-asked-once
description: "a kist's permission is asked once per person, per kist, per permission, ever; stored in the person's identity space, it travels to all their devices; never re-asked until they change it (\"1 means 1\")"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-11T04:03:28.533Z
---

Owner, 2026-11-07: "if lumis asks access to store images in the shared images location, that should only ask once, ever, for all my devices, ever ever, until I change it. 1 means 1."

- **What it is:** a permission is the person's answer to a kist's request, such as a named grant in the common area (docs/VAULT-OUT.md).
- **Where it lives:** in the person's identity-class space, so it syncs with the fleet.
- **What it's keyed by:** the kist's gate identity (developer key and option), not its name.
- **When it is asked again:** never. A reinstall, an install on another device, an update or a device reset all reuse the stored answer. Only the person changing it in settings changes it.

**Why:** the owner's "we can actually count" stance. A device that asks again is lying about what it already knows.

**How to apply:**
- Any permission or consent flow in ferros or Photon checks the person's stored answer first.
- The prompt appears only if no answer exists for that person, kist and permission.
- Never design per-device or per-install prompts.

**Decided 2026-11-08 (owner, on the capability design: "it sounds perfecter! Let's build it!"):**
- A grant is the permission: a kist holds only what it was handed at spawn; wants are declared in the signed capsule, each with one line of reason.
- Every want defaults to OFF; an update that adds a want asks for it (off until answered).
- The person sets their own defaults per right on the permissions page (e.g. every new kist gets internet, or address/contacts); out of the box all off.
- No lying: a denied right is an empty slot, never fake data. Kists run without it (Mahere without GPS; Lumis without the camera has no cameras to pick).
- ~~Kists can never ask; the system asks once at first launch, all unanswered wants on one screen.~~ Replaced 2026-11-10 (owner: "asks once at first launch is not the thing ... maybe I never share my location with anyone so there's no reason for Photon to ever ask me"): the drop that installs a kist (drag its circle onto the home map, then Add or Cancel) asks only what it needs to run at all; every other right is asked at the person's first action that needs it (Photon's location the first time they share it), never before. A kist can put a question only while holding the one-shot ticket the input path hands it with a fresh gesture (the clipboard's rule: no gesture, no capability), so no background or at-launch asking is possible. Still once, ever, per person, per kist, per right.
- A declared right carries its reason as an idea (see [[ideas-language-independent]]) and whether it is needed to run.
- Revoke = kill and respawn without the right. Every kill and every end, for every process, is one shot dead, no cleanup ("just like everything else").
- Settings: per kist, per right (who has GPS), and the defaults.

Related: [[ferros_vocabulary]], [[settings-identity-to-device-only]].
