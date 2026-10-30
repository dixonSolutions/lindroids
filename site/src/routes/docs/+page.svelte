<script lang="ts">
	import { onMount } from 'svelte';
	import { DOCS } from '#lib/site.js';

	// <site>/docs: the address to share. It forwards to the docs site, so moving it (a custom
	// domain) changes one line (DOCS in src/lib/site.ts), not every link already out there.
	onMount(() => location.replace(DOCS));
</script>

<svelte:head>
	<title>Lindroids docs</title>
	<meta http-equiv="refresh" content="0; url={DOCS}" />
	<link rel="canonical" href={DOCS} />
</svelte:head>

<main class="grid min-h-svh place-items-center p-6 text-center">
	<p>Taking you to the Lindroids docs… <a class="underline" href={DOCS}>Open the docs</a></p>
</main>
