---
name: people-not-devices-split
description: "Owner agreed (2026-11-10) to split people from devices: a contact is a person/identity with a fleet of device keys; devices are named from their own key; \"you\" is an identity, not sibling contacts; shared model in fgtw/photon_core"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-10T23:03:04.230Z
---

Owner, 2026-11-10, after Photon's departure page named the Pro "JewelryView" (a sibling contact's handle-hash pseudonym) instead of its device name "ProsperityVisionary": asked "Should it be split like that?" and approved ("please") the proposal sent to the Photon session (photon-b9).

**The model:**
- An identity (a person: contacts, plus "you") has a published name or pseudonym and a fleet: the device keys from its chain's fold.
- A device has its key, a name (renamed `fleet.name.<pk>`, else `device_name_default(pk, seed)`) and fleet state.
- Messaging a person fans out to their fleet. Syncing your own devices is fleet work.
- No `is_sibling` contacts.

**How to apply:**
- Name a device only from its key, never from a contact.
- The shared types belong in fgtw / photon_core, so ferros and Photon use one model.
- ferros already keeps the fleet as member keys and will adopt the shared types.
- Photon's side is a planned piece of work, not a quick fix.

Related: [[feedback-photon-is-the-full-app]], [[feedback-fgtw-devices-first]].
