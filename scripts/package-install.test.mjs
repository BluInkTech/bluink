// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import {
	mkdir,
	mkdtemp,
	readFile,
	readdir,
	rm,
	stat,
	writeFile,
} from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { installExecutable } from './package-install.mjs'
import {
	packagedNativePath,
	sha256,
	targets,
	writeJson,
} from './release-utils.mjs'

async function fixture(t, hostOnly = false) {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-install-'))
	t.after(() => rm(directory, { recursive: true, force: true }))
	const native = []
	for (const target of hostOnly ? [targets[0]] : targets) {
		const bytes = Buffer.from(`${target.rust} executable`)
		const file = packagedNativePath(target, hostOnly)
		await mkdir(path.dirname(path.join(directory, file)), { recursive: true })
		await writeFile(path.join(directory, file), bytes)
		native.push({
			target: target.rust,
			file: target.artifact,
			path: file,
			sha256: sha256(bytes),
		})
	}
	await writeJson(path.join(directory, 'release.json'), {
		version: '26.11.1-alpha',
		native,
	})
	const bin = path.join(directory, 'bin/bluink.exe')
	return { directory, bin, native }
}

test('install selects this platform executable and is safe to re-run', async (t) => {
	const { directory, bin, native } = await fixture(t)
	const windows = await readFile(bin)
	assert.equal(
		await installExecutable(directory, { platform: 'win32', arch: 'x64' }),
		undefined,
	)
	assert.deepEqual(await readFile(bin), windows)
	for (const [platform, arch, target] of [
		['linux', 'x64', 'x86_64-unknown-linux-gnu'],
		['darwin', 'arm64', 'aarch64-apple-darwin'],
	]) {
		const entry = native.find((value) => value.target === target)
		for (let run = 0; run < 2; run++) {
			assert.equal(await installExecutable(directory, { platform, arch }), bin)
			assert.equal(sha256(await readFile(bin)), entry.sha256)
			if (process.platform !== 'win32')
				assert.equal((await stat(bin)).mode & 0o777, 0o755)
		}
	}
	// No temporary copy is left beside the bin.
	assert.deepEqual(await readdir(path.join(directory, 'bin')), ['bluink.exe'])
})

test('install refuses unsupported platforms and altered or missing executables', async (t) => {
	const { directory, bin } = await fixture(t)
	const before = await readFile(bin)
	await assert.rejects(
		installExecutable(directory, { platform: 'freebsd', arch: 'x64' }),
		/does not support freebsd\/x64.*linux-x64, win32-x64, darwin-arm64/,
	)
	await assert.rejects(
		installExecutable(directory, { platform: 'linux', arch: 'arm64' }),
		/does not support linux\/arm64/,
	)
	await writeFile(path.join(directory, 'runtime/bluink-linux-x64'), 'altered')
	await assert.rejects(
		installExecutable(directory, { platform: 'linux', arch: 'x64' }),
		/runtime\/bluink-linux-x64 is not the executable recorded/,
	)
	await rm(path.join(directory, 'runtime/bluink-darwin-arm64'))
	await assert.rejects(
		installExecutable(directory, { platform: 'darwin', arch: 'arm64' }),
		/runtime\/bluink-darwin-arm64 is missing/,
	)
	assert.deepEqual(await readFile(bin), before)
})

test('a host-only package already holds its executable at the bin path', async (t) => {
	const { directory, bin, native } = await fixture(t, true)
	assert.equal(native[0].path, 'bin/bluink.exe')
	assert.equal(
		await installExecutable(directory, { platform: 'linux', arch: 'x64' }),
		bin,
	)
	assert.equal(sha256(await readFile(bin)), native[0].sha256)
	await assert.rejects(
		installExecutable(directory, { platform: 'darwin', arch: 'arm64' }),
		/does not support darwin\/arm64.*linux-x64\./,
	)
})
