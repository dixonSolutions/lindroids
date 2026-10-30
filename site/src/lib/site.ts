// Where the project lives. The key is packaging/repo/lindroids.gpg (copy-media.js does not check
// it; build-repo.sh refuses to sign with any other).
export const REPO = 'https://github.com/dixonSolutions/lindroids';
export const SITE = 'https://dixonsolutions.github.io/lindroids';
export const DISCORD = 'https://discord.gg/aWQwxV4wm';
export const KEY_FPR = 'CF71814EDB63C911B430479C6FFD91C90F43BF7B';
export const INSTALL = `curl -fsSL ${SITE}/install.sh | sh`;

import { resolve } from '$app/paths';
// The site's base path ('' in `npm run dev`, /lindroids on GitHub Pages), for static files and
// the repositories next to them.
export const base = resolve('/').replace(/\/$/, '');
