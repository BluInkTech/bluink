// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import {
	downloadFrontend,
	frontendAudit,
	frontendInput,
	stageBuildFrontend,
	stageFrontend,
	validateFrontend,
	verifyBundledFrontend,
} from './frontend-asset.mjs'
import { packDirectory } from './package-artifacts.mjs'
import { writeJson } from './release-utils.mjs'

test('Release frontend download and bundling are bound to exact bytes; source builds reuse only a verified cache', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-frontend-'))
	try {
		const source = path.join(directory, 'source')
		await mkdir(path.join(source, 'assets'), { recursive: true })
		await writeJson(path.join(source, 'package.json'), {
			name: '@bluink/web',
			version: '26.10.7-alpha',
			private: true,
			files: ['assets', 'release.json', 'LICENSE', 'THIRD_PARTY_NOTICES.txt'],
		})
		await writeJson(path.join(source, 'release.json'), {
			format: 'bluink.web',
			formatVersion: 1,
			version: '26.10.7-alpha',
			fontsVersion: '26.10.2',
		})
		for (const file of [
			'LICENSE',
			'THIRD_PARTY_NOTICES.txt',
			'assets/index.html',
		])
			await writeFile(path.join(source, file), 'Compiled frontend fixture.\n')
		const record = await packDirectory(
			source,
			path.join(directory, 'packed'),
			frontendAudit,
		)
		const repository = 'https://github.com/BluInkTech/bluink'
		const input = frontendInput(record, repository)
		validateFrontend(input, record.version, repository)
		for (const override of [
			{ version: '26.10.8-alpha' },
			{ url: 'https://example.com/file.tgz' },
			{ sha256: 'invalid' },
			{ archive: '/private/path' },
		])
			assert.throws(
				() =>
					validateFrontend(
						{ ...input, ...override },
						record.version,
						repository,
					),
				/frontend/,
			)
		const bytes = await readFile(record.archive)
		const downloaded = await downloadFrontend(
			input,
			repository,
			path.join(directory, 'download'),
			async (url) => {
				assert.equal(url, input.url)
				return new Response(bytes)
			},
		)
		assert.equal(downloaded.sha256, record.sha256)
		await assert.rejects(
			downloadFrontend(
				input,
				repository,
				directory,
				async () => new Response('missing', { status: 404 }),
			),
			/Upload.*404/,
		)
		await assert.rejects(
			downloadFrontend(
				input,
				repository,
				directory,
				async () => new Response('changed'),
			),
			/integrity mismatch/,
		)
		const bundled = path.join(directory, 'bundled')
		await stageFrontend(record, bundled)
		await verifyBundledFrontend(bundled, record)
		await writeFile(path.join(bundled, 'assets/index.html'), 'Changed frontend')
		await assert.rejects(verifyBundledFrontend(bundled, record), /differs/)
		await writeFile(path.join(bundled, 'assets/debug.js.map'), '{}')
		await assert.rejects(verifyBundledFrontend(bundled, record), /Unexpected/)
		const checkout = path.join(directory, 'checkout')
		await mkdir(path.join(checkout, '.resource-packages'), { recursive: true })
		await writeJson(path.join(checkout, 'release-input.json'), {
			version: record.version,
			publicRepository: repository,
			frontend: input,
		})
		await writeFile(
			path.join(checkout, '.resource-packages', record.filename),
			bytes,
		)
		await stageBuildFrontend(checkout)
		await verifyBundledFrontend(path.join(checkout, 'backend/web'), record)
		await writeFile(
			path.join(checkout, '.resource-packages', record.filename),
			'changed cache',
		)
		await assert.rejects(stageBuildFrontend(checkout), /integrity mismatch/)
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})
