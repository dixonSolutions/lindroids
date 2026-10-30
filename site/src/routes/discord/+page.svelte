<script lang="ts">
	import { onMount } from 'svelte';
	import { DISCORD } from '#lib/site.js';

	// <site>/discord: the address to share. It forwards to the current invite, so a new invite
	// changes one line (DISCORD in src/lib/site.ts), not every link already out there.
	onMount(() => location.replace(DISCORD));
</script>

<svelte:head>
	<title>Lindroids on Discord</title>
	<meta http-equiv="refresh" content="0; url={DISCORD}" />
	<link rel="canonical" href={DISCORD} />
</svelte:head>

<main class="grid min-h-svh place-items-center p-6 text-center">
	<p>Taking you to the Lindroids Discord… <a class="underline" href={DISCORD}>Join the server</a></p>
</main>
