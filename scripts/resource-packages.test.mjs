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
import { json, sha256, writeJson } from './release-utils.mjs'

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
			devDependencies: { '@bluink/typst': '26.10.1-alpha' },
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
		const first = await packResource(
			'fonts',
			path.join(temporary, 'first'),
			options,
		)
		await utimes(path.join(source, 'index.js'), new Date(0), new Date(0))
		const second = await packResource(
			'fonts',
			path.join(temporary, 'second'),
			options,
		)
		assert.equal(first.sha256, second.sha256)
		assert.equal(first.version, '26.10.2')
		assert.equal(first.manifest.publishConfig.access, 'public')
		assert.equal(first.manifest.repository.directory, 'fonts')
		for (const field of ['author', 'private', 'devDependencies', 'scripts'])
			assert.equal(first.manifest[field], undefined)
		assert.equal(first.files.includes('private-notes.txt'), false)
		assert.deepEqual(await json(path.join(source, 'package.json')), manifest)
		await assert.rejects(
			packResource('web', temporary, options),
			/fonts or typst/,
		)
		await assert.rejects(
			packResource('fonts', temporary, { ...options, version: 'latest' }),
			/calendar/,
		)
		await writeJson(path.join(source, 'package.json'), {
			...manifest,
			files: ['../private-notes.txt'],
		})
		await assert.rejects(
			packResource('fonts', temporary, options),
			/unsafe package files/,
		)
		await writeJson(path.join(source, 'package.json'), {
			...manifest,
			files: [...manifest.files, 'private-notes.txt'],
		})
		await assert.rejects(
			packResource('fonts', temporary, options),
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

test('source builds stage installed resources and fail on catalogue drift or changed Typst binaries', async () => {
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-installed-resources-'),
	)
	try {
		const installRoot = path.join(temporary, 'checkout')
		const packageRoots = Object.fromEntries(
			['fonts', 'typst'].map((kind) => [
				kind,
				path.join(installRoot, 'backend/node_modules/@bluink', kind),
			]),
		)
		for (const kind of ['fonts', 'typst']) {
			await mkdir(path.join(installRoot, kind), { recursive: true })
			await mkdir(path.join(packageRoots[kind], 'assets'), { recursive: true })
			await writeJson(path.join(packageRoots[kind], 'package.json'), {
				name: `@bluink/${kind}`,
				type: 'module',
				exports: './index.js',
			})
			await writeFile(path.join(packageRoots[kind], 'index.js'), 'export {}\n')
		}
		const catalogue = '[{"id":"fixture"}]\n'
		await writeFile(path.join(installRoot, 'fonts/catalogue.json'), catalogue)
		await writeFile(path.join(packageRoots.fonts, 'catalogue.json'), catalogue)
		await writeFile(
			path.join(packageRoots.fonts, 'assets/font.woff2'),
			'paired assets',
		)
		const targets = {}
		for (const target of ['linux-x64', 'windows-x64', 'macos-arm64']) {
			const binary = target === 'windows-x64' ? 'typst.exe' : 'typst'
			const bytes = Buffer.from(target)
			targets[target] = { binary, binarySha256: sha256(bytes) }
			await mkdir(path.join(packageRoots.typst, 'assets', target))
			await writeFile(
				path.join(packageRoots.typst, 'assets', target, binary),
				bytes,
			)
		}
		for (const folder of [packageRoots.typst, path.join(installRoot, 'typst')])
			await writeJson(path.join(folder, 'sources.json'), { targets })
		await stageInstalledResources({ installRoot })
		assert.equal(
			await readFile(path.join(installRoot, 'fonts/assets/font.woff2'), 'utf8'),
			'paired assets',
		)
		assert.equal(
			await readFile(
				path.join(installRoot, 'typst/assets/linux-x64/typst'),
				'utf8',
			),
			'linux-x64',
		)
		await writeFile(
			path.join(packageRoots.fonts, 'catalogue.json'),
			'different',
		)
		await assert.rejects(
			stageInstalledResources({ installRoot, packageRoots }),
			/fonts contract differs/,
		)
		await writeFile(path.join(packageRoots.fonts, 'catalogue.json'), catalogue)
		await writeFile(
			path.join(packageRoots.typst, 'assets/linux-x64/typst'),
			'tampered',
		)
		await assert.rejects(
			stageInstalledResources({ installRoot, packageRoots }),
			/Typst binary checksum mismatch/,
		)
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
})
