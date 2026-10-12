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

/** Resolve installed resources; Rust owns project/index state and subscriptions. */
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
	let stop: Promise<void> | undefined
	const cleanup = () => {
		if (!stop) {
			stop = application.stop()
		}
		return stop
	}
	hostLifecycle.register(cleanup)
	try {
		if (consoleMode) startServerConsole(application, consoleMode)
		await application.initialize(true)
		hostLifecycle.checkRunning()
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
