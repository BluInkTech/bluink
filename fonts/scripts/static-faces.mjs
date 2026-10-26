// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

// Build complete static Word fonts. HarfBuzz owns font instancing; this module
// changes only family/style metadata and repacks table bytes with new checksums.
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fonts } from '../catalogue.js'

const require = createRequire(import.meta.url)
const generator = { name: 'harfbuzzjs', version: '1.6.3' }
const wasm = await readFile(
	require.resolve('harfbuzzjs/dist/harfbuzz-subset.wasm'),
)
const {
	instance: { exports: hb },
} = await WebAssembly.instantiate(wasm)
hb._initialize()

function digest(data) {
	return createHash('sha256').update(data).digest('hex')
}

function tables(bytes) {
	if (bytes.readUInt32BE(0) !== 0x00010000)
		throw new Error('Word faces must be TrueType fonts')
	const result = new Map()
	for (let index = 0; index < bytes.readUInt16BE(4); index++) {
		const entry = 12 + index * 16
		const offset = bytes.readUInt32BE(entry + 8)
		const length = bytes.readUInt32BE(entry + 12)
		if (offset + length > bytes.length)
			throw new Error('Invalid font table bounds')
		result.set(
			bytes.toString('ascii', entry, entry + 4),
			Buffer.from(bytes.subarray(offset, offset + length)),
		)
	}
	return result
}

function editableRights(parsed) {
	const flags = parsed.get('OS/2').readUInt16BE(8)
	if (![0, 8].includes(flags & 15) || flags & 512)
		throw new Error(
			`Font does not permit editable outline embedding: fsType=${flags}`,
		)
	return flags
}

function instantiate(bytes, weight) {
	const pointer = hb.malloc(bytes.length)
	if (!pointer) throw new Error('Cannot allocate font input')
	let face = 0
	let input = 0
	let outputFace = 0
	let outputBlob = 0
	try {
		new Uint8Array(hb.memory.buffer).set(bytes, pointer)
		const blob = hb.hb_blob_create(
			pointer,
			bytes.length,
			2 /* WRITABLE */,
			0,
			0,
		)
		face = hb.hb_face_create(blob, 0)
		hb.hb_blob_destroy(blob)
		input = hb.hb_subset_input_create_or_fail()
		if (!input) throw new Error('Cannot allocate font instancer')
		// Include unencoded glyphs, all layout features/scripts/names and hinting.
		hb.hb_subset_input_keep_everything(input)
		hb.hb_subset_input_set_flags(
			input,
			hb.hb_subset_input_get_flags(input) | 2 /* RETAIN_GIDS */,
		)
		if (
			!hb.hb_subset_input_pin_all_axes_to_default(input, face) ||
			!hb.hb_subset_input_pin_axis_location(
				input,
				face,
				0x77676874 /* wght */,
				weight,
			)
		)
			throw new Error('Cannot pin variable font axes')
		outputFace = hb.hb_subset_or_fail(face, input)
		if (!outputFace) throw new Error('Cannot instantiate variable font')
		outputBlob = hb.hb_face_reference_blob(outputFace)
		const offset = hb.hb_blob_get_data(outputBlob, 0)
		const length = hb.hb_blob_get_length(outputBlob)
		// Memory can grow during each WASM call. Copy from a fresh view before free.
		return Buffer.from(new Uint8Array(hb.memory.buffer, offset, length))
	} finally {
		if (outputBlob) hb.hb_blob_destroy(outputBlob)
		if (outputFace) hb.hb_face_destroy(outputFace)
		if (input) hb.hb_subset_input_destroy(input)
		if (face) hb.hb_face_destroy(face)
		hb.free(pointer)
	}
}

