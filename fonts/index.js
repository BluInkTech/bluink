// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import pkg from './package.json' with { type: 'json' }

export { fonts, fontStylesheet } from './catalogue.js'
export const version = pkg.version

export function fontDirectory(format) {
	if (format !== 'web' && format !== 'pdf')
		throw new Error(`Unsupported font format: ${format}`)
	const directory = fileURLToPath(
		new URL(`./assets/${format}/`, import.meta.url),
	)
	if (!existsSync(directory))
		throw new Error(
			'@bluink/fonts assets are missing. Run npm run build --workspace @bluink/fonts.',
		)
	return directory
}
