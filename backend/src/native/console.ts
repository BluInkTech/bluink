// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { spawn } from 'node:child_process'
import type { BluinkApplication, BluinkConsole } from '../../runtime/index.js'
import pkg from '../../package.json' with { type: 'json' }
import logger, { setServerConsoleActive } from '../logger.js'
import { nativeAddon } from './addon.js'
import { hostLifecycle } from './lifecycle.js'

/** RSS is host metadata; logs, status, counters and rendering remain in Rust. */
export function startRssReporting(
	console: Pick<BluinkConsole, 'updateRss'>,
	sample: () => number = () => process.memoryUsage.rss(),
): () => void {
	const update = () => {
		try {
			console.updateRss(sample())
		} catch {
			console.updateRss(null)
		}
	}
	update()
	const interval = setInterval(update, 5_000)
	interval.unref()
	return () => clearInterval(interval)
}

/** OS launcher arguments are passed directly; URLs are never shell commands. */
export function browserCommand(
	url: string,
	platform: NodeJS.Platform = process.platform,
): { command: string; args: string[] } {
	const address = new URL(url)
	if (!['http:', 'https:'].includes(address.protocol))
		throw new Error('The browser address must use HTTP or HTTPS.')
	switch (platform) {
		case 'darwin':
			return { command: 'open', args: [address.href] }
		case 'win32':
			return {
				command: 'rundll32',
				args: ['url.dll,FileProtocolHandler', address.href],
			}
		default:
			return { command: 'xdg-open', args: [address.href] }
	}
}

async function openBrowser(url: string): Promise<void> {
	const { command, args } = browserCommand(url)
	await new Promise<void>((resolve, reject) => {
		const child = spawn(command, args, { stdio: 'ignore', shell: false })
		child.once('error', reject)
		child.once('exit', (code) =>
			code === 0 ? resolve() : reject(new Error('Browser launcher failed.')),
		)
		child.unref()
	})
}

function reportFailure(name: string): void {
	logger.diagnostic({
		timestampMs: Date.now(),
		level: 'error',
		target: 'bluink.host',
		name,
		message:
			name === 'browser_open_failed'
				? 'Could not open the browser; use the displayed address'
				: 'Console action failed; stopping Bluink',
	})
}

async function consumeActions(console: BluinkConsole): Promise<void> {
	let browser: Promise<void> | undefined
	while (!hostLifecycle.stopping) {
		const action = await console.nextAction()
		if (hostLifecycle.stopping) return
		if (!action) throw new Error('Console stopped unexpectedly.')
		if (action.kind === 'shutdown') {
			await hostLifecycle.stop()
			return
		}
		if (action.kind === 'openBrowser' && action.url && !browser) {
			// A slow OS launcher must not prevent Quit from being handled.
			browser = openBrowser(action.url)
				.catch(() => reportFailure('browser_open_failed'))
				.finally(() => {
					browser = undefined
				})
		}
	}
}

/** Attach before initialization; shutdown is finalized after diagnostic drain. */
export function startServerConsole(
	application: BluinkApplication,
	mode: 'auto' | 'plain',
): void {
	const console = new (nativeAddon().BluinkConsole)({
		cliVersion: pkg.version,
		nodeVersion: process.version,
		mode,
	})
	setServerConsoleActive(true)
	const stopRss = startRssReporting(console)
	hostLifecycle.register(async () => stopRss())
	hostLifecycle.registerFinalizer(async () => {
		try {
			await console.stop()
		} finally {
			setServerConsoleActive(false)
		}
	})
	console.attachApplication(application)
	void consumeActions(console).catch(() => {
		reportFailure('console_action_failed')
		process.exitCode = 1
		void hostLifecycle.stop().catch(() => {
			process.stderr.write('Backend cleanup failed\n')
		})
	})
}