function renameNames(bytes, family, weight, italic) {
	const style =
		`${weight === 700 ? 'Bold' : ''}${italic ? ' Italic' : ''}`.trim() ||
		'Regular'
	const replacements = new Map([
		[1, family],
		[2, style],
		[3, `BluinkStatic-1.0-${family}-${style}`],
		[4, `${family} ${style}`],
		[6, `${family.replaceAll(' ', '')}-${style.replaceAll(' ', '')}`],
		[16, family],
		[17, style],
		[21, family],
		[22, style],
	])
	const format = bytes.readUInt16BE(0)
	if (format !== 0 && format !== 1)
		throw new Error('Unsupported font name format')
	const count = bytes.readUInt16BE(2)
	const storage = bytes.readUInt16BE(4)
	const records = []
	const encode = (text, unicode) =>
		unicode ? Buffer.from(text, 'utf16le').swap16() : Buffer.from(text, 'ascii')
	for (let index = 0; index < count; index++) {
		const offset = 6 + index * 12
		const record = Buffer.from(bytes.subarray(offset, offset + 12))
		const id = record.readUInt16BE(6)
		if (id === 18 || id === 25) continue
		const platform = record.readUInt16BE(0)
		const start = storage + record.readUInt16BE(10)
		const original = bytes.subarray(start, start + record.readUInt16BE(8))
		records.push({
			record,
			value: replacements.has(id)
				? encode(replacements.get(id), platform === 0 || platform === 3)
				: original,
		})
	}
	for (const [id, value] of replacements) {
		if (
			records.some(
				({ record }) =>
					record.readUInt16BE(0) === 3 &&
					record.readUInt16BE(2) === 1 &&
					record.readUInt16BE(4) === 0x409 &&
					record.readUInt16BE(6) === id,
			)
		)
			continue
		const record = Buffer.alloc(12)
		for (const [offset, number] of [
			[0, 3],
			[2, 1],
			[4, 0x409],
			[6, id],
		])
			record.writeUInt16BE(number, offset)
		records.push({ record, value: encode(value, true) })
	}
	records.sort((a, b) =>
		Buffer.compare(a.record.subarray(0, 8), b.record.subarray(0, 8)),
	)
	const languages = []
	if (format === 1) {
		const languageOffset = 6 + count * 12
		for (let index = 0; index < bytes.readUInt16BE(languageOffset); index++) {
			const offset = languageOffset + 2 + index * 4
			const start = storage + bytes.readUInt16BE(offset + 2)
			languages.push(bytes.subarray(start, start + bytes.readUInt16BE(offset)))
		}
	}
	const header = Buffer.alloc(
		6 + records.length * 12 + (format === 1 ? 2 + languages.length * 4 : 0),
	)
	header.writeUInt16BE(format, 0)
	header.writeUInt16BE(records.length, 2)
	header.writeUInt16BE(header.length, 4)
	const strings = []
	let length = 0
	for (const [index, { record, value }] of records.entries()) {
		record.writeUInt16BE(value.length, 8)
		record.writeUInt16BE(length, 10)
		record.copy(header, 6 + index * 12)
		strings.push(value)
		length += value.length
	}
	if (format === 1) {
		const offset = 6 + records.length * 12
		header.writeUInt16BE(languages.length, offset)
		for (const [index, value] of languages.entries()) {
			header.writeUInt16BE(value.length, offset + 2 + index * 4)
			header.writeUInt16BE(length, offset + 4 + index * 4)
			strings.push(value)
			length += value.length
		}
	}
	return Buffer.concat([header, ...strings])
}

function checksum(bytes) {
	let sum = 0
	for (let offset = 0; offset < bytes.length; offset += 4) {
		let word = 0
		for (let index = 0; index < 4; index++)
			word = (word << 8) | (bytes[offset + index] ?? 0)
		sum = (sum + (word >>> 0)) >>> 0
	}
	return sum
}

function pack(parsed) {
	const entries = [...parsed].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
	let offset = 12 + entries.length * 16
	const output = Buffer.alloc(
		offset +
			entries.reduce(
				(sum, [, bytes]) => sum + Math.ceil(bytes.length / 4) * 4,
				0,
			),
	)
	output.writeUInt32BE(0x00010000, 0)
	output.writeUInt16BE(entries.length, 4)
	const power = Math.floor(Math.log2(entries.length))
	output.writeUInt16BE(16 * 2 ** power, 6)
	output.writeUInt16BE(power, 8)
	output.writeUInt16BE(entries.length * 16 - 16 * 2 ** power, 10)
	let headOffset = 0
	for (const [index, [tag, bytes]] of entries.entries()) {
		const record = 12 + index * 16
		output.write(tag, record, 4, 'ascii')
		output.writeUInt32BE(checksum(bytes), record + 4)
		output.writeUInt32BE(offset, record + 8)
		output.writeUInt32BE(bytes.length, record + 12)
		bytes.copy(output, offset)
		if (tag === 'head') headOffset = offset
		offset += Math.ceil(bytes.length / 4) * 4
	}
	output.writeUInt32BE((0xb1b0afba - checksum(output)) >>> 0, headOffset + 8)
	return output
}

