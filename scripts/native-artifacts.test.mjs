// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import vm from 'node:vm'
import { targets, verifyNativeArtifacts } from './release-utils.mjs'
import { buildNative } from './build-native.mjs'

test('foreign local builds fail before running a compiler or reading artifacts', async () => {
	const target = targets.find(
		(value) =>
			value.platform !== process.platform || value.arch !== process.arch,
	)
	await assert.rejects(buildNative(target.rust), /native platform.*CI runner/)
	await assert.rejects(buildNative('unsupported-target'), /Unsupported native/)
	await assert.rejects(
		buildNative(undefined, { mode: 'fast' }),
		/Unknown native build mode/,
	)
})

test('a public release identity cannot be used for a development build', async () => {
	const previous = process.env.BLUINK_PUBLIC_COMMIT
	try {
		process.env.BLUINK_PUBLIC_COMMIT = 'a'.repeat(40)
		await assert.rejects(
			buildNative(undefined, { mode: 'development' }),
			/must use release mode/,
		)
	} finally {
		if (previous === undefined) delete process.env.BLUINK_PUBLIC_COMMIT
		else process.env.BLUINK_PUBLIC_COMMIT = previous
	}
})

test('the addon loader explicitly selects one mode without falling back to another artifact', async () => {
	const source = await readFile(
		new URL('../backend/runtime/index.cjs', import.meta.url),
		'utf8',
	)
	const target = targets.find(
		(value) =>
			value.platform === process.platform && value.arch === process.arch,
	)
	const directory = path.resolve('test-runtime')
	for (const mode of [undefined, 'release', 'development']) {
		const loaded = []
		const context = {
			__dirname: directory,
			module: { exports: {} },
			process: {
				platform: process.platform,
				arch: process.arch,
				env: { BLUINK_NATIVE_MODE: mode },
			},
			require: (name) => {
				if (name === 'node:path') return path
				loaded.push(name)
				throw new Error('Missing selected artifact')
			},
		}
		assert.throws(
			() => vm.runInNewContext(source, context),
			/Missing selected artifact/,
		)
		assert.deepEqual(loaded, [
			path.join(
				directory,
				mode === 'development' ? 'development' : '',
				target.artifact,
			),
		])
		context.process.env.BLUINK_NATIVE_MODE = 'unknown'
		assert.throws(
			() => vm.runInNewContext(source, { ...context }),
			/Unknown BLUINK_NATIVE_MODE/,
		)
	}
})

test('native verification checks every target, release version and public commit', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-native-'))
	const identity = { version: '26.10.1-alpha', publicCommit: 'a'.repeat(40) }
	const marker = (target, overrides = {}) =>
		`BLUINK_NATIVE_BUILD_INFO_V1:${JSON.stringify({ sourceDigest: 'current', target, mode: 'release', ...identity, ...overrides })}:END_BLUINK_NATIVE_BUILD_INFO`
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
		for (const mode of ['development', undefined, 'unknown']) {
			await writeFile(
				path.join(directory, target.artifact),
				marker(target.rust, { mode }),
			)
			await assert.rejects(
				verifyNativeArtifacts(directory, 'current', false, identity),
				/not a release build/,
			)
		}
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
