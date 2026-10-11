// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import sources from '../sources.json' with { type: 'json' }
import { createHash } from 'node:crypto'
import {
	chmod,
	cp,
	mkdir,
	mkdtemp,
	readFile,
	rm,
	writeFile,
} from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { inflateRawSync } from 'node:zlib'
import { tar } from '../../scripts/release-utils.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const cache = path.join(root, '.cache')
const vendor = path.join(root, 'assets')
const { version, targets } = sources

/** Download only pinned upstream archives, verify bytes, and stage their license. */
export async function prepareTypst(requested = Object.keys(targets)) {
	await mkdir(cache, { recursive: true })
	await mkdir(vendor, { recursive: true })
	for (const target of requested) {
		const spec = targets[target]
		if (!spec) throw new Error(`Unsupported Typst target: ${target}`)
		const archivePath = path.join(cache, spec.archive)
		let bytes
		try {
			bytes = await readFile(archivePath)
		} catch {
			/* fetch below */
		}
		if (bytes && sha256(bytes) !== spec.sha256) {
			await rm(archivePath, { force: true })
			bytes = undefined
		}
		if (!bytes) {
			const url = `https://github.com/typst/typst/releases/download/v${version}/${spec.archive}`
			const response = await fetch(url, { signal: AbortSignal.timeout(90_000) })
			if (!response.ok)
				throw new Error(
					`Cannot download Typst ${version} for ${target}: HTTP ${response.status}`,
				)
			if (
				Number(response.headers.get('content-length') ?? 0) >
				64 * 1024 * 1024
			)
				throw new Error(
					`Typst ${target} archive exceeds the 64 MiB download limit.`,
				)
			bytes = Buffer.from(await response.arrayBuffer())
			if (bytes.length > 64 * 1024 * 1024)
				throw new Error(
					`Typst ${target} archive exceeds the 64 MiB download limit.`,
				)
			if (sha256(bytes) !== spec.sha256)
				throw new Error(`Typst ${target} checksum mismatch.`)
			await writeFile(archivePath, bytes)
		}
		const destination = path.join(vendor, target)
		const temporary = await mkdtemp(
			path.join(os.tmpdir(), 'bluink-typst-stage-'),
		)
		try {
			if (spec.archive.endsWith('.zip'))
				await extractZipFiles(bytes, spec.directory, temporary)
			else await tar(['-xf', archivePath, '-C', temporary], { capture: true })
			const source = path.join(temporary, spec.directory)
			await rm(destination, { recursive: true, force: true })
			await mkdir(destination, { recursive: true })
			for (const filename of [spec.binary, 'LICENSE', 'NOTICE', 'README.md'])
				await cp(path.join(source, filename), path.join(destination, filename))
			if (spec.binary !== 'typst.exe')
				await chmod(path.join(destination, spec.binary), 0o755)
			if (
				sha256(await readFile(path.join(destination, spec.binary))) !==
				spec.binarySha256
			)
				throw new Error(`Typst ${target} executable checksum mismatch.`)
			console.log(`Prepared Typst ${version} for ${target}`)
		} finally {
			await rm(temporary, { recursive: true, force: true })
		}
	}
}

function sha256(bytes) {
	return createHash('sha256').update(bytes).digest('hex')
}

async function extractZipFiles(bytes, directory, destination) {
	let eocd = -1
	for (
		let offset = bytes.length - 22;
		offset >= Math.max(0, bytes.length - 65_557);
		offset--
	)
		if (bytes.readUInt32LE(offset) === 0x06054b50) {
			eocd = offset
			break
		}
	if (eocd < 0) throw new Error('Invalid Typst ZIP archive.')
	const entries = bytes.readUInt16LE(eocd + 10)
	let offset = bytes.readUInt32LE(eocd + 16)
	const wanted = new Set(['typst.exe', 'LICENSE', 'NOTICE', 'README.md'])
	for (let index = 0; index < entries; index++) {
		if (bytes.readUInt32LE(offset) !== 0x02014b50)
			throw new Error('Invalid Typst ZIP directory.')
		const method = bytes.readUInt16LE(offset + 10)
		const compressedSize = bytes.readUInt32LE(offset + 20)
		const uncompressedSize = bytes.readUInt32LE(offset + 24)
		const nameLength = bytes.readUInt16LE(offset + 28)
		const extraLength = bytes.readUInt16LE(offset + 30)
		const commentLength = bytes.readUInt16LE(offset + 32)
		const localOffset = bytes.readUInt32LE(offset + 42)
		const name = bytes.toString('utf8', offset + 46, offset + 46 + nameLength)
		offset += 46 + nameLength + extraLength + commentLength
		if (!name.startsWith(`${directory}/`)) continue
		const filename = name.slice(directory.length + 1)
		if (!wanted.has(filename)) continue
		if (bytes.readUInt32LE(localOffset) !== 0x04034b50)
			throw new Error('Invalid Typst ZIP entry.')
		const localNameLength = bytes.readUInt16LE(localOffset + 26)
		const localExtraLength = bytes.readUInt16LE(localOffset + 28)
		const dataOffset = localOffset + 30 + localNameLength + localExtraLength
		const compressed = bytes.subarray(dataOffset, dataOffset + compressedSize)
		const result =
			method === 8
				? inflateRawSync(compressed)
				: method === 0
					? compressed
					: undefined
		if (!result || result.length !== uncompressedSize)
			throw new Error(`Invalid Typst ZIP content: ${filename}`)
		const target = path.join(destination, directory, filename)
		await mkdir(path.dirname(target), { recursive: true })
		await writeFile(target, result)
	}
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
	await prepareTypst(
		process.argv.slice(2).length ? process.argv.slice(2) : undefined,
	)
