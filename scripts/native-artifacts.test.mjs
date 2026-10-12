// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { targets, verifyNativeArtifacts } from './release-utils.mjs'
import { buildNative } from './build-native.mjs'

test('foreign local builds fail before running a compiler or reading artifacts', async () => {
	const target = targets.find(
		(value) =>
			value.platform !== process.platform || value.arch !== process.arch,
	)
	await assert.rejects(buildNative(target.rust), /native platform.*CI runner/)
	await assert.rejects(buildNative('unsupported-target'), /Unsupported native/)
})

test('native verification checks every target, release version and public commit', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-native-'))
	const identity = { version: '26.10.1-alpha', publicCommit: 'a'.repeat(40) }
	const marker = (target, overrides = {}) =>
		`BLUINK_NATIVE_BUILD_INFO_V1:${JSON.stringify({ sourceDigest: 'current', target, ...identity, ...overrides })}:END_BLUINK_NATIVE_BUILD_INFO`
	try {
		await assert.rejects(
			verifyNativeArtifacts(directory, 'current', false, identity),
			/Missing/,
		)
		for (const target of targets)
			await writeFile(
				path.join(directory, target.artifact),
				marker(target.rust),
			)
		assert.equal(
			(await verifyNativeArtifacts(directory, 'current', false, identity))
				.length,
			3,
		)
		const target = targets[2]
		for (const overrides of [
			{ version: '26.10.2-alpha' },
			{ publicCommit: 'b'.repeat(40) },
			{ gitCommit: 'private' },
		]) {
			await writeFile(
				path.join(directory, target.artifact),
				marker(target.rust, overrides),
			)
			await assert.rejects(
				verifyNativeArtifacts(directory, 'current', false, identity),
				/release identity mismatch/,
			)
		}
		await writeFile(
			path.join(directory, target.artifact),
			marker(target.rust, { sourceDigest: 'stale' }),
		)
		await assert.rejects(
			verifyNativeArtifacts(directory, 'current', false, identity),
			/Stale or mismatched/,
		)
		await writeFile(path.join(directory, target.artifact), 'unidentified')
		await assert.rejects(
			verifyNativeArtifacts(directory, 'current', false, identity),
			/no build identity/,
		)
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})
