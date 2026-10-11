// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { access, mkdir, realpath, symlink } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'
import { promisify } from 'node:util'
import { withTempDir } from './fixtures.js'

test('temporary fixtures resolve a linked system temp directory and clean up', async () => {
	await withTempDir(async (directory) => {
		const target = path.join(directory, 'physical')
		const alias = path.join(directory, 'linked')
		await mkdir(target)
		await symlink(
			target,
			alias,
			process.platform === 'win32' ? 'junction' : 'dir',
		)
		const { stdout } = await promisify(execFile)(
			process.execPath,
			[
				'--import',
				'tsx',
				'--input-type=module',
				'-e',
				'const {withTempDir} = await import(process.argv[1]); await withTempDir(async directory => console.log(directory));',
				new URL('./fixtures.ts', import.meta.url).href,
			],
			{
				env: { ...process.env, TMPDIR: alias, TEMP: alias, TMP: alias },
				timeout: 10_000,
			},
		)
		const fixture = stdout.trim()
		assert.equal(path.dirname(fixture), await realpath(target))
		assert.match(path.basename(fixture), /^bluink-test-/)
		await assert.rejects(access(fixture), { code: 'ENOENT' })
	})
})
