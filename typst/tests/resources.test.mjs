// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import os from 'node:os'
import path from 'node:path'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFile, stat } from 'node:fs/promises'
import test from 'node:test'
import { targets, typstBinary, upstreamVersion } from '../index.js'
import sources from '../sources.json' with { type: 'json' }

test('all packaged targets include executable bytes and upstream licence notices', async () => {
	const notice = await readFile(
		new URL('../licenses/typst-assets-NOTICE', import.meta.url),
	)
	assert.equal(
		createHash('sha256').update(notice).digest('hex'),
		sources.assetsNotice.noticeSha256,
	)
	for (const [target, relative] of Object.entries(targets)) {
		const [platform, arch] = target.split('/')
		const binary = typstBinary(platform, arch)
		assert.ok(binary.endsWith(relative.replaceAll('/', path.sep)))
		const bytes = await readFile(binary)
		assert.ok(bytes.length > 1_000_000)
		if (platform !== 'win32') assert.ok((await stat(binary)).mode & 0o111)
		const folder = new URL(
			`../assets/${relative.substring(0, relative.lastIndexOf('/'))}/`,
			import.meta.url,
		)
		assert.ok(
			(await readFile(new URL('LICENSE', folder), 'utf8')).includes('Apache'),
		)
		assert.ok((await readFile(new URL('NOTICE', folder), 'utf8')).length > 1000)
	}
	assert.throws(() => typstBinary('linux', 'arm64'), /not packaged/)
})

test('the resolver runs the pinned engine independent of cwd', () => {
	assert.match(
		execFileSync(typstBinary(), ['--version'], {
			cwd: os.tmpdir(),
			encoding: 'utf8',
		}),
		new RegExp(`^typst ${upstreamVersion.replaceAll('.', '\\.')}`),
	)
})
