<script lang="ts">
	import { onMount } from 'svelte';
	
	import { Badge } from '#lib/components/ui/badge/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Card from '#lib/components/ui/card/index.js';
	import * as Tabs from '#lib/components/ui/tabs/index.js';
	import { Separator } from '#lib/components/ui/separator/index.js';
	import ThemeToggle from '#lib/components/theme-toggle.svelte';
	import Command from '#lib/components/command.svelte';
	import { REPO, SITE, DISCORD, KEY_FPR, INSTALL, base } from '#lib/site.js';
	import Package from '@lucide/svelte/icons/package';
	import Boxes from '@lucide/svelte/icons/boxes';
	import Hammer from '@lucide/svelte/icons/hammer';
	import AppWindow from '@lucide/svelte/icons/app-window';
	import Smartphone from '@lucide/svelte/icons/smartphone';
	import Layers from '@lucide/svelte/icons/layers';
	import Sparkles from '@lucide/svelte/icons/sparkles';
	import Github from '#lib/components/github-icon.svelte';
	import Discord from '#lib/components/discord-icon.svelte';

	// The release workflow writes VERSION next to the repositories; absent in `npm run dev`.
	let version = $state('');
	onMount(async () => {
		try {
			const r = await fetch(`${base}/VERSION`);
			if (r.ok) version = (await r.text()).trim();
		} catch {
			// No VERSION: the badge stays hidden.
		}
	});

	const is = [
		{
			icon: Package,
			title: 'A package, not an app',
			text: 'You install Lindroids inside the Linux environment you already use: with pkg in Termux, or with the package manager of a proot distro. There is no Lindroids app in a store, and nothing to install on Android first.'
		},
		{
			icon: Boxes,
			title: 'Not a container',
			text: 'Lindroids does not run Linux itself. Termux, proot-distro and Flatpak keep running your programs, with their files and packages. Lindroids gives those programs windows on Android, and Android features through the desktop portals.'
		},
		{
			icon: Hammer,
			title: 'It builds apps for your programs',
			text: 'lindroids export fills the window app’s template with a program’s name and icon and signs it on the phone, in milliseconds. The app belongs to that program: its icon starts it, Recents shows it.'
		}
	];

	const features = [
		{
			icon: AppWindow,
			title: 'A window of its own, now',
			text: 'lindroids run opens a program in a window with its name, closed when the program ends or Termux stops.'
		},
		{
			icon: Smartphone,
			title: 'An app of its own, for good',
			text: 'lindroids export: tap Install, and the program is in the app list with its icon, also when Termux is not running.'
		},
		{
			icon: Layers,
			title: 'Any Linux program with a window',
			text: 'Termux packages, programs in a proot distro, Flatpaks from Flathub (no root), X11 programs.'
		},
		{
			icon: Sparkles,
			title: 'Android underneath',
			text: 'Dark mode and accent colour, Android’s file picker, Open with, notifications, clipboard, sound and microphone, touch, pen and keyboards.'
		}
	];

	const shots = [
		{ src: 'app-calculator.png', alt: 'GNOME Calculator in its own app, full screen' },
		{ src: 'app-recents.png', alt: 'Recents shows Calculator with its own icon' },
		{ src: 'app-vscodium.png', alt: 'VSCodium from Flathub in its own app' }
	];

	const repos = {
		apt: `# Termux: $PREFIX/etc/apt/sources.list.d/lindroids.list
deb [signed-by=$PREFIX/etc/apt/keyrings/lindroids.gpg] ${SITE} termux main
# Debian / Ubuntu: /etc/apt/sources.list.d/lindroids.list
deb [signed-by=/etc/apt/keyrings/lindroids.gpg] ${SITE} stable main`,
		pacman: `# after: pacman-key --add lindroids.asc && pacman-key --lsign-key ${KEY_FPR}
# Termux with pacman: $PREFIX/etc/pacman.conf
[lindroids]
Server = ${SITE}/pacman/termux/$arch
# Arch Linux and derivatives: /etc/pacman.conf
[lindroids]
Server = ${SITE}/pacman/arch/$arch`,
		rpm: `# Fedora, RHEL, Rocky, Alma, CentOS
sudo curl -o /etc/yum.repos.d/lindroids.repo ${SITE}/rpm/lindroids.repo
# openSUSE
sudo zypper addrepo ${SITE}/rpm/lindroids.repo`,
		apk: `# Alpine, postmarketOS, Chimera: the key lindroids.rsa.pub into /etc/apk/keys/, then
echo ${SITE}/apk >> /etc/apk/repositories
apk update && apk add lindroids`,
		xbps: `# Void Linux: imports the same RSA key on the first sync
echo repository=${SITE}/xbps > /etc/xbps.d/lindroids.conf
xbps-install -S lindroids`
	};
