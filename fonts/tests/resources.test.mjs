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
import * as hb from 'harfbuzzjs'

function tables(bytes) {
	assert.equal(bytes.readUInt32BE(0), 0x00010000)
	const result = new Map()
	for (let index = 0; index < bytes.readUInt16BE(4); index++) {
		const entry = 12 + index * 16
		const offset = bytes.readUInt32BE(entry + 8)
		const length = bytes.readUInt32BE(entry + 12)
		assert.ok(offset + length <= bytes.length)
		result.set(
			bytes.toString('ascii', entry, entry + 4),
			bytes.subarray(offset, offset + length),
		)
	}
	return result
}

function checksum(bytes) {
	let sum = 0
	for (let offset = 0; offset < bytes.length; offset += 4) {
		let word = 0
		for (let index = 0; index < 4; index++)
			word = word * 256 + (bytes[offset + index] ?? 0)
		sum = (sum + word) >>> 0
	}
	return sum
}

function shapedFont(bytes, weight) {
	const face = new hb.Face(new hb.Blob(bytes))
	const font = new hb.Font(face)
	font.setVariations([new hb.Variation('wght', weight)])
	return font
}

function shape(bytes, text, weight) {
	const font = shapedFont(bytes, weight)
	const buffer = new hb.Buffer()
	buffer.addText(text)
	buffer.guessSegmentProperties()
	hb.shape(font, buffer)
	return buffer.getGlyphInfosAndPositions()
}

test('Word faces retain complete glyphs, shaping, permissions and checksums through Node/WASM generation', async () => {
	const manifest = JSON.parse(
		await readFile(path.join(fontDirectory('docx'), 'manifest.json'), 'utf8'),
	)
	const sources = JSON.parse(
		await readFile(new URL('../sources.json', import.meta.url), 'utf8'),
	)
	assert.equal(manifest.schemaVersion, 1)
	assert.deepEqual(manifest.generator, { name: 'harfbuzzjs', version: '1.6.3' })
	assert.equal(manifest.fonts.length, fonts.length)
	for (const font of manifest.fonts) {
		assert.equal(
			fonts.find((entry) => entry.id === font.id).family,
			font.family,
		)
		assert.match(font.docxFamily, /^Bluink [A-F0-9]{12}$/)
		assert.deepEqual(
			font.faces.map((face) => [face.weight, face.italic]),
			[
				[400, false],
				[700, false],
				[400, true],
				[700, true],
			],
		)
		for (const face of font.faces) {
			const data = await readFile(path.join(fontDirectory('docx'), face.path))
			assert.equal(createHash('sha256').update(data).digest('hex'), face.sha256)
			assert.equal(checksum(data), 0xb1b0afba, face.path)
			for (let index = 0; index < data.readUInt16BE(4); index++) {
				const entry = 12 + index * 16
				const offset = data.readUInt32BE(entry + 8)
				const table = Buffer.from(
					data.subarray(offset, offset + data.readUInt32BE(entry + 12)),
				)
				if (data.toString('ascii', entry, entry + 4) === 'head')
					table.writeUInt32BE(0, 8)
				assert.equal(checksum(table), data.readUInt32BE(entry + 4), face.path)
			}
			const parsed = tables(data)
			assert.equal(parsed.has('fvar'), false)
			assert.equal(parsed.has('gvar'), false)
			const metrics = parsed.get('OS/2')
			assert.equal(metrics.readUInt16BE(4), face.weight)
			assert.equal(metrics.readUInt16BE(8), face.fsType)
			assert.ok([0, 8].includes(face.fsType & 15) && !(face.fsType & 512))
			assert.equal(!!(metrics.readUInt16BE(62) & 1), face.italic)
			assert.equal(!!(metrics.readUInt16BE(62) & 32), face.weight === 700)
			assert.equal(parsed.get('maxp').readUInt16BE(4), face.glyphCount)
			const source = sources.fonts
				.find((entry) => entry.id === font.id)
				.faces.find((source) => {
					const bounds = source.weight.split(' ').map(Number)
					return (
						source.style === (face.italic ? 'italic' : 'normal') &&
						bounds[0] <= face.weight &&
						bounds.at(-1) >= face.weight
					)
				})
			const original = await readFile(
				path.join(
					fontDirectory('pdf'),
					font.id,
					`${font.id}-${source.weight.replaceAll(' ', '-')}-${source.style}.ttf`,
				),
			)
			assert.equal(
				tables(original).get('maxp').readUInt16BE(4),
				face.glyphCount,
			)
			for (const tag of [
				'GPOS',
				'GSUB',
				'GDEF',
				'kern',
				'fpgm',
				'prep',
				'cvt ',
			]) {
				if (tables(original).has(tag))
					assert.ok(parsed.has(tag), `${face.path}: ${tag}`)
			}
			const expectedFont = shapedFont(original, face.weight)
			const actualFont = shapedFont(data, face.weight)
			for (const char of 'ARBgmé012') {
				const expected = expectedFont.glyphToJson(
					expectedFont.nominalGlyph(char.codePointAt(0)),
				)
				const actual = actualFont.glyphToJson(
					actualFont.nominalGlyph(char.codePointAt(0)),
				)
				assert.equal(
					actual.length,
					expected.length,
					`${face.path}: ${char} outline`,
				)
				for (const [index, command] of expected.entries()) {
					assert.equal(actual[index].type, command.type)
					assert.equal(actual[index].values.length, command.values.length)
					for (const [coordinate, value] of command.values.entries())
						assert.ok(
							Math.abs(actual[index].values[coordinate] - value) <= 1,
							`${face.path}: ${char} outline coordinate`,
						)
				}
			}
			for (const text of [
				'AVATAR To office ffi fi fl Résumé 123',
				'a\u0301 e\u0308',
				'Ελληνικά',
				'Кириллица',
				'العربية',
				'हिन्दी',
			]) {
				const expected = shape(original, text, face.weight)
				const actual = shape(data, text, face.weight)
				assert.equal(actual.length, expected.length, `${face.path}: ${text}`)
				for (const [index, glyph] of expected.entries()) {
					assert.equal(
						actual[index].codepoint,
						glyph.codepoint,
						`${face.path}: ${text}`,
					)
					assert.equal(actual[index].cluster, glyph.cluster)
					for (const metric of ['xAdvance', 'yAdvance', 'xOffset', 'yOffset'])
						assert.ok(
							Math.abs(actual[index][metric] - glyph[metric]) <= 1,
							`${face.path}: ${text} ${metric}`,
						)
				}
			}
		}
		assert.deepEqual(
			await readFile(path.join(fontDirectory('docx'), font.id, 'LICENSE')),
			await readFile(path.join(fontDirectory('pdf'), font.id, 'LICENSE')),
		)
	}
})

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
