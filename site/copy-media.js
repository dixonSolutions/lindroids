// The site shows the repository's own media (../media); copied into static/media, not committed twice.
import { cpSync, mkdirSync, rmSync } from 'node:fs';

const out = new URL('./static/media/', import.meta.url);
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
for (const f of ['icon.svg', 'icon-512.png', 'social-preview.png', 'demos/export-and-run.mp4', 'screenshots']) {
	cpSync(new URL(`../media/${f}`, import.meta.url), new URL(f, out), { recursive: true });
}
