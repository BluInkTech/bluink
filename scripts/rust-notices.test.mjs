// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, unlink, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { dependencyLicenseTexts } from './rust-notices.mjs'

test('vendored Git notices retain the linking exception and native dependency credits', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-git-notices-'))
	try {
		const contents = {
			'LICENSE-MIT': 'Binding licence\n',
			'libgit2/COPYING': 'Native licence\nLINKING EXCEPTION\n',
			'libgit2/AUTHORS': 'Native contributors\n',
			'libgit2/git.git-authors': 'Git contributors\n',
			'libgit2/deps/parser/LICENSE-MIT': 'Parser licence\n',
		}
		for (const [file, text] of Object.entries(contents)) {
			const destination = path.join(directory, file)
			await mkdir(path.dirname(destination), { recursive: true })
			await writeFile(destination, text)
		}
		const pkg = {
			name: 'libgit2-sys',
			manifest_path: path.join(directory, 'Cargo.toml'),
		}
		assert.deepEqual(
			await dependencyLicenseTexts(pkg),
			Object.entries(contents)
				.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
				.map(([file, text]) => `${file}\n${text}`),
		)
		await unlink(path.join(directory, 'libgit2/COPYING'))
		await assert.rejects(dependencyLicenseTexts(pkg), /ENOENT/)
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})