function buildFace(source, family, weight, italic) {
	const original = tables(source)
	const permissions = editableRights(original)
	const glyphs = original.get('maxp').readUInt16BE(4)
	const parsed = original.has('fvar')
		? tables(instantiate(source, weight))
		: original
	if (
		parsed.has('fvar') ||
		parsed.has('gvar') ||
		parsed.get('maxp').readUInt16BE(4) !== glyphs
	)
		throw new Error(
			'Static generation must retain every glyph and remove variation axes',
		)
	if (editableRights(parsed) !== permissions)
		throw new Error('Embedding permissions changed')
	parsed.delete('DSIG')
	parsed.set('name', renameNames(parsed.get('name'), family, weight, italic))
	const os2 = parsed.get('OS/2')
	os2.writeUInt16BE(weight, 4)
	os2.writeUInt16BE(
		(os2.readUInt16BE(62) & ~(1 | 32 | 64)) |
			(italic ? 1 : 0) |
			(weight === 700 ? 32 : 0) |
			(weight === 400 && !italic ? 64 : 0),
		62,
	)
	const head = parsed.get('head')
	head.writeUInt32BE(0, 8)
	head.writeBigUInt64BE(2082844800n, 20)
	head.writeBigUInt64BE(2082844800n, 28)
	head.writeUInt16BE((weight === 700 ? 1 : 0) | (italic ? 2 : 0), 44)
	return { bytes: pack(parsed), fsType: permissions, glyphCount: glyphs }
}

export async function buildStaticFaces(stage, sources) {
	const output = path.join(stage, 'docx')
	await mkdir(output)
	const families = []
	for (const entry of fonts) {
		const sourceFaces = sources.find((item) => item.id === entry.id).faces
		// Stable aliases avoid installed originals and upstream OFL reserved names.
		const family = `Bluink ${digest(entry.id).slice(0, 12).toUpperCase()}`
		const directory = path.join(output, entry.id)
		await mkdir(directory)
		await writeFile(
			path.join(directory, 'LICENSE'),
			await readFile(path.join(stage, 'pdf', entry.id, 'LICENSE')),
		)
		const faces = []
		for (const [weight, italic] of [
			[400, false],
			[700, false],
			[400, true],
			[700, true],
		]) {
			const style = italic ? 'italic' : 'normal'
			const source = sourceFaces.find((face) => {
				const bounds = face.weight.split(' ').map(Number)
				return (
					face.style === style && bounds[0] <= weight && bounds.at(-1) >= weight
				)
			})
			if (!source) throw new Error(`Missing ${entry.id} ${weight} ${style}`)
			const input = await readFile(
				path.join(
					stage,
					'pdf',
					entry.id,
					`${entry.id}-${source.weight.replaceAll(' ', '-')}-${style}.ttf`,
				),
			)
			const { bytes, fsType, glyphCount } = buildFace(
				input,
				family,
				weight,
				italic,
			)
			const filename = `${entry.id}-${weight}-${style}.ttf`
			await writeFile(path.join(directory, filename), bytes)
			faces.push({
				weight,
				italic,
				path: `${entry.id}/${filename}`,
				sha256: digest(bytes),
				fsType,
				glyphCount,
			})
		}
		families.push({
			id: entry.id,
			family: entry.family,
			docxFamily: family,
			faces,
		})
	}
	await writeFile(
		path.join(output, 'manifest.json'),
		`${JSON.stringify({ schemaVersion: 1, generator, fonts: families }, null, 2)}\n`,
	)
	console.log(
		`Prepared ${families.length * 4} complete static Word faces using Node/WASM.`,
	)
}
