<script lang="ts">
	import './layout.css';
	import { ModeWatcher } from 'mode-watcher';
	
	import { SITE, base } from '#lib/site.js';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
	const description =
		'A package for your Linux environments on Android (Termux, proot distros, Flatpak): their GUI programs open as Android windows, and each one can become an Android app of its own, built on the phone.';
</script>

<svelte:head>
	<title>Lindroids: Linux apps on Android, each one an app of its own</title>
	<meta name="description" content={description} />
	<link rel="icon" href="{base}/media/icon.svg" type="image/svg+xml" />
	<link rel="apple-touch-icon" href="{base}/media/icon-512.png" />
	<meta property="og:title" content="Lindroids" />
	<meta property="og:description" content={description} />
	<meta property="og:image" content="{SITE}/media/social-preview.png" />
	<meta name="twitter:card" content="summary_large_image" />
</svelte:head>

<ModeWatcher themeColors={{ dark: '#141a24', light: '#ffffff' }} />
{@render children()}
