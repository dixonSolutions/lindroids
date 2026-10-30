import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// A static site, served by GitHub Pages under /<repo> (BASE_PATH, set by site.yml).
			adapter: adapter({ strict: true }),
			paths: { base: (process.env.BASE_PATH ?? '') as '' | `/${string}` },
			prerender: {
				// The signing keys are published next to the site by the release (build-repo.sh).
				handleHttpError: ({ path, message }) => {
					if (/\/lindroids\.(gpg|asc|rsa\.pub)$/.test(path)) return;
					throw new Error(message);
				}
			}
		})
	]
});
