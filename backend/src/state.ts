// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import path from 'node:path'
import { fontDirectory, version as fontsVersion } from '@bluink/fonts'
import { webDirectory } from '@bluink/web'
import pkg from '../package.json' with { type: 'json' }
import type { BluinkApplication } from '../runtime/index.js'
import { nativeAddon, publicationRuntime } from './native/addon.js'
import { startServerConsole } from './native/console.js'
import { hostLifecycle } from './native/lifecycle.js'
import { subscribeProject } from './native/watcher.js'

/** Installed resources and OS subscriptions; project/index state stays in Rust. */
export async function createAppState(consoleMode?: 'auto' | 'plain'): Promise<{
	application: BluinkApplication
}> {
	hostLifecycle.checkRunning()
	const cwd = process.cwd()
	const runtime = publicationRuntime()
	const cacheLocation = process.env['BLUINK_CACHE_LOCATION']
	const application = new (nativeAddon().BluinkApplication)({
		projectRoot: path.resolve(process.env['BLUINK_PROJECT_DIR'] ?? cwd),
		...(cacheLocation && { cacheLocation }),
		publicDir: webDirectory(pkg.version, fontsVersion),
		dataDir: path.join(cwd, 'data'),
		webFontFolder: fontDirectory('web'),
		pdfFontFolder: runtime.fontFolder,
		typstBinary: runtime.typstBinary,
	})
	let stopSubscription = () => {}
	let stop: Promise<void> | undefined
	const cleanup = () => {
		if (!stop) {
			stop = (async () => {
				try {
					stopSubscription()
				} finally {
					await application.stop()
				}
			})()
		}
		return stop
	}
	hostLifecycle.register(cleanup)
	try {
		if (consoleMode) startServerConsole(application, consoleMode)
		const opened = JSON.parse(await application.initialize(true)) as {
			watchRoots: Array<{ path: string; prefix: string }>
		}
		hostLifecycle.checkRunning()
		stopSubscription = subscribeProject(application, opened.watchRoots)
		return { application }
	} catch (error) {
		try {
			await cleanup()
		} catch (cleanupError) {
			throw new AggregateError(
				[error, cleanupError],
				'Application startup and cleanup failed',
			)
		}
		throw error
	}
}
