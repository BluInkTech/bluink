// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { backendPackageRoot } from './addon.js'

/** Resolve the compiled interface bundled beside the CLI's runtime. */
export function webDirectory(
	expectedVersion: string,
	expectedFontsVersion: string,
	packageRoot = backendPackageRoot(),
): string {
	const root = path.join(packageRoot, 'web')
	const metadataFile = path.join(root, 'release.json')
	const directory = path.join(root, 'assets')
	if (
		!existsSync(metadataFile) ||
		!existsSync(path.join(directory, 'index.html'))
	)
		throw new Error(
			'Bundled frontend assets are missing. Reinstall Bluink; in a development checkout run npm run build:web.',
		)
	const metadata = JSON.parse(readFileSync(metadataFile, 'utf8')) as {
		format: string
		formatVersion: number
		version: string
		fontsVersion: string
	}
	if (metadata.format !== 'bluink.web' || metadata.formatVersion !== 1)
		throw new Error('The bundled frontend release metadata is invalid.')
	if (metadata.version !== expectedVersion)
		throw new Error(
			`Bluink CLI ${expectedVersion} requires frontend ${expectedVersion}; found ${metadata.version}. Reinstall Bluink.`,
		)
	if (metadata.fontsVersion !== expectedFontsVersion)
		throw new Error(
			`The bundled frontend requires @bluink/fonts ${metadata.fontsVersion}; found ${expectedFontsVersion}. Reinstall Bluink.`,
		)
	return directory
}
