<script lang="ts">
	import { Button } from '#lib/components/ui/button/index.js';
	import Copy from '@lucide/svelte/icons/copy';
	import Check from '@lucide/svelte/icons/check';

	// A shell snippet: one or more lines, wrapped rather than scrolled, lightly coloured, with a
	// copy button. Comment lines (#) are shown but not copied.
	let { text }: { text: string } = $props();
	let copied = $state(false);

	type Token = { s: string; k: 'cmd' | 'flag' | 'arg' | 'url' | 'op' | 'sp' | 'comment' };
	const OPS = new Set(['|', '||', '&&', ';', '>', '>>', '\\']);
	// Words after which the next word is still a command (`sudo apk add`).
	const PREFIX = new Set(['sudo', 'doas']);

	const lines = $derived.by(() => {
		let continued = false;
		return text.split('\n').map((line): Token[] => {
			if (line.trimStart().startsWith('#')) return [{ s: line, k: 'comment' }];
			let cmd = !continued;
			continued = line.trimEnd().endsWith('\\');
			return line.split(/(\s+)/).filter(Boolean).map((s): Token => {
				if (/^\s+$/.test(s)) return { s, k: 'sp' };
				if (OPS.has(s)) {
					cmd = s !== '>' && s !== '>>' && s !== '\\';
					return { s, k: 'op' };
				}
				if (cmd) {
					cmd = PREFIX.has(s);
					return { s, k: 'cmd' };
				}
				if (s.startsWith('-')) return { s, k: 'flag' };
				if (/^(https?:|\/|\$|\[)/.test(s)) return { s, k: 'url' };
				return { s, k: 'arg' };
			});
		});
	});

	const plain = $derived(
		text
			.split('\n')
			.filter((l) => !l.trimStart().startsWith('#'))
			.join('\n')
	);

	async function copy() {
		try {
			await navigator.clipboard.writeText(plain);
			copied = true;
			setTimeout(() => (copied = false), 1500);
		} catch {
			// No clipboard (an insecure context): the text is still there to select.
		}
	}

	const colour: Record<Token['k'], string> = {
		cmd: 'text-amber-700 dark:text-amber-200',
		flag: 'text-sky-700 dark:text-sky-300',
		url: 'text-orange-700 dark:text-orange-300',
		arg: 'text-foreground',
		op: 'text-muted-foreground',
		sp: '',
		comment: 'text-muted-foreground italic'
	};
</script>

<div class="relative rounded-xl border bg-muted/50 py-3.5 pr-12 pl-4">
	<pre
		class="m-0 font-mono text-sm leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere]"><code
			>{#each lines as line, i (i)}{#each line as t, j (j)}<span class={colour[t.k]}>{t.s}</span
					>{/each}{#if i < lines.length - 1}{'\n'}{/if}{/each}</code
		></pre>
	<Button
		variant="ghost"
		size="icon-sm"
		class="absolute top-2 right-2 text-muted-foreground"
		aria-label="Copy"
		title="Copy"
		onclick={copy}
	>
		{#if copied}<Check />{:else}<Copy />{/if}
	</Button>
</div>
