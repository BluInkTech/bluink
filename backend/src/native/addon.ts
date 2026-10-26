// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { createRequire } from 'node:module'
import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { fontDirectory } from '@bluink/fonts'
import { typstBinary } from '@bluink/typst'
import { setBackendDiagnostics } from '../logger.js'

type NativeAddon = typeof import('../../runtime/index.js')

const require = createRequire(import.meta.url)
let cachedAddon: NativeAddon | undefined

/** Process shutdown only: do not load an addon merely to close diagnostics. */
export async function shutdownNativeDiagnostics(): Promise<void> {
	await cachedAddon?.shutdownNativeDiagnostics()
}

/** Source and compiled modules share resources at the backend package root. */
export function backendPackageRoot(): string {
	const directory = path.dirname(fileURLToPath(import.meta.url))
	const root = ['../..', '../../..']
		.map((relative) => path.resolve(directory, relative))
		.find((candidate) => existsSync(path.join(candidate, 'runtime/index.cjs')))
	if (!root) throw new Error('runtime/index.cjs is missing from this package')
	return root
}

/** Resolve packaged resources beside this backend package in source or dist. */
export function publicationRuntime(): {
	typstBinary: string
	fontFolder: string
	docxFontFolder: string
} {
	return {
		typstBinary: typstBinary(),
		fontFolder: fontDirectory('pdf'),
		docxFontFolder: fontDirectory('docx'),
	}
}

export function nativeAddon(): NativeAddon {
	if (cachedAddon) return cachedAddon
	let addon: NativeAddon
	try {
		addon = require(
			path.join(backendPackageRoot(), 'runtime/index.cjs'),
		) as NativeAddon
	} catch (error) {
		const detail = error instanceof Error ? error.message : String(error)
		throw new Error(
			`The native backend could not be loaded for ${process.platform}/${process.arch}. Reinstall Bluink for this platform; for a source checkout, run npm run build:native. (` +
				detail +
				')',
		)
	}
	try {
		addon.configureNativeDiagnostics()
		setBackendDiagnostics((event) =>
			addon.recordNativeDiagnostic(JSON.stringify(event)),
		)
	} catch (error) {
		const detail = error instanceof Error ? error.message : String(error)
		process.stderr.write(`Native diagnostics are unavailable: ${detail}\n`)
	}
	cachedAddon = addon
	return addon
}

/** Resolve UI-authored bootstrap data outside the native binary. */
export function createApplication(
	options: Omit<
		import('../../runtime/index.js').ApplicationOptions,
		'workspaceTemplatePath'
	>,
) {
	return new (nativeAddon().BluinkApplication)({
		...options,
		workspaceTemplatePath: path.join(
			backendPackageRoot(),
			'vendor/workspace-starter.json',
		),
	})
}
