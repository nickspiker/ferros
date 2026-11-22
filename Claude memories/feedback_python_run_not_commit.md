---
name: feedback-python-run-not-commit
description: Owner 2026-11-22 - the no-Python rule is about commits only: Python may be run and used locally as a tool, never committed (python-guard enforces commits)
metadata:
  type: feedback
---

Owner, 2026-11-22, after two agents were reported for running a no-op `python3 -c 1`: "this whole no python thing, you can run it and use it, just don't commit it."

**The rule:** no Python in any commit of the owner's, anywhere (tools/python-guard refuses it at commit and push, 2026-11-21). Running Python locally as a tool (a quick calculation, a one-off script in a scratchpad) is fine.

**How to apply:** use Python freely for throwaway local work in scratchpads; never put a .py file, a python shebang, a build step running python, or a Command("python...") into a repo. Agent briefs should say "no Python in commits", not "never run python". This narrows AGENT.md's "Not allowed: Python" to code that ships.

Related: [[feedback_one_line]].
