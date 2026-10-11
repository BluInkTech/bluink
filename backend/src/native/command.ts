// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import path from 'node:path'
import { fontDirectory } from '@bluink/fonts'
import { nativeAddon, publicationRuntime } from './addon.js'
import { hostLifecycle } from './lifecycle.js'

export type CommandValues = Record<
	string,
	string | boolean | (string | boolean)[] | undefined
>

/** Host responsibilities only: installed resources, arguments and presentation. */
export async function runApplicationCommand(
	command: 'export' | 'publication',
	values: CommandValues,
	positionals: string[],
): Promise<void> {
	hostLifecycle.checkRunning()
	const cwd = process.cwd()
	const project = values['project']
	const runtime = publicationRuntime()
	const application = new (nativeAddon().BluinkApplication)({
		projectRoot: path.resolve(typeof project === 'string' ? project : cwd),
		...(process.env['BLUINK_CACHE_LOCATION'] && {
			cacheLocation: process.env['BLUINK_CACHE_LOCATION'],
		}),
		webFontFolder: fontDirectory('web'),
		pdfFontFolder: runtime.fontFolder,
		typstBinary: runtime.typstBinary,
	})
	let stopped: Promise<void> | undefined
	const stop = () => (stopped ??= application.stop())
	const unregister = hostLifecycle.register(stop)
	try {
		const result = JSON.parse(
			await application.executeCommand(
				command,
				JSON.stringify({ cwd, values, positionals }),
			),
		) as { kind: string; value: unknown }
		process.stdout.write(
			`${result.kind === 'text' ? String(result.value) : JSON.stringify(result.value, null, 2)}\n`,
		)
	} finally {
		try {
			await stop()
		} finally {
			unregister()
		}
	}
}
