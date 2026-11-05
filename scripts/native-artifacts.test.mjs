// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import {
	nativeBuildInfo,
	nativeSourceDigest,
	packagedNativePath,
	targets,
	verifyNativeArtifacts,
} from './release-utils.mjs'
import { buildNative } from './build-native.mjs'

test('shared Rust format changes invalidate native artifacts while UI starters do not', async () => {
	const directory = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-native-source-'),
	)
	try {
		for (const file of [
			'backend/src/main.rs',
			'backend/format/src/canonical_json.rs',
			'backend/resources/workspace-starter.json',
			'rust-toolchain.toml',
			'fonts/catalogue.json',
			'backend/Cargo.toml',
			'backend/format/Cargo.toml',
			'backend/Cargo.lock',
			'backend/build.rs',
			'scripts/build-native.mjs',
			'scripts/release-utils.mjs',
		]) {
			const destination = path.join(directory, file)
			await mkdir(path.dirname(destination), { recursive: true })
			await writeFile(destination, file)
		}
		const before = await nativeSourceDigest(directory)
		await writeFile(
			path.join(directory, 'backend/resources/workspace-starter.json'),
			'new widgets',
		)
		assert.equal(await nativeSourceDigest(directory), before)
		await writeFile(
			path.join(directory, 'backend/format/src/canonical_json.rs'),
			'new encoding',
		)
		assert.notEqual(await nativeSourceDigest(directory), before)
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})

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

test('packaged verification reads npm bin and runtime paths and records them', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-packaged-'))
	const identity = { version: '26.10.1-alpha', publicCommit: 'a'.repeat(40) }
	const marker = (target) =>
		`BLUINK_NATIVE_BUILD_INFO_V1:${JSON.stringify({ sourceDigest: 'current', target, mode: 'release', ...identity })}:END_BLUINK_NATIVE_BUILD_INFO`
	try {
		for (const target of targets) {
			const file = path.join(directory, packagedNativePath(target))
			await mkdir(path.dirname(file), { recursive: true })
			await writeFile(file, marker(target.rust))
		}
		const native = await verifyNativeArtifacts(
			directory,
			'current',
			false,
			identity,
			{ packaged: true },
		)
		assert.deepEqual(
			native.map((entry) => [entry.file, entry.path]),
			[
				['bluink-linux-x64', 'runtime/bluink-linux-x64'],
				['bluink-win32-x64.exe', 'bin/bluink.exe'],
				['bluink-darwin-arm64', 'runtime/bluink-darwin-arm64'],
			],
		)
		// The Windows executable is only ever at the bin path in a package.
		await assert.rejects(
			verifyNativeArtifacts(
				path.join(directory, 'runtime'),
				'current',
				false,
				identity,
			),
			/Missing bluink-win32-x64\.exe/,
		)
		const host = targets.find(
			(value) =>
				value.platform === process.platform && value.arch === process.arch,
		)
		assert.equal(packagedNativePath(host, true), 'bin/bluink.exe')
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})

test('build identity ignores the bare marker strings the executable also holds', () => {
	const info = { sourceDigest: 'current', target: targets[0].rust }
	const marker = (text) =>
		Buffer.from(
			`BLUINK_NATIVE_BUILD_INFO_V1:${text}:END_BLUINK_NATIVE_BUILD_INFO`,
		)
	const strings = Buffer.concat([marker(''), Buffer.from('Arguments\0')])
	assert.deepEqual(
		nativeBuildInfo(Buffer.concat([strings, marker(JSON.stringify(info))])),
		info,
	)
	assert.throws(() => nativeBuildInfo(strings), /no build identity/)
	assert.throws(
		() =>
			nativeBuildInfo(
				Buffer.concat([
					marker(JSON.stringify(info)),
					marker(JSON.stringify({ ...info, target: 'other' })),
				]),
			),
		/more than one build identity/,
	)
})
