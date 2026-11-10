# web

The source of ferros.org: one self-contained page, `index.html`, wired the way passless.org is (no scripts, no cookies, no trackers, no fonts fetched, relative units only), and `robots.txt`, with `wrangler.toml` (the Cloudflare Pages wiring: from this directory, `wrangler pages deploy --branch main`; the Pages project is `ferros`, production branch `main`, ferros.org attached; a deploy without `--branch main` lands as a preview on the current git branch and does not reach ferros.org) and `logo.svg` (the Illustrator logo, half the column wide above the title since 2026-11-10).

Rewritten on 2026-11-02 to say what is true: what runs on the Pixel 8 Pro (and since November the Pixel 8a) today, what is designed, what is still a bet, and what ferros will never be. The page before it (ripped from the live site that day, commit 2086daf) described a Redox-based ring-memory kernel and a status board with everything off; git history keeps it. Every claim on the page should point at something in this repository; change the page when the repository changes.

`shots/`: the screens on the page, captured on the device with `tools/snap.sh` (16-bit VSF RGB visuals) and made 8-bit sRGB AV1 images (AVIF, full and half size) by `tools/vsf2web --mask`, with the glass's edge and camera hole as their alpha from the device's own mask (`screencap --mask`) (2026-11-10).
