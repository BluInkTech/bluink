// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { tar } from '../../scripts/release-utils.mjs'

// A synthetic USTAR/XZ archive containing only sample.txt, with fixed metadata.
const fixture = Buffer.from(
	'/Td6WFoAAATm1rRGAgAhARYAAAB0L+Wj4Cf/AH9dADmYSf7v5OzsHQflt4yRPF8OVHwZ9tQdj4GOr86Xwy5lGzZPHi67w3j/e8qTe9W5EdSuiN9d1IkaZ/Fe67guGRVpmLR/SzqIBwtRzxEP6trFpN5qoaCYxxBfZ/RzzKHjzEnsYdU+qwP+orK9VVfc8F2rFW0J0xnqpQHCl2hSLQAAAJujTMGHxQsYAAGbAYBQAABmg3EIscRn+wIAAAAABFla',
	'base64',
)

test('XZ extraction handles absolute archive paths with spaces without Git tar or external xz on Windows', async () => {
	const folder = await mkdtemp(path.join(os.tmpdir(), 'bluink tar test '))
	try {
		const archive = path.join(folder, 'fixture archive.tar.xz')
		const destination = path.join(folder, 'output directory')
		await mkdir(destination)
		await writeFile(archive, fixture)
		await tar(['-xf', archive, '-C', destination], {
			capture: true,
			// No PATH lookup or separate compressor should be needed on Windows.
			env:
				process.platform === 'win32'
					? { ...process.env, PATH: '' }
					: process.env,
		})
		assert.equal(
			await readFile(path.join(destination, 'sample.txt'), 'utf8'),
			'Bluink archive extraction fixture.\n',
		)
	} finally {
		await rm(folder, { recursive: true, force: true })
	}
})

test('failed archive extraction reports the tool diagnostic', async () => {
	const folder = await mkdtemp(path.join(os.tmpdir(), 'bluink tar failure '))
	try {
		const archive = path.join(folder, 'invalid.tar.xz')
		await writeFile(archive, 'This is not an archive.')
		await assert.rejects(
			tar(['-xf', archive, '-C', folder], { capture: true }),
			(error) => {
				assert.match(error.message, /failed \([1-9]\d*\)/)
				assert.ok(error.message.split('). ').at(-1).trim().length > 0)
				return true
			},
		)
	} finally {
		await rm(folder, { recursive: true, force: true })
	}
})
