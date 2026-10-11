// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { readFile, stat, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { gzipSync } from 'node:zlib'

/** Fixed USTAR subset for the already audited CLI file set, including its lock. */
export async function writeCliArchive(directory, files, archive) {
	const blocks = []
	for (const file of files.toSorted()) {
		const name = `package/${file}`
		if (
			Buffer.byteLength(name) > 100 ||
			!/^[\w./-]+$/.test(name) ||
			name.split('/').includes('..')
		)
			throw new Error(`Unsupported CLI archive path: ${file}`)
		const filename = path.join(directory, file)
		const info = await stat(filename)
		if (!info.isFile())
			throw new Error(`CLI archives require regular files: ${file}`)
		const bytes = await readFile(filename)
		const header = Buffer.alloc(512)
		header.write(name, 0, 100)
		const octal = (value, offset, width) => {
			const text = value.toString(8)
			if (text.length >= width)
				throw new Error('CLI archive header limit exceeded.')
			header.write(`${text.padStart(width - 1, '0')}\0`, offset, width)
		}
		octal(file === 'bluink' ? 0o755 : 0o644, 100, 8)
		octal(0, 108, 8)
		octal(0, 116, 8)
		octal(bytes.length, 124, 12)
		octal(499162500, 136, 12)
		header.fill(32, 148, 156)
		header[156] = 48
		header.write('ustar\0', 257, 6)
		header.write('00', 263, 2)
		const checksum = header.reduce((sum, byte) => sum + byte, 0)
		header.write(`${checksum.toString(8).padStart(6, '0')}\0 `, 148, 8)
		blocks.push(header, bytes, Buffer.alloc((512 - (bytes.length % 512)) % 512))
	}
	blocks.push(Buffer.alloc(1024))
	await writeFile(archive, gzipSync(Buffer.concat(blocks), { level: 9 }))
}
