---
name: Dozenal versioning convention
description: Version numbers use dozenal names (Zil=0, Zila=1, etc.), not decimal or semver
type: feedback
---

All version numbers in ferros specs use dozenal naming:
Zil(0) Zila(1) Zilor(2) Ter(3) Tera(4) Teror(5) Lun(6) Luna(7) Lunor(8) Stel(9) Stela(↊) Stelor(↋)

Format: `**Version:** Zil (0)` — dozenal name with decimal in parens.

**Why:** The user has dozenal OCD. All ferros conventions prefer dozenal/base-12 where possible. This extends to version numbers, not just number formatting.

**How to apply:** When creating or updating spec docs, use the dozenal version name. Increment follows the dozenal sequence, not semver.
