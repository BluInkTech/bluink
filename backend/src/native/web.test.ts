// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { webDirectory } from './web.js'

test('bundled frontend resolves without an npm web dependency and rejects missing or mismatched assets', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-web-runtime-'))
	try {
		assert.throws(
			() => webDirectory('26.10.7-alpha', '26.10.2', directory),
			/missing/,
		)
		await mkdir(path.join(directory, 'web/assets'), { recursive: true })
		await writeFile(
			path.join(directory, 'web/assets/index.html'),
			'<html></html>',
		)
		const metadata = {
			format: 'bluink.web',
			formatVersion: 1,
			version: '26.10.7-alpha',
			fontsVersion: '26.10.2',
		}
		await writeFile(
			path.join(directory, 'web/release.json'),
			JSON.stringify(metadata),
		)
		assert.equal(
			webDirectory(metadata.version, metadata.fontsVersion, directory),
			path.join(directory, 'web/assets'),
		)
		assert.throws(
			() => webDirectory('26.10.8-alpha', metadata.fontsVersion, directory),
			/requires frontend/,
		)
		assert.throws(
			() => webDirectory(metadata.version, '26.10.3', directory),
			/requires @bluink\/fonts/,
		)
		await writeFile(
			path.join(directory, 'web/release.json'),
			JSON.stringify({ ...metadata, formatVersion: 2 }),
		)
		assert.throws(
			() => webDirectory(metadata.version, metadata.fontsVersion, directory),
			/invalid/,
		)
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})
