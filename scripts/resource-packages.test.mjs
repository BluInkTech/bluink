// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import {
	mkdir,
	mkdtemp,
	readFile,
	rm,
	utimes,
	writeFile,
} from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { packDirectory } from './package-artifacts.mjs'
import {
	downloadResources,
	packResource,
	resolvePublishedResources,
	stageInstalledResources,
} from './resource-packages.mjs'
import { json, writeJson } from './release-utils.mjs'

test('single-resource packing is reproducible, omits private tooling and needs no sibling package', async () => {
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-single-resource-'),
	)
	try {
		const source = path.join(temporary, 'fonts')
		await mkdir(path.join(source, 'assets/web'), { recursive: true })
		await mkdir(path.join(source, 'assets/pdf/inter'), { recursive: true })
		await mkdir(path.join(source, 'assets/docx/inter'), { recursive: true })
		const manifest = {
			name: '@bluink/fonts',
			version: '26.10.1-alpha',
			type: 'module',
			license: 'MPL-2.0',
			private: true,
			author: 'Private author metadata',
			devDependencies: { harfbuzzjs: '1.6.4' },
			scripts: { prepack: 'exit 99', build: 'exit 99' },
			files: ['index.js', 'sources.json', 'LICENSE', 'README.md', 'assets'],
		}
		await writeJson(path.join(source, 'package.json'), manifest)
		for (const [file, content] of Object.entries({
			'index.js': 'export const fixture = true;\n',
			'sources.json': '{}\n',
			LICENSE: 'Synthetic licence\n',
			'README.md': 'Synthetic resource\n',
			'assets/web/inter.css': '/* synthetic font */\n',
			'assets/pdf/inter/LICENSE': 'Synthetic font licence\n',
			'assets/docx/manifest.json': '{"schemaVersion":1,"fonts":[]}\n',
			'assets/docx/inter/inter-400-normal.ttf': 'Synthetic static font\n',
			'private-notes.txt': 'Unpublished source notes\n',
		}))
			await writeFile(path.join(source, file), content)
		const options = {
			version: '26.10.2',
			repository: 'https://github.com/BluInkTech/bluink',
			sourceRoot: temporary,
		}
		const first = await packResource(path.join(temporary, 'first'), options)
		await utimes(path.join(source, 'index.js'), new Date(0), new Date(0))
		const second = await packResource(path.join(temporary, 'second'), options)
		assert.equal(first.sha256, second.sha256)
		assert.equal(first.version, '26.10.2')
		assert.equal(first.manifest.publishConfig.access, 'public')
		assert.equal(first.manifest.repository.directory, 'fonts')
		for (const field of ['author', 'private', 'devDependencies', 'scripts'])
			assert.equal(first.manifest[field], undefined)
		assert.equal(first.files.includes('private-notes.txt'), false)
		assert.deepEqual(await json(path.join(source, 'package.json')), manifest)
		await assert.rejects(
			packResource(temporary, { ...options, version: 'latest' }),
			/calendar/,
		)
		await writeJson(path.join(source, 'package.json'), {
			...manifest,
			files: ['../private-notes.txt'],
		})
		await assert.rejects(
			packResource(temporary, options),
			/unsafe package files/,
		)
		await writeJson(path.join(source, 'package.json'), {
			...manifest,
			files: [...manifest.files, 'private-notes.txt'],
		})
		await assert.rejects(
			packResource(temporary, options),
			/unsafe package files/,
		)
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
})

test('CLI preparation downloads frozen registry bytes and never needs resource builders', async () => {
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-resource-download-'),
	)
	try {
		const source = path.join(temporary, 'source')
		await mkdir(source)
		await writeJson(path.join(source, 'package.json'), {
			name: '@bluink/fonts',
			version: '26.10.2',
			files: ['index.js'],
			license: 'MPL-2.0',
		})
		await writeFile(
			path.join(source, 'index.js'),
			'export const version = "26.10.2";\n',
		)
		const record = await packDirectory(source, path.join(temporary, 'packed'), {
			allowed: () => true,
		})
		const bytes = await readFile(record.archive)
		const requests = []
		const fetchArchive = async (url) => {
			requests.push(url)
			return new Response(bytes)
		}
		const records = await downloadResources(
			[record],
			path.join(temporary, 'downloaded'),
			fetchArchive,
		)
		assert.equal(records[0].sha256, record.sha256)
		assert.deepEqual(requests, [
			'https://registry.npmjs.org/@bluink/fonts/-/fonts-26.10.2.tgz',
		])
		const resolved = await resolvePublishedResources(
			[{ name: record.name, version: record.version }],
			path.join(temporary, 'resolved'),
			{
				lookup: async (name, version) => ({
					name,
					version,
					dist: { integrity: record.integrity },
				}),
				fetchArchive,
			},
		)
		assert.equal(resolved[0].sha256, record.sha256)
		await assert.rejects(
			downloadResources(
				[{ ...record, sha256: undefined }],
				temporary,
				fetchArchive,
			),
			/SHA-256/,
		)
		await assert.rejects(
			downloadResources(
				[{ ...record, sha256: 'f'.repeat(64) }],
				temporary,
				fetchArchive,
			),
			/integrity mismatch/,
		)
		await assert.rejects(
			downloadResources(
				[record],
				temporary,
				async () => new Response('missing', { status: 404 }),
			),
			/Publish the exact.*HTTP 404/,
		)
		await assert.rejects(
			downloadResources(
				[{ ...record, version: '../../outside' }],
				temporary,
				fetchArchive,
			),
			/Invalid resource download/,
		)
		await assert.rejects(
			resolvePublishedResources(
				[{ name: record.name, version: record.version }],
				temporary,
				{ lookup: async () => undefined },
			),
			/Publish the exact/,
		)
		await assert.rejects(
			resolvePublishedResources(
				[{ name: record.name, version: 'latest' }],
				temporary,
			),
			/exact published/,
		)
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
})

test('source builds stage installed fonts and fail on catalogue drift', async () => {
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-installed-resources-'),
	)
	try {
		const installRoot = path.join(temporary, 'checkout')
		const packageRoot = path.join(
			installRoot,
			'backend/node_modules/@bluink/fonts',
		)
		await mkdir(path.join(installRoot, 'fonts'), { recursive: true })
		await mkdir(path.join(packageRoot, 'assets'), { recursive: true })
		await writeJson(path.join(packageRoot, 'package.json'), {
			name: '@bluink/fonts',
			type: 'module',
			exports: './index.js',
		})
		await writeFile(path.join(packageRoot, 'index.js'), 'export {}\n')
		const catalogue = '[{"id":"fixture"}]\n'
		await writeFile(path.join(installRoot, 'fonts/catalogue.json'), catalogue)
		await writeFile(path.join(packageRoot, 'catalogue.json'), catalogue)
		await writeFile(
			path.join(packageRoot, 'assets/font.woff2'),
			'paired assets',
		)
		await stageInstalledResources({ installRoot })
		assert.equal(
			await readFile(path.join(installRoot, 'fonts/assets/font.woff2'), 'utf8'),
			'paired assets',
		)
		await writeFile(path.join(packageRoot, 'catalogue.json'), 'different')
		await assert.rejects(
			stageInstalledResources({ installRoot, packageRoot }),
			/fonts contract differs/,
		)
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
})
