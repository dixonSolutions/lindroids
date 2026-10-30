<script lang="ts">
	import { mode, setMode, userPrefersMode } from 'mode-watcher';
	import { Button } from '#lib/components/ui/button/index.js';
	import Sun from '@lucide/svelte/icons/sun';
	import Moon from '@lucide/svelte/icons/moon';
	import Monitor from '@lucide/svelte/icons/monitor';

	// System, then light, then dark: the site follows the device until told otherwise.
	const next = { system: 'light', light: 'dark', dark: 'system' } as const;
	const label = $derived(
		userPrefersMode.current === 'system'
			? `Theme: as the device (${mode.current})`
			: `Theme: ${userPrefersMode.current}`
	);
</script>

<Button
	variant="ghost"
	size="icon"
	title={label}
	aria-label={label}
	onclick={() => setMode(next[userPrefersMode.current])}
>
	{#if userPrefersMode.current === 'system'}
		<Monitor />
	{:else if userPrefersMode.current === 'light'}
		<Sun />
	{:else}
		<Moon />
	{/if}
</Button>
