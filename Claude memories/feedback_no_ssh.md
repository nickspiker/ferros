---
name: Don't SSH into other machines
description: Never SSH into remote machines unless user explicitly says to — do exactly what's asked
type: feedback
---

When user says "git pull", run git pull locally. Don't SSH into other machines.

**Why:** User asked for git pull, I tried to SSH into the MacBook. User was furious: "Did I say SSH into another machine or did I say git pull?"

**How to apply:** Always execute commands on the local machine unless the user explicitly says to run something on a remote host. Never assume which machine a command should run on.
