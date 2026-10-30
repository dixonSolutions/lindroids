// Where the project lives. The key is packaging/repo/lindroids.gpg (copy-media.js does not check
// it; build-repo.sh refuses to sign with any other).
export const REPO = 'https://github.com/dixonSolutions/lindroids';
export const SITE = 'https://dixonsolutions.github.io/lindroids';
// The current invite (never expires, lands in #general). Share <site>/discord (src/routes/discord),
// which forwards here: a new invite then changes only this line.
export const DISCORD = 'https://discord.gg/gBbW9mNr9P';
// The docs (Mintlify, built from docs.json and docs/ in the repository). Share <site>/docs
// (src/routes/docs), which forwards here, so a custom domain later changes only this line.
export const DOCS = 'https://dixon-solutions.mintlify.site';
export const KEY_FPR = 'CF71814EDB63C911B430479C6FFD91C90F43BF7B';
// Termux: the Linux environment Lindroids runs in (proot distros and Flatpak run inside it).
export const TERMUX_PLAY = 'https://play.google.com/store/apps/details?id=com.termux';
export const TERMUX_FDROID = 'https://f-droid.org/packages/com.termux/';
export const TERMUX_GITHUB = 'https://github.com/termux/termux-app/releases';
export const INSTALL = `curl -fsSL ${SITE}/install.sh | sh`;

import { resolve } from '$app/paths';
// The site's base path ('' in `npm run dev`, /lindroids on GitHub Pages), for static files and
// the repositories next to them.
export const base = resolve('/').replace(/\/$/, '');
