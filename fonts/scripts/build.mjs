// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { createHash } from 'node:crypto'
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import wawoff2 from 'wawoff2'
import { fonts } from '../catalogue.js'

const root = path.resolve(import.meta.dirname, '..')
const lock = JSON.parse(await readFile(path.join(root, 'sources.json'), 'utf8'))
const assets = path.join(root, 'assets')
const cache = path.join(root, '.cache')

function hash(bytes) {
	return createHash('sha256').update(bytes).digest('hex')
}

async function download(source) {
	const filename = path.join(cache, source.sha256)
	let bytes = await readFile(filename).catch(() => undefined)
	if (bytes && hash(bytes) === source.sha256) return bytes
	const response = await fetch(source.url, {
		signal: AbortSignal.timeout(90_000),
	})
	if (!response.ok)
		throw new Error(`Cannot download ${source.url}: HTTP ${response.status}`)
	bytes = Buffer.from(await response.arrayBuffer())
	if (bytes.length > 16 * 1024 * 1024 || hash(bytes) !== source.sha256)
		throw new Error(`Invalid font resource or checksum: ${source.url}`)
	await writeFile(filename, bytes)
	return bytes
}

async function webFont(face, ttf) {
	const filename = path.join(cache, `${face.sha256}.woff2`)
	const [bytes, checksum] = await Promise.all([
		readFile(filename).catch(() => undefined),
		readFile(`${filename}.sha256`, 'utf8').catch(() => undefined),
	])
	if (bytes && hash(bytes) === checksum) return bytes
	const compressed = Buffer.from(await wawoff2.compress(ttf))
	await writeFile(filename, compressed)
	await writeFile(`${filename}.sha256`, hash(compressed))
	return compressed
}

await mkdir(cache, { recursive: true })
// Generate into a sibling directory so failed downloads never damage usable assets.
const stage = path.join(root, '.cache', 'stage')
await rm(stage, { recursive: true, force: true })
await mkdir(path.join(stage, 'web', 'licenses'), { recursive: true })
await mkdir(path.join(stage, 'pdf'), { recursive: true })

const manifest = []
for (const font of fonts) {
	const sources = lock.fonts.find((entry) => entry.id === font.id)
	if (!sources?.faces.length)
		throw new Error(`Missing locked sources for ${font.id}`)
	const web = path.join(stage, 'web', font.id)
	const pdf = path.join(stage, 'pdf', font.id)
	await mkdir(web, { recursive: true })
	await mkdir(pdf, { recursive: true })
	const rules = []
	for (const face of sources.faces) {
		const ttf = await download(face)
		const basename = `${font.id}-${face.weight.replaceAll(' ', '-')}-${face.style}`
		await writeFile(path.join(pdf, `${basename}.ttf`), ttf)
		await writeFile(
			path.join(web, `${basename}.woff2`),
			await webFont(face, ttf),
		)
		rules.push(
			`@font-face { font-family: '${font.family}'; font-style: ${face.style}; font-weight: ${face.weight}; font-display: swap; src: url('./${font.id}/${basename}.woff2') format('woff2'); }`,
		)
	}
	const license = await download(sources.license)
	await writeFile(
		path.join(stage, 'web', `${font.id}.css`),
		`/* ${license.toString('utf8').replaceAll('*/', '* /')} */\n${rules.join('\n')}\n`,
	)
	await writeFile(
		path.join(stage, 'web', 'licenses', `${font.id}.txt`),
		license,
	)
	await writeFile(path.join(pdf, 'LICENSE'), license)
	manifest.push({
		...font,
		license: 'OFL-1.1',
		source: sources.license.url,
		faces: sources.faces.map(({ weight, style }) => ({ weight, style })),
	})
}
await writeFile(
	path.join(stage, 'web', 'licenses.json'),
	`${JSON.stringify(manifest, null, 2)}\n`,
)
await writeFile(
	path.join(stage, 'manifest.json'),
	`${JSON.stringify(manifest, null, 2)}\n`,
)
await rm(assets, { recursive: true, force: true })
const { rename } = await import('node:fs/promises')
await rename(stage, assets)
console.log(`Prepared ${fonts.length} paired web/PDF font families.`)
