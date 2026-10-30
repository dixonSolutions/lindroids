<script lang="ts">
	import { Button } from '#lib/components/ui/button/index.js';
	import Copy from '@lucide/svelte/icons/copy';
	import Check from '@lucide/svelte/icons/check';

	let { text, prompt = '$' }: { text: string; prompt?: string } = $props();
	let copied = $state(false);

	async function copy() {
		try {
			await navigator.clipboard.writeText(text);
			copied = true;
			setTimeout(() => (copied = false), 1500);
		} catch {
			// No clipboard (an insecure context): the text is still there to select.
		}
	}
</script>

<div class="flex items-center gap-2 rounded-lg border bg-muted/60 py-1.5 pr-1.5 pl-3 font-mono text-sm">
	<span class="text-muted-foreground select-none">{prompt}</span>
	<code class="min-w-0 flex-1 overflow-x-auto whitespace-nowrap">{text}</code>
	<Button variant="ghost" size="icon-sm" aria-label="Copy" onclick={copy}>
		{#if copied}<Check />{:else}<Copy />{/if}
	</Button>
</div>
