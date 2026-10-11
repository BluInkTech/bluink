// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { lstat, readFile, readdir } from 'node:fs/promises'
import path from 'node:path'
import { createHash } from 'node:crypto'

const forbiddenPath =
	/(?:^|\/)(?:\.git|\.env(?:\.[^/]*)?|\.demo|\.cache|node_modules|demo-data-generator|screenshots|recordings)(?:\/|$)|\.(?:map|tsbuildinfo|tgz|parquet)$/i
const privateText =
	/(?:\/home\/[^/\s"']+\/(?:github|projects)|\/mnt\/[a-z]\/Users\/|[A-Z]:[\\/](?:Users|github)[\\/]|codex\x2dclipboard\x2d|BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY|(?:npm|ghp|github_pat)_[A-Za-z0-9]{30,})/
const sourceMap = /(?:[#@]\s*sourceMappingURL\s*=|["']sourcesContent["']\s*:)/

export async function filesIn(directory, prefix = '') {
	const files = []
	for (const entry of await readdir(directory, { withFileTypes: true })) {
		const relative = prefix ? `${prefix}/${entry.name}` : entry.name
		const file = path.join(directory, entry.name)
		if (entry.isSymbolicLink() || (await lstat(file)).isSymbolicLink())
			throw new Error(`Symlinks are not distributable: ${relative}`)
		if (entry.isDirectory()) files.push(...(await filesIn(file, relative)))
		else if (entry.isFile()) files.push(relative)
		else throw new Error(`Non-regular distribution file: ${relative}`)
	}
	return files.sort()
}

/** Scan actual distribution bytes in addition to the filename allowlist. */
export async function auditDirectory(
	directory,
	{
		allowed,
		required = [],
		frontend = false,
		privateRevision,
		trustedUpstream = {},
	} = {},
) {
	if ((await lstat(directory)).isSymbolicLink())
		throw new Error('Symlinks are not distributable staging roots.')
	const files = await filesIn(directory)
	for (const file of required)
		if (!files.includes(file))
			throw new Error(`Required distribution file is missing: ${file}`)
	for (const file of files) {
		if (forbiddenPath.test(file) || (allowed && !allowed(file)))
			throw new Error(`Unexpected distribution file: ${file}`)
		if (
			frontend &&
			/(?:^assets\/fonts\/|\.(?:tsx?|jsx|woff2?|ttf|otf|pdb|dSYM)$)/i.test(
				file,
			) &&
			file !== 'index.d.ts'
		)
			throw new Error(`Source or duplicated font in Web package: ${file}`)
		const bytes = await readFile(path.join(directory, file))
		const text = bytes.toString('utf8')
		const upstream = trustedUpstream[file]
		if (
			upstream &&
			createHash('sha256').update(bytes).digest('hex') !== upstream
		)
			throw new Error(`Upstream executable checksum mismatch: ${file}`)
		// Pinned upstream executables can contain their vendor's build paths.
		// Credentials and this release's private revision are never exempted.
		const sensitive =
			/BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY|(?:npm|ghp|github_pat)_[A-Za-z0-9]{30,}/.test(
				text,
			)
		if (
			(!upstream && privateText.test(text)) ||
			sensitive ||
			(privateRevision && text.includes(privateRevision))
		)
			throw new Error(`Private build information in distribution file: ${file}`)
		if (/\.(?:js|mjs|cjs|css|html|json)$/.test(file) && sourceMap.test(text))
			throw new Error(
				`Source map reference or embedded source in distribution file: ${file}`,
			)
	}
	return files
}

export const webFileAllowed = (file) =>
	/^(?:package\.json|index\.(?:js|d\.ts)|release\.json|LICENSE|README\.md|THIRD_PARTY_NOTICES\.txt|assets\/(?:[^/]+\.(?:html|svg|webmanifest|ico|png)|assets\/[^/]+\.(?:js|css|wasm)))$/.test(
		file,
	)
export const cliFileAllowed = (file) =>
	/^(?:bluink|package\.json|npm-shrinkwrap\.json|LICENSE|README\.md|THIRD_PARTY_NOTICES\.txt|release\.json|dist\/(?:package\.json|(?:src\/)?(?:native\/)?[\w-]+\.js)|runtime\/(?:index\.cjs|bluink_runtime\.(?:linux-x64-gnu|win32-x64-msvc|darwin-arm64)\.node)|vendor\/(?:theme-macros\/(?:html\.html|pdf\.j2)|themes\/(?:classic|editorial)\/(?:manifest\.json|html\/article\.html|site\/(?:page|listing)\.html|pdf\/document\.typ)))$/.test(
		file,
	)
