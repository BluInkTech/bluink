// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { contractHash, verifyCliBuild } from './cli-build.mjs'
import { fileHash, writeJson } from './release-utils.mjs'

test('prebuilt CLI artifacts bind the exact source, contract and complete output bytes', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-cli-build-'))
	try {
		await mkdir(path.join(directory, 'dist'))
		await writeFile(
			path.join(directory, 'dist/index.js'),
			'export const fixture = true\n',
		)
		await writeFile(
			path.join(directory, 'THIRD_PARTY_NOTICES.txt'),
			'Synthetic licence fixture.\n',
		)
		const identity = { version: '26.10.7-alpha', publicCommit: 'a'.repeat(40) }
		const report = {
			format: 'bluink.cli-build',
			formatVersion: 1,
			...identity,
			contractHash: await contractHash(),
			toolchain: { node: 'v26.9.0', npm: '12.2.0', rust: 'rustc 1.99.0' },
			files: Object.fromEntries(
				await Promise.all(
					['dist/index.js', 'THIRD_PARTY_NOTICES.txt'].map(async (file) => [
						file,
						await fileHash(path.join(directory, file)),
					]),
				),
			),
		}
		const manifest = path.join(directory, 'build.json')
		await writeJson(manifest, report)
		assert.deepEqual(await verifyCliBuild(directory, identity), report)
		for (const change of [
			(value) => {
				value.version = '26.10.8-alpha'
			},
			(value) => {
				value.publicCommit = 'b'.repeat(40)
			},
			(value) => {
				value.contractHash = 'c'.repeat(64)
			},
			(value) => {
				delete value.files['dist/index.js']
			},
			(value) => {
				value.files['../outside'] = 'd'.repeat(64)
			},
			(value) => {
				value.toolchain = {}
			},
		]) {
			const modified = structuredClone(report)
			change(modified)
			await writeJson(manifest, modified)
			await assert.rejects(
				verifyCliBuild(directory, identity),
				/does not match/,
			)
		}
		await writeJson(manifest, report)
		for (const file of Object.keys(report.files)) {
			const original = await readFile(path.join(directory, file))
			await writeFile(path.join(directory, file), 'Changed bytes')
			await assert.rejects(
				verifyCliBuild(directory, identity),
				/output changed/,
			)
			await writeFile(path.join(directory, file), original)
		}
		for (const file of ['dist/debug.js.map', 'dist/package.json']) {
			await writeFile(path.join(directory, file), '{}')
			await assert.rejects(
				verifyCliBuild(directory, identity),
				/Unexpected distribution/,
			)
			await rm(path.join(directory, file))
		}
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})
