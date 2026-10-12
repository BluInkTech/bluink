// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { fonts, fontDirectory, fontStylesheet } from '../index.js'
import { typstBinary } from '@bluink/typst'

test('every catalogue family has paired assets, declared faces and upstream notices', async () => {
	const source = JSON.parse(
		await readFile(new URL('../sources.json', import.meta.url)),
	)
	assert.equal(fonts.length, new Set(fonts.map((font) => font.id)).size)
	for (const font of fonts) {
		assert.equal(fontStylesheet(font.id), `/fonts/${font.id}.css`)
		const stylesheet = await readFile(
			path.join(fontDirectory('web'), `${font.id}.css`),
			'utf8',
		)
		assert.ok(stylesheet.includes('SIL OPEN FONT LICENSE'), font.id)
		const faces = source.fonts.find((entry) => entry.id === font.id).faces
		for (const face of faces) {
			const name = `${font.id}-${face.weight.replaceAll(' ', '-')}-${face.style}`
			const pdf = await readFile(
				path.join(fontDirectory('pdf'), font.id, `${name}.ttf`),
			)
			assert.equal(createHash('sha256').update(pdf).digest('hex'), face.sha256)
			const web = await readFile(
				path.join(fontDirectory('web'), font.id, `${name}.woff2`),
			)
			assert.equal(web.subarray(0, 4).toString(), 'wOF2')
			assert.ok(stylesheet.includes(`./${font.id}/${name}.woff2`))
		}
		assert.deepEqual(
			await readFile(path.join(fontDirectory('pdf'), font.id, 'LICENSE')),
			await readFile(
				path.join(fontDirectory('web'), 'licenses', `${font.id}.txt`),
			),
		)
	}
	assert.throws(() => fontStylesheet('../other'), /Unknown bundled font/)
	assert.throws(() => fontDirectory('other'), /Unsupported font format/)
})

test('Typst finds and renders all catalogue families, including bold and italic', async () => {
	const temporary = await mkdtemp(path.join(os.tmpdir(), 'bluink-font-render-'))
	try {
		const source = fonts
			.map(
				(font) =>
					`#text(font: ${JSON.stringify(font.family)})[${font.family}: Résumé 123 #strong[Bold] #emph[Italic]]`,
			)
			.join('\n\n')
		const input = path.join(temporary, 'fonts.typ')
		const output = path.join(temporary, 'fonts.pdf')
		await writeFile(input, source)
		const compilation = spawnSync(
			typstBinary(),
			[
				'compile',
				'--ignore-system-fonts',
				'--font-path',
				fontDirectory('pdf'),
				input,
				output,
			],
			{ encoding: 'utf8' },
		)
		assert.equal(compilation.status, 0, compilation.stderr)
		assert.equal(
			compilation.stderr,
			'',
			'All declared font faces should render without warnings.',
		)
		assert.equal((await readFile(output)).subarray(0, 5).toString(), '%PDF-')
		const available = execFileSync(
			typstBinary(),
			['fonts', '--ignore-system-fonts', '--font-path', fontDirectory('pdf')],
			{ encoding: 'utf8' },
		)
		for (const font of fonts)
			assert.ok(available.split('\n').includes(font.family), font.family)
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
})
