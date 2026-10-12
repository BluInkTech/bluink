// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import test from 'node:test'
import { appendFile, mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import {
	packDirectory,
	productionLock,
	verifyArchive,
} from './package-artifacts.mjs'

test('published shrinkwrap records exact verified archives and rejects an incomplete dependency closure', () => {
	const manifest = {
		name: '@bluink/cli',
		version: '26.10.2-alpha',
		license: 'MPL-2.0',
		dependencies: { '@bluink/fonts': '26.10.2-alpha' },
	}
	const fonts = {
		name: '@bluink/fonts',
		version: '26.10.2-alpha',
		integrity: 'sha512-verified',
		manifest: { license: 'MPL-2.0' },
	}
	const lock = productionLock(manifest, [fonts])
	assert.equal(
		lock.packages['node_modules/@bluink/fonts'].integrity,
		fonts.integrity,
	)
	assert.equal(
		lock.packages['node_modules/@bluink/fonts'].resolved,
		'https://registry.npmjs.org/@bluink/fonts/-/fonts-26.10.2-alpha.tgz',
	)
	assert.throws(() => productionLock(manifest, []), /Missing exact dependency/)
	assert.throws(
		() => productionLock(manifest, [{ ...fonts, version: '26.10.1-alpha' }]),
		/Missing exact dependency/,
	)
	assert.throws(
		() =>
			productionLock(manifest, [
				{ ...fonts, manifest: { dependencies: { unknown: '1.0.0' } } },
			]),
		/transitive dependencies/,
	)
})

test('dependency verification inspects packed identity and rejects changed archive bytes', async () => {
	const temporary = await mkdtemp(path.join(os.tmpdir(), 'bluink-pack-test-'))
	try {
		const directory = path.join(temporary, 'input')
		await mkdir(directory)
		await writeFile(
			path.join(directory, 'package.json'),
			JSON.stringify({
				name: '@bluink/example',
				version: '1.0.0',
				files: ['index.js'],
			}),
		)
		await writeFile(
			path.join(directory, 'index.js'),
			'export const ready = true',
		)
		const options = {
			allowed: (file) => ['package.json', 'index.js'].includes(file),
		}
		const archive = await packDirectory(
			directory,
			path.join(temporary, 'output'),
			options,
		)
		const verified = await verifyArchive(
			{ ...archive, manifest: { name: 'forged' } },
			options,
		)
		assert.equal(verified.manifest.name, '@bluink/example')
		await assert.rejects(
			verifyArchive({ ...archive, version: '2.0.0' }, options),
			/identity mismatch/,
		)
		await appendFile(archive.archive, 'changed')
		await assert.rejects(verifyArchive(archive, options), /integrity mismatch/)
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
})