</script>

<header class="sticky top-0 z-10 border-b bg-background/80 backdrop-blur">
	<div class="mx-auto flex h-14 max-w-5xl items-center gap-3 px-4">
		<a href="{base}/" class="flex items-center gap-2 font-semibold">
			<img src="{base}/media/icon.svg" alt="" class="size-7 rounded-md" />
			Lindroids
		</a>
		{#if version}<Badge variant="secondary" class="hidden sm:inline-flex">v{version}</Badge>{/if}
		<nav class="ml-auto flex items-center gap-1">
			<Button variant="ghost" size="sm" href="#install" class="hidden sm:inline-flex">Install</Button>
			<Button variant="ghost" size="sm" href="{REPO}#readme" class="hidden sm:inline-flex">Docs</Button>
			<Button variant="ghost" size="icon" href={DISCORD} aria-label="Discord" title="Discord"><Discord /></Button>
			<Button variant="ghost" size="icon" href={REPO} aria-label="GitHub"><Github /></Button>
			<ThemeToggle />
		</nav>
	</div>
</header>

<main class="mx-auto max-w-5xl px-4">
	<section class="grid items-center gap-10 py-12 md:grid-cols-[1fr_auto] md:py-20">
		<div class="min-w-0 space-y-6">
			<img src="{base}/media/icon.svg" alt="Lindroids" class="size-20 rounded-2xl shadow-lg" />
			<h1 class="text-4xl font-bold tracking-tight text-balance sm:text-5xl">
				Linux apps on Android, <span class="text-primary">each one an app of its own.</span>
			</h1>
			<p class="max-w-xl text-lg text-pretty text-muted-foreground">
				A package for the Linux environments you already use on Android (Termux, proot distros,
				Flatpak). Their GUI programs open as Android windows, and <code class="font-mono text-foreground">lindroids export</code>
				turns one into a real Android app, built on the phone. No root, no browser.
			</p>
			<div class="max-w-xl space-y-2">
				<Command prompt="~" text={INSTALL} />
				<p class="text-sm text-muted-foreground">In Termux (F-Droid, GitHub or Google Play), or in a proot distro.</p>
			</div>
			<div class="flex flex-wrap gap-2">
				<Button href="#install">Get started</Button>
				<Button variant="outline" href={REPO}><Github /> GitHub</Button>
				<Button variant="outline" href={DISCORD}><Discord /> Discord</Button>
			</div>
		</div>
		<figure class="mx-auto w-64 md:w-72">
			<video
				class="aspect-[5/8] w-full rounded-[1.75rem] border-8 border-foreground/90 bg-black shadow-2xl"
				src="{base}/media/demos/export-and-run.mp4"
				poster="{base}/media/screenshots/app-calculator.png"
				autoplay
				muted
				loop
				playsinline
				aria-label="lindroids run, then lindroids export, on a Galaxy Tab"
			></video>
			<figcaption class="mt-3 text-center text-sm text-muted-foreground">
				45 seconds on a Galaxy Tab (Android 16)
			</figcaption>
		</figure>
	</section>

	<section class="py-10">
		<h2 class="mb-6 text-2xl font-semibold tracking-tight">What Lindroids is, and is not</h2>
		<div class="grid gap-4 md:grid-cols-3">
			{#each is as item (item.title)}
				<Card.Root>
					<Card.Header>
						<item.icon class="mb-2 size-6 text-primary" />
						<Card.Title>{item.title}</Card.Title>
					</Card.Header>
					<Card.Content class="text-sm text-muted-foreground">{item.text}</Card.Content>
				</Card.Root>
			{/each}
		</div>
	</section>

	<section class="py-10">
		<h2 class="mb-6 text-2xl font-semibold tracking-tight">Two commands</h2>
		<div class="grid gap-4 md:grid-cols-2">
			<Command text="lindroids run org.gnome.Calculator" />
			<Command text="lindroids export org.gnome.Calculator" />
		</div>
		<div class="mt-6 grid gap-x-8 gap-y-6 sm:grid-cols-2">
			{#each features as f (f.title)}
				<div class="flex gap-3">
					<f.icon class="mt-0.5 size-5 shrink-0 text-primary" />
					<div>
						<h3 class="font-medium">{f.title}</h3>
						<p class="text-sm text-muted-foreground">{f.text}</p>
					</div>
				</div>
			{/each}
		</div>
	</section>

	<section class="py-10">
		<h2 class="mb-6 text-2xl font-semibold tracking-tight">On a real Android device</h2>
		<div class="grid grid-cols-3 gap-3 sm:gap-6">
			{#each shots as s (s.src)}
				<figure>
					<img
						src="{base}/media/screenshots/{s.src}"
						alt={s.alt}
						loading="lazy"
						class="aspect-[5/8] w-full rounded-xl border object-cover object-top shadow"
					/>
					<figcaption class="mt-2 text-xs text-muted-foreground sm:text-sm">{s.alt}</figcaption>
				</figure>
			{/each}
		</div>
	</section>

	<section id="install" class="min-w-0 scroll-mt-20 py-10">
		<h2 class="mb-2 text-2xl font-semibold tracking-tight">Install</h2>
		<p class="mb-6 max-w-2xl text-muted-foreground">
			<code class="font-mono text-foreground">install.sh</code> adds the Lindroids repository to the
			package manager, so the usual upgrade keeps it current, and offers to set up Flatpak. The first
			<code class="font-mono text-foreground">lindroids export</code> asks Android once to allow installs.
		</p>
		<Command prompt="~" text={INSTALL} />
		<h3 class="mt-10 mb-3 font-medium">Or add a repository by hand</h3>
		<Tabs.Root value="apt" class="min-w-0">
			<Tabs.List>
				<Tabs.Trigger value="apt">APT</Tabs.Trigger>
				<Tabs.Trigger value="pacman">pacman</Tabs.Trigger>
				<Tabs.Trigger value="rpm">RPM</Tabs.Trigger>
				<Tabs.Trigger value="apk">apk</Tabs.Trigger>
				<Tabs.Trigger value="xbps">xbps</Tabs.Trigger>
			</Tabs.List>
			{#each Object.entries(repos) as [id, text] (id)}
				<Tabs.Content value={id} class="min-w-0">
					<pre class="overflow-x-auto rounded-lg border bg-muted/60 p-4 font-mono text-sm">{text}</pre>
				</Tabs.Content>
			{/each}
		</Tabs.Root>
		<p class="mt-3 text-sm text-muted-foreground">
			Signing key <a class="underline" href="{base}/lindroids.gpg">lindroids.gpg</a>
			(<a class="underline" href="{base}/lindroids.asc">armored</a>), fingerprint
			<code class="font-mono break-all">{KEY_FPR}</code>; apk and xbps:
			<a class="underline" href="{base}/lindroids.rsa.pub">lindroids.rsa.pub</a>.
		</p>
	</section>

	<section class="py-10">
		<Card.Root class="items-center gap-4 text-center sm:flex-row sm:text-left">
			<Card.Header class="flex-1">
				<Card.Title class="text-xl">Join the community</Card.Title>
				<Card.Description>
					Questions, programs that work (or do not), ideas and help: talk to the people who
					build and use Lindroids on Discord.
				</Card.Description>
			</Card.Header>
			<Card.Content class="flex flex-wrap justify-center gap-2 sm:justify-end">
				<Button href={DISCORD}><Discord /> Join the Discord</Button>
				<Button variant="outline" href="{REPO}/issues"><Github /> Issues</Button>
			</Card.Content>
		</Card.Root>
	</section>
</main>

<Separator class="mt-10" />
<footer class="mx-auto flex max-w-5xl flex-wrap items-center gap-x-6 gap-y-2 px-4 py-8 text-sm text-muted-foreground">
	<span>MIT or Apache-2.0</span>
	<a class="hover:text-foreground" href="{REPO}/releases">Releases</a>
	<a class="hover:text-foreground" href="{REPO}/blob/main/docs/APPS.md">How the apps work</a>
	<a class="hover:text-foreground" href={DISCORD}>Discord</a>
</footer>
