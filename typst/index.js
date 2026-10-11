// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import sources from './sources.json' with { type: 'json' }

export const upstreamVersion = sources.version
export const targets = {
	'linux/x64': 'linux-x64/typst',
	'win32/x64': 'windows-x64/typst.exe',
	'darwin/arm64': 'macos-arm64/typst',
}

/** Resolve resources relative to this package, independent of the user's cwd. */
export function typstBinary(platform = process.platform, arch = process.arch) {
	const relative = targets[`${platform}/${arch}`]
	if (!relative)
		throw new Error(`Typst is not packaged for ${platform}/${arch}.`)
	const binary = fileURLToPath(new URL(`./assets/${relative}`, import.meta.url))
	if (!existsSync(binary))
		throw new Error(
			'@bluink/typst assets are missing. Run npm run build --workspace @bluink/typst.',
		)
	return binary
}
